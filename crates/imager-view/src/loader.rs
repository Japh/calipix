//! Background reading of tiles, so that region reads from a lazily loaded
//! image never run on the interface thread.
//!
//! The view replaces the list of wanted work every frame, most important
//! first. Worker threads take the first entry nobody is working on, so work
//! that scrolled out of view before it was started is simply never done.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use imager_core::{Buffer, Image, RealPixel, Rect, dispatch_real};

use crate::tiles::{TileGrid, TileKey, read_tile};

/// Something to compute off the interface thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Work {
    /// The samples of one tile.
    Tile(TileKey),
    /// The smallest and largest sample of a whole plane.
    Range(usize),
}

/// The outcome of one piece of [`Work`].
pub enum Loaded {
    Tile(TileKey, Result<Buffer, String>),
    Range(usize, Result<(f64, f64), String>),
}

/// Called from a worker thread when a result is ready, to wake the
/// interface (for example `egui::Context::request_repaint`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Queue {
    wanted: Vec<Work>,
    in_flight: HashSet<Work>,
    /// Finished work whose result the view has not collected yet. It must
    /// not be started again just because the view, not having seen the
    /// result, still lists it as wanted.
    delivered: HashSet<Work>,
    closed: bool,
}

struct Shared {
    image: Image,
    grid: TileGrid,
    queue: Mutex<Queue>,
    wake_workers: Condvar,
    waker: Mutex<Option<Waker>>,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Worker threads reading tiles of one image. Dropping the loader lets the
/// threads finish what they are doing and exit.
pub struct Loader {
    shared: Arc<Shared>,
    results: Receiver<Loaded>,
}

impl Loader {
    /// Starts `threads` workers for `image`.
    pub fn new(image: Image, grid: TileGrid, threads: usize) -> Loader {
        let shared = Arc::new(Shared {
            image,
            grid,
            queue: Mutex::new(Queue::default()),
            wake_workers: Condvar::new(),
            waker: Mutex::new(None),
        });
        let (tx, results) = channel();
        for i in 0..threads.max(1) {
            let shared = shared.clone();
            let tx = tx.clone();
            // If a thread cannot be started the others carry on; with none
            // at all tiles stay blank, which the view reports as loading.
            let _ = std::thread::Builder::new()
                .name(format!("imager-view tile loader {i}"))
                .spawn(move || worker(&shared, &tx));
        }
        Loader { shared, results }
    }

    /// Sets how the interface is told that a result is ready.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.shared.waker.lock().unwrap_or_else(|e| e.into_inner()) = waker;
    }

    /// Replaces the outstanding work, most important first. Work already
    /// being done is finished regardless.
    pub fn set_wanted(&self, wanted: Vec<Work>) {
        let mut queue = self.shared.queue();
        queue.wanted = wanted;
        let Queue {
            wanted, delivered, ..
        } = &mut *queue;
        wanted.retain(|work| !delivered.contains(work));
        drop(queue);
        self.shared.wake_workers.notify_all();
    }

    /// True if `work` is queued or being done.
    pub fn is_pending(&self, work: &Work) -> bool {
        let queue = self.shared.queue();
        queue.in_flight.contains(work)
            || queue.wanted.contains(work)
            || queue.delivered.contains(work)
    }

    /// The next finished result, if any; never blocks.
    pub fn try_recv(&self) -> Option<Loaded> {
        let loaded = self.results.try_recv().ok()?;
        let work = match &loaded {
            Loaded::Tile(key, _) => Work::Tile(*key),
            Loaded::Range(plane, _) => Work::Range(*plane),
        };
        self.shared.queue().delivered.remove(&work);
        Some(loaded)
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let mut queue = self.shared.queue();
        queue.closed = true;
        queue.wanted.clear();
        drop(queue);
        self.shared.wake_workers.notify_all();
    }
}

fn worker(shared: &Shared, results: &Sender<Loaded>) {
    loop {
        let work = {
            let mut queue = shared.queue();
            loop {
                if queue.closed {
                    return;
                }
                let next = queue
                    .wanted
                    .iter()
                    .position(|w| !queue.in_flight.contains(w));
                if let Some(i) = next {
                    let work = queue.wanted.remove(i);
                    queue.in_flight.insert(work);
                    break work;
                }
                queue = shared
                    .wake_workers
                    .wait(queue)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        let loaded = match work {
            Work::Tile(key) => Loaded::Tile(
                key,
                read_tile(&shared.image, &shared.grid, key).map_err(|e| e.to_string()),
            ),
            Work::Range(plane) => Loaded::Range(
                plane,
                plane_range(&shared.image, plane).map_err(|e| e.to_string()),
            ),
        };
        {
            let mut queue = shared.queue();
            queue.in_flight.remove(&work);
            queue.wanted.retain(|w| *w != work);
            queue.delivered.insert(work);
        }
        if results.send(loaded).is_err() {
            return;
        }
        let waker = shared
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(wake) = waker {
            wake();
        }
    }
}

/// Rows read at a time when scanning a plane for its range.
const RANGE_BAND_SAMPLES: usize = 4 << 20;

/// The smallest and largest non-NaN sample of a plane, `(0, 0)` if there is
/// none: the same answer as `imager_render::default_display_range` gives for
/// a non-8-bit plane, computed a band at a time so that a plane larger than
/// memory can be scanned.
pub fn plane_range(image: &Image, plane: usize) -> imager_core::Result<(f64, f64)> {
    fn scan<T: RealPixel>(samples: &[T], range: &mut Option<(f64, f64)>) {
        for v in samples.iter().map(|v| v.to_f64()).filter(|v| !v.is_nan()) {
            *range = Some(match *range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
    }
    let meta = image.meta();
    let (width, height) = (meta.width(), meta.height());
    let mut band = (RANGE_BAND_SAMPLES / width).max(1);
    if let Some((_, th)) = image.tile_size() {
        band = (band / th.max(1)).max(1) * th.max(1);
    }
    let mut range = None;
    let mut y = 0;
    while y < height {
        let rows = band.min(height - y);
        let samples = image.read_region(plane, Rect::new(0, y, width, rows))?;
        dispatch_real!(&samples, |s| scan(s, &mut range), complex => {
            return Err(imager_core::Error::Unsupported(
                "complex images cannot be displayed directly".to_string(),
            ));
        });
        y += rows;
    }
    Ok(range.unwrap_or((0.0, 0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn wait_for(loader: &Loader, n: usize) -> Vec<Loaded> {
        let start = Instant::now();
        let mut got = Vec::new();
        while got.len() < n {
            match loader.try_recv() {
                Some(l) => got.push(l),
                None => {
                    assert!(start.elapsed() < Duration::from_secs(20), "loader stalled");
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        got
    }

    #[test]
    fn loads_tiles_and_ranges_and_wakes() {
        let data: Vec<u16> = (0..64 * 48).map(|i| i as u16).collect();
        let image = Image::from_vec(64, 48, data).unwrap();
        let grid = TileGrid::new(64, 48, 32);
        let loader = Loader::new(image.clone(), grid, 2);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        loader.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        let key = TileKey {
            plane: 0,
            level: 0,
            tx: 1,
            ty: 1,
        };
        loader.set_wanted(vec![Work::Tile(key), Work::Range(0)]);
        let got = wait_for(&loader, 2);
        for loaded in got {
            match loaded {
                Loaded::Tile(k, tile) => {
                    assert_eq!(k, key);
                    let tile = tile.unwrap();
                    assert_eq!(tile.len(), 32 * 16);
                    assert_eq!(tile.get_f64(0), Some((32 * 64 + 32) as f64));
                }
                Loaded::Range(plane, range) => {
                    assert_eq!(plane, 0);
                    assert_eq!(range.unwrap(), (0.0, (64 * 48 - 1) as f64));
                }
            }
        }
        // The wake for the last result may still be on its way.
        let start = Instant::now();
        while wakes.load(Ordering::SeqCst) < 2 {
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!loader.is_pending(&Work::Tile(key)));
        assert!(!loader.is_pending(&Work::Range(0)));
    }

    #[test]
    fn work_is_done_once_however_often_it_is_asked_for() {
        use imager_core::{Buffer, ImageMeta, PixelSource};
        #[derive(Debug)]
        struct Counting(ImageMeta, Arc<AtomicUsize>);
        impl PixelSource for Counting {
            fn meta(&self) -> &ImageMeta {
                &self.0
            }
            fn read_plane(&self, _: usize) -> imager_core::Result<Buffer> {
                self.1.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                Ok(Buffer::zeros(self.0.dtype(), self.0.plane_len()))
            }
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let meta = ImageMeta::plane(imager_core::DType::U8, 8, 8).unwrap();
        let image = Image::new(Counting(meta, reads.clone()));
        let loader = Loader::new(image, TileGrid::new(8, 8, 8), 3);
        let work = Work::Tile(TileKey {
            plane: 0,
            level: 0,
            tx: 0,
            ty: 0,
        });
        // The view asks again every frame until it has collected the result.
        let start = Instant::now();
        let mut got = None;
        while got.is_none() {
            loader.set_wanted(vec![work]);
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(1));
            got = loader.try_recv();
        }
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(1));
            assert!(loader.try_recv().is_none());
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let image = Image::from_vec(8, 8, vec![0u8; 64]).unwrap();
        let loader = Loader::new(image, TileGrid::new(8, 8, 8), 1);
        let outside = TileKey {
            plane: 0,
            level: 0,
            tx: 5,
            ty: 0,
        };
        loader.set_wanted(vec![Work::Tile(outside), Work::Range(3)]);
        for loaded in wait_for(&loader, 2) {
            match loaded {
                Loaded::Tile(_, r) => assert!(r.unwrap_err().contains("outside")),
                Loaded::Range(_, r) => assert!(r.unwrap_err().contains("plane 3")),
            }
        }
    }

    #[test]
    fn range_matches_the_reference_renderer() {
        let data = vec![f32::NAN, 2.0, -3.0, 7.5, f32::NAN, 0.0];
        let image = Image::from_vec(3, 2, data).unwrap();
        let reference =
            imager_render::default_display_range(&image.read_plane(0).unwrap()).unwrap();
        assert_eq!(plane_range(&image, 0).unwrap(), reference);
        let nan = Image::from_vec(2, 1, vec![f32::NAN; 2]).unwrap();
        let reference = imager_render::default_display_range(&nan.read_plane(0).unwrap()).unwrap();
        assert_eq!(plane_range(&nan, 0).unwrap(), reference);
    }
}
