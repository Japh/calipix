//! A shared, byte-budgeted cache of decoded planes, so that images read
//! lazily from disk stay responsive without holding everything in memory.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{Buffer, Image, ImageMeta, PixelSource, Rect, Result, crop_buffer};

/// Default budget of the [global](PlaneCache::global) cache: 1 GiB.
const DEFAULT_BUDGET: usize = 1 << 30;

struct Entry {
    plane: Buffer,
    last_used: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<(u64, usize), Entry>,
    bytes: usize,
    clock: u64,
}

/// Least-recently-used cache of planes, limited by total sample bytes.
pub struct PlaneCache {
    state: Mutex<State>,
    budget: AtomicU64,
}

impl PlaneCache {
    pub fn new(budget_bytes: usize) -> Arc<PlaneCache> {
        Arc::new(PlaneCache {
            state: Mutex::new(State::default()),
            budget: AtomicU64::new(budget_bytes as u64),
        })
    }

    /// The cache shared by every lazily read image in the process.
    pub fn global() -> &'static Arc<PlaneCache> {
        static GLOBAL: OnceLock<Arc<PlaneCache>> = OnceLock::new();
        GLOBAL.get_or_init(|| PlaneCache::new(DEFAULT_BUDGET))
    }

    pub fn budget(&self) -> usize {
        self.budget.load(Ordering::Relaxed) as usize
    }

    /// Changes the budget, evicting planes if it shrank.
    pub fn set_budget(&self, budget_bytes: usize) {
        self.budget.store(budget_bytes as u64, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        evict(&mut state, budget_bytes);
    }

    /// Sample bytes currently held.
    pub fn bytes_used(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bytes
    }

    fn get(&self, key: (u64, usize)) -> Option<Buffer> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clock += 1;
        let now = state.clock;
        let entry = state.entries.get_mut(&key)?;
        entry.last_used = now;
        Some(entry.plane.clone())
    }

    fn insert(&self, key: (u64, usize), plane: &Buffer) {
        let budget = self.budget();
        let size = plane.byte_len();
        // A plane that could never fit would only push everything else out.
        if size > budget {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clock += 1;
        let entry = Entry {
            plane: plane.clone(),
            last_used: state.clock,
        };
        if let Some(old) = state.entries.insert(key, entry) {
            state.bytes -= old.plane.byte_len();
        }
        state.bytes += size;
        evict(&mut state, budget);
    }

    fn forget(&self, source: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut freed = 0;
        state.entries.retain(|key, entry| {
            let keep = key.0 != source;
            if !keep {
                freed += entry.plane.byte_len();
            }
            keep
        });
        state.bytes -= freed;
    }
}

fn evict(state: &mut State, budget: usize) {
    while state.bytes > budget {
        let Some(oldest) = state
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| *k)
        else {
            break;
        };
        if let Some(entry) = state.entries.remove(&oldest) {
            state.bytes -= entry.plane.byte_len();
        }
    }
}

/// Wraps a slow [`PixelSource`] (typically a file reader) so that whole
/// planes it produces are kept in a [`PlaneCache`].
///
/// Region reads are served from a cached plane when there is one and are
/// otherwise passed straight to the wrapped source, so a tiled source is
/// never forced to decode a whole plane just to show part of it.
pub struct Cached<S> {
    source: S,
    id: u64,
    cache: Arc<PlaneCache>,
}

impl<S: PixelSource> Cached<S> {
    pub fn new(source: S, cache: Arc<PlaneCache>) -> Cached<S> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Cached {
            source,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            cache,
        }
    }

    pub fn inner(&self) -> &S {
        &self.source
    }
}

impl<S> Drop for Cached<S> {
    fn drop(&mut self) {
        self.cache.forget(self.id);
    }
}

impl<S: PixelSource> std::fmt::Debug for Cached<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cached")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl<S: PixelSource> PixelSource for Cached<S> {
    fn meta(&self) -> &ImageMeta {
        self.source.meta()
    }

    fn read_plane(&self, plane: usize) -> Result<Buffer> {
        let key = (self.id, plane);
        if let Some(hit) = self.cache.get(key) {
            return Ok(hit);
        }
        // The lock is not held while loading; two threads asking for the
        // same plane may both read it, which is harmless.
        let loaded = self.source.read_plane(plane)?;
        self.cache.insert(key, &loaded);
        Ok(loaded)
    }

    fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
        match self.cache.get((self.id, plane)) {
            Some(hit) => crop_buffer(&hit, self.meta().width(), region),
            None => self.source.read_region(plane, region),
        }
    }

    fn tile_size(&self) -> Option<(usize, usize)> {
        self.source.tile_size()
    }
}

impl Image {
    /// An image over a slow source, with its planes kept in the
    /// [global cache](PlaneCache::global).
    pub fn cached(source: impl PixelSource + 'static) -> Image {
        Image::new(Cached::new(source, PlaneCache::global().clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DType;
    use std::sync::atomic::AtomicUsize;

    /// A source that counts how often it is asked for data.
    #[derive(Debug)]
    struct Counting {
        meta: ImageMeta,
        plane_reads: Arc<AtomicUsize>,
        region_reads: Arc<AtomicUsize>,
    }

    impl PixelSource for Counting {
        fn meta(&self) -> &ImageMeta {
            &self.meta
        }

        fn read_plane(&self, plane: usize) -> Result<Buffer> {
            self.plane_reads.fetch_add(1, Ordering::Relaxed);
            Ok(Buffer::from_vec(vec![plane as u8; self.meta.plane_len()]))
        }

        fn read_region(&self, plane: usize, region: Rect) -> Result<Buffer> {
            self.region_reads.fetch_add(1, Ordering::Relaxed);
            Ok(Buffer::from_vec(vec![plane as u8; region.w * region.h]))
        }
    }

    fn setup(budget: usize) -> (Image, Arc<PlaneCache>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let cache = PlaneCache::new(budget);
        let (planes, regions) = (Arc::default(), Arc::default());
        let source = Counting {
            // 10 x 10 samples of one byte: 100 bytes per plane, 4 planes.
            meta: ImageMeta::xyczt(DType::U8, 10, 10, 1, 4, 1).unwrap(),
            plane_reads: Arc::clone(&planes),
            region_reads: Arc::clone(&regions),
        };
        let image = Image::new(Cached::new(source, cache.clone()));
        (image, cache, planes, regions)
    }

    #[test]
    fn repeated_reads_hit_the_cache() {
        let (image, cache, planes, _) = setup(1000);
        for _ in 0..3 {
            assert_eq!(image.read_plane(2).unwrap().get_f64(0), Some(2.0));
        }
        assert_eq!(planes.load(Ordering::Relaxed), 1);
        assert_eq!(cache.bytes_used(), 100);
    }

    #[test]
    fn least_recently_used_plane_is_evicted() {
        let (image, cache, planes, _) = setup(250);
        image.read_plane(0).unwrap();
        image.read_plane(1).unwrap();
        image.read_plane(0).unwrap(); // plane 1 is now the oldest
        image.read_plane(2).unwrap(); // evicts plane 1
        assert_eq!(cache.bytes_used(), 200);
        assert_eq!(planes.load(Ordering::Relaxed), 3);
        image.read_plane(0).unwrap();
        assert_eq!(planes.load(Ordering::Relaxed), 3);
        image.read_plane(1).unwrap();
        assert_eq!(planes.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn regions_use_a_cached_plane_or_the_source() {
        let (image, _, planes, regions) = setup(1000);
        let rect = Rect::new(1, 1, 2, 2);
        image.read_region(3, rect).unwrap();
        assert_eq!(
            (
                planes.load(Ordering::Relaxed),
                regions.load(Ordering::Relaxed)
            ),
            (0, 1)
        );
        image.read_plane(3).unwrap();
        let region = image.read_region(3, rect).unwrap();
        assert_eq!(region.as_slice::<u8>().unwrap(), &[3; 4]);
        assert_eq!(
            (
                planes.load(Ordering::Relaxed),
                regions.load(Ordering::Relaxed)
            ),
            (1, 1)
        );
    }

    #[test]
    fn oversized_planes_and_budget_changes() {
        let (image, cache, planes, _) = setup(50);
        image.read_plane(0).unwrap();
        image.read_plane(0).unwrap();
        assert_eq!(cache.bytes_used(), 0);
        assert_eq!(planes.load(Ordering::Relaxed), 2);

        cache.set_budget(300);
        for plane in 0..3 {
            image.read_plane(plane).unwrap();
        }
        assert_eq!(cache.bytes_used(), 300);
        cache.set_budget(100);
        assert_eq!(cache.bytes_used(), 100);
    }

    #[test]
    fn dropping_the_image_frees_its_planes() {
        let (image, cache, _, _) = setup(1000);
        image.read_plane(0).unwrap();
        let view = image.slice(2, 1).unwrap();
        view.read_plane(0).unwrap();
        assert_eq!(cache.bytes_used(), 200);
        drop(image);
        assert_eq!(cache.bytes_used(), 200, "the view still needs the source");
        drop(view);
        assert_eq!(cache.bytes_used(), 0);
    }
}
