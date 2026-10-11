//! Tiling: how a plane is cut into fixed-size tiles at each decimation
//! level, how a tile's samples are read, and the byte-budgeted cache that
//! holds them.

use std::collections::HashMap;
use std::hash::Hash;

use calipix_core::{Buffer, Image, Pixel, Rect, Result, dispatch};

use crate::geometry::MAX_LEVEL;

/// One tile of one plane at one decimation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub plane: usize,
    pub level: u8,
    pub tx: u32,
    pub ty: u32,
}

/// Position of a tile within a plane, whatever the plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    pub level: u8,
    pub tx: u32,
    pub ty: u32,
}

impl TileCoord {
    pub fn on_plane(self, plane: usize) -> TileKey {
        TileKey {
            plane,
            level: self.level,
            tx: self.tx,
            ty: self.ty,
        }
    }

    /// The tile `up` levels coarser that covers this one.
    pub fn ancestor(self, up: u8) -> TileCoord {
        TileCoord {
            level: self.level + up,
            tx: self.tx >> up,
            ty: self.ty >> up,
        }
    }
}

/// The tile layout of a `width` x `height` plane.
///
/// Level `l` holds one sample for every `2^l` x `2^l` block of the plane:
/// the sample in the middle of the block ([`TileGrid::sample_position`]),
/// with no averaging, as in ImageJ's reduced views and
/// `calipix_render::Reduction::Nearest`. Each level is cut into tiles of
/// `tile` x `tile` samples, smaller at the right and bottom edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileGrid {
    pub width: usize,
    pub height: usize,
    pub tile: usize,
}

impl TileGrid {
    pub fn new(width: usize, height: usize, tile: usize) -> TileGrid {
        TileGrid {
            width,
            height,
            tile: tile.max(1),
        }
    }

    /// Distance in image pixels between samples at `level`.
    pub fn step(level: u8) -> usize {
        1usize << level.min(MAX_LEVEL)
    }

    /// The full-resolution coordinate that sample `index` of `level` is
    /// taken from, along an axis of `size` pixels: the middle of its block
    /// of `2^level` pixels, `index * 2^level + 2^level / 2`, or the last
    /// pixel if the block is cut short by the edge of the plane.
    pub fn sample_position(level: u8, index: usize, size: usize) -> usize {
        let step = TileGrid::step(level);
        (index * step + step / 2).min(size.saturating_sub(1))
    }

    /// Samples per row and rows at `level`.
    pub fn level_size(&self, level: u8) -> (usize, usize) {
        let step = TileGrid::step(level);
        (self.width.div_ceil(step), self.height.div_ceil(step))
    }

    /// The coarsest level worth using: the first at which the whole plane
    /// fits one tile.
    pub fn max_level(&self) -> u8 {
        (0..MAX_LEVEL)
            .find(|&level| {
                let (w, h) = self.level_size(level);
                w <= self.tile && h <= self.tile
            })
            .unwrap_or(MAX_LEVEL)
    }

    /// A tile's rectangle in samples of its own level, or `None` if the
    /// tile is outside the plane.
    pub fn tile_rect(&self, coord: TileCoord) -> Option<Rect> {
        let (lw, lh) = self.level_size(coord.level);
        let x = coord.tx as usize * self.tile;
        let y = coord.ty as usize * self.tile;
        (x < lw && y < lh).then(|| Rect::new(x, y, self.tile.min(lw - x), self.tile.min(lh - y)))
    }

    /// The tiles at `level` that hold samples for the image pixels in
    /// `visible`, nearest the middle of `visible` first.
    pub fn tiles_in(&self, visible: Rect, level: u8) -> Vec<TileCoord> {
        if visible.w == 0 || visible.h == 0 {
            return Vec::new();
        }
        let span = self.tile * TileGrid::step(level);
        let (lw, lh) = self.level_size(level);
        let last_tx = (lw - 1) / self.tile;
        let last_ty = (lh - 1) / self.tile;
        let tx0 = visible.x / span;
        let ty0 = visible.y / span;
        let tx1 = ((visible.x + visible.w - 1) / span).min(last_tx);
        let ty1 = ((visible.y + visible.h - 1) / span).min(last_ty);
        let mut tiles = Vec::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                tiles.push(TileCoord {
                    level,
                    tx: tx as u32,
                    ty: ty as u32,
                });
            }
        }
        let mid = |c: TileCoord| {
            let cx =
                (c.tx as f64 + 0.5) * span as f64 - (visible.x as f64 + visible.w as f64 / 2.0);
            let cy =
                (c.ty as f64 + 0.5) * span as f64 - (visible.y as f64 + visible.h as f64 / 2.0);
            cx * cx + cy * cy
        };
        tiles.sort_by(|a, b| mid(*a).total_cmp(&mid(*b)));
        tiles
    }
}

/// Most samples read from a source in one request while decimating.
const BAND_SAMPLES: usize = 4 << 20;

/// Reads the samples of one tile.
///
/// At level 0 this is a single region read. At coarser levels the kept
/// samples ([`TileGrid::sample_position`]) are picked from full-resolution
/// rows without ever holding more than a band of the plane: an in-memory
/// plane is indexed directly, a source with a natural tile height is read
/// one row of its tiles at a time, and any other source is read one needed
/// row at a time.
pub fn read_tile(image: &Image, grid: &TileGrid, key: TileKey) -> Result<Buffer> {
    let coord = TileCoord {
        level: key.level,
        tx: key.tx,
        ty: key.ty,
    };
    let rect = grid.tile_rect(coord).ok_or_else(|| {
        calipix_core::Error::OutOfBounds(format!(
            "tile {coord:?} outside a {}x{} plane",
            grid.width, grid.height
        ))
    })?;
    if key.level == 0 {
        return image.read_region(key.plane, rect);
    }
    // The full-resolution columns and rows the tile's samples come from.
    let xs: Vec<usize> = (rect.x..rect.x + rect.w)
        .map(|i| TileGrid::sample_position(key.level, i, grid.width))
        .collect();
    let ys: Vec<usize> = (rect.y..rect.y + rect.h)
        .map(|j| TileGrid::sample_position(key.level, j, grid.height))
        .collect();
    if image.is_in_memory() {
        let plane = image.read_plane(key.plane)?;
        return Ok(pick(&plane, grid.width, &xs, &ys));
    }
    let x0 = xs[0];
    let span_w = xs[xs.len() - 1] - x0 + 1;
    let columns: Vec<usize> = xs.iter().map(|x| x - x0).collect();
    let band_rows = match image.tile_size() {
        Some((_, th)) if th > 1 => {
            // Whole rows of source tiles, as many as fit the band budget.
            let rows = (BAND_SAMPLES / span_w).max(1);
            (rows / th).max(1) * th
        }
        _ => 1,
    };
    let mut parts: Vec<Buffer> = Vec::new();
    let mut next = 0; // index into `ys` of the next output row
    while next < ys.len() {
        // The needed rows that fall in the same band as the next one.
        let top = ys[next];
        let band = top / band_rows;
        let count = ys[next..]
            .iter()
            .take_while(|y| *y / band_rows == band)
            .count();
        let rows: Vec<usize> = ys[next..next + count].iter().map(|y| y - top).collect();
        let height = rows[rows.len() - 1] + 1;
        let region = image.read_region(key.plane, Rect::new(x0, top, span_w, height))?;
        parts.push(pick(&region, span_w, &columns, &rows));
        next += count;
    }
    Ok(concat(&parts))
}

/// Picks the samples at the given columns of the given rows from a
/// row-major buffer `src_w` wide.
fn pick(src: &Buffer, src_w: usize, columns: &[usize], rows: &[usize]) -> Buffer {
    fn go<T: Pixel>(src: &[T], src_w: usize, columns: &[usize], rows: &[usize]) -> Buffer {
        let mut out = Vec::with_capacity(columns.len() * rows.len());
        for &y in rows {
            let row = &src[y * src_w..(y + 1) * src_w];
            out.extend(columns.iter().map(|&x| row[x]));
        }
        Buffer::from_vec(out)
    }
    dispatch!(src, |s| go(s, src_w, columns, rows))
}

/// Joins one or more buffers of one pixel type end to end.
fn concat(parts: &[Buffer]) -> Buffer {
    if parts.len() == 1 {
        return parts[0].clone();
    }
    fn go<T: Pixel>(first: &[T], parts: &[Buffer]) -> Buffer {
        let mut out: Vec<T> = Vec::with_capacity(parts.iter().map(Buffer::len).sum());
        out.extend_from_slice(first);
        for part in &parts[1..] {
            // All parts come from one image, so the type always matches.
            out.extend_from_slice(part.as_slice::<T>().unwrap_or(&[]));
        }
        Buffer::from_vec(out)
    }
    dispatch!(&parts[0], |s| go(s, parts))
}

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

/// A least-recently-used cache limited by the total size of its values.
///
/// Every access stamps the entry with a rising counter. Eviction removes
/// the entries with the oldest stamps until the total is within budget,
/// but never an entry used since [`mark`](Self::mark) was last taken, so
/// what the current frame draws is always kept even if it alone exceeds
/// the budget.
pub struct ByteLru<K, V> {
    entries: HashMap<K, Entry<V>>,
    bytes: usize,
    clock: u64,
}

impl<K: Hash + Eq + Clone, V> Default for ByteLru<K, V> {
    fn default() -> Self {
        ByteLru {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
        }
    }
}

impl<K: Hash + Eq + Clone, V> ByteLru<K, V> {
    /// The current stamp; entries used after this are protected from
    /// [`evict`](Self::evict) when it is passed back.
    pub fn mark(&self) -> u64 {
        self.clock
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total size of the cached values in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Looks up a value and marks it as just used.
    pub fn get(&mut self, key: &K) -> Option<&mut V> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|e| {
            e.last_used = clock;
            &mut e.value
        })
    }

    /// Every cached value, in no particular order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values().map(|e| &e.value)
    }

    /// Looks up a value without affecting what is evicted next.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|e| &e.value)
    }

    /// Adds or replaces a value of the given size, marking it as just used.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.clock += 1;
        let entry = Entry {
            value,
            bytes,
            last_used: self.clock,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
    }

    /// Records that a value's size changed (for example when a GPU copy
    /// was added to it).
    pub fn resize(&mut self, key: &K, bytes: usize) {
        if let Some(e) = self.entries.get_mut(key) {
            self.bytes = self.bytes - e.bytes + bytes;
            e.bytes = bytes;
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let e = self.entries.remove(key)?;
        self.bytes -= e.bytes;
        Some(e.value)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// Removes least-recently-used entries until at most `budget` bytes
    /// remain, sparing entries used since `mark`. Returns how many were
    /// removed.
    pub fn evict(&mut self, budget: usize, mark: u64) -> usize {
        if self.bytes <= budget {
            return 0;
        }
        let mut old: Vec<(u64, K)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.last_used <= mark)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        old.sort_by_key(|(used, _)| *used);
        let mut removed = 0;
        for (_, key) in old {
            if self.bytes <= budget {
                break;
            }
            self.remove(&key);
            removed += 1;
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::{DType, ImageMeta, PixelSource};
    use proptest::prelude::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn level_sizes_and_tile_rects() {
        let grid = TileGrid::new(1000, 300, 256);
        assert_eq!(grid.level_size(0), (1000, 300));
        assert_eq!(grid.level_size(1), (500, 150));
        assert_eq!(grid.level_size(3), (125, 38));
        assert_eq!(grid.max_level(), 2);
        let c = |level, tx, ty| TileCoord { level, tx, ty };
        assert_eq!(grid.tile_rect(c(0, 0, 0)), Some(Rect::new(0, 0, 256, 256)));
        assert_eq!(
            grid.tile_rect(c(0, 3, 1)),
            Some(Rect::new(768, 256, 232, 44))
        );
        assert_eq!(grid.tile_rect(c(0, 4, 0)), None);
        assert_eq!(
            grid.tile_rect(c(1, 1, 0)),
            Some(Rect::new(256, 0, 244, 150))
        );
        assert_eq!(c(0, 3, 1).ancestor(1), c(1, 1, 0));
        assert_eq!(c(0, 3, 1).ancestor(2), c(2, 0, 0));
    }

    #[test]
    fn visible_rectangle_selects_tiles() {
        let grid = TileGrid::new(1000, 700, 256);
        let coords = |visible, level| {
            let mut t: Vec<(u32, u32)> = grid
                .tiles_in(visible, level)
                .iter()
                .map(|c| (c.tx, c.ty))
                .collect();
            t.sort();
            t
        };
        // Entirely inside one tile.
        assert_eq!(coords(Rect::new(10, 10, 100, 100), 0), [(0, 0)]);
        // Touching a tile boundary from the left does not pull in the next.
        assert_eq!(coords(Rect::new(0, 0, 256, 256), 0), [(0, 0)]);
        assert_eq!(coords(Rect::new(0, 0, 257, 256), 0), [(0, 0), (1, 0)]);
        // A window across four tiles.
        assert_eq!(
            coords(Rect::new(200, 500, 100, 100), 0),
            [(0, 1), (0, 2), (1, 1), (1, 2)]
        );
        // The whole plane: 4 x 3 tiles at level 0, 2 x 2 at level 1, 1 at 2.
        let all = Rect::new(0, 0, 1000, 700);
        assert_eq!(coords(all, 0).len(), 12);
        assert_eq!(coords(all, 1), [(0, 0), (0, 1), (1, 0), (1, 1)]);
        assert_eq!(coords(all, 2), [(0, 0)]);
        assert!(grid.tiles_in(Rect::new(0, 0, 0, 5), 0).is_empty());
        // Nearest the middle of the view first.
        let order = grid.tiles_in(Rect::new(500, 0, 500, 300), 0);
        assert_eq!((order[0].tx, order[0].ty), (2, 0));
    }

    #[test]
    fn lru_evicts_least_recently_used_within_budget() {
        let mut lru: ByteLru<u32, &str> = ByteLru::default();
        lru.insert(1, "a", 40);
        lru.insert(2, "b", 40);
        lru.insert(3, "c", 40);
        assert_eq!((lru.len(), lru.bytes()), (3, 120));
        // Within budget nothing goes.
        assert_eq!(lru.evict(120, lru.mark()), 0);
        // Using 1 makes 2 the oldest.
        assert_eq!(lru.get(&1), Some(&mut "a"));
        assert_eq!(lru.evict(100, lru.mark()), 1);
        assert!(lru.contains(&1) && !lru.contains(&2) && lru.contains(&3));
        assert_eq!(lru.bytes(), 80);
        // peek does not count as use: 3 is still older than 1.
        assert_eq!(lru.peek(&3), Some(&"c"));
        assert_eq!(lru.evict(40, lru.mark()), 1);
        assert!(lru.contains(&1) && !lru.contains(&3));
    }

    #[test]
    fn lru_spares_entries_used_since_the_mark() {
        let mut lru: ByteLru<u32, ()> = ByteLru::default();
        lru.insert(1, (), 50);
        lru.insert(2, (), 50);
        let mark = lru.mark();
        lru.insert(3, (), 50);
        lru.get(&1);
        // Budget 0, but 1 and 3 were used this frame.
        assert_eq!(lru.evict(0, mark), 1);
        assert!(lru.contains(&1) && lru.contains(&3) && !lru.contains(&2));
        assert_eq!(lru.bytes(), 100);
    }

    #[test]
    fn lru_tracks_replacement_and_resize() {
        let mut lru: ByteLru<u32, u8> = ByteLru::default();
        lru.insert(1, 0, 10);
        lru.insert(1, 1, 30);
        assert_eq!((lru.len(), lru.bytes()), (1, 30));
        lru.resize(&1, 70);
        assert_eq!(lru.bytes(), 70);
        assert_eq!(lru.remove(&1), Some(1));
        assert_eq!(lru.bytes(), 0);
        assert!(lru.is_empty());
    }

    /// A source that is not in memory, optionally reports a tile size, and
    /// records the largest region it was asked for.
    #[derive(Debug)]
    struct Lazy {
        meta: ImageMeta,
        tile: Option<(usize, usize)>,
        largest: Arc<Mutex<usize>>,
    }

    fn value(x: usize, y: usize) -> u16 {
        (x * 7 + y * 131) as u16
    }

    impl PixelSource for Lazy {
        fn meta(&self) -> &ImageMeta {
            &self.meta
        }
        fn read_plane(&self, _plane: usize) -> Result<Buffer> {
            panic!("a lazily read image must never be read a whole plane at a time");
        }
        fn read_region(&self, _plane: usize, r: Rect) -> Result<Buffer> {
            let mut largest = self.largest.lock().unwrap();
            *largest = (*largest).max(r.w * r.h);
            let mut v = Vec::with_capacity(r.w * r.h);
            for y in r.y..r.y + r.h {
                v.extend((r.x..r.x + r.w).map(|x| value(x, y)));
            }
            Ok(Buffer::from_vec(v))
        }
        fn tile_size(&self) -> Option<(usize, usize)> {
            self.tile
        }
    }

    fn expected(grid: &TileGrid, key: TileKey) -> Vec<u16> {
        let rect = grid
            .tile_rect(TileCoord {
                level: key.level,
                tx: key.tx,
                ty: key.ty,
            })
            .unwrap();
        let at = |i: usize, size: usize| TileGrid::sample_position(key.level, i, size);
        let mut v = Vec::new();
        for y in rect.y..rect.y + rect.h {
            v.extend(
                (rect.x..rect.x + rect.w).map(|x| value(at(x, grid.width), at(y, grid.height))),
            );
        }
        v
    }

    #[test]
    fn decimated_reads_stay_within_a_band() {
        let (w, h) = (9000, 7000);
        let largest = Arc::new(Mutex::new(0));
        let lazy = Lazy {
            meta: ImageMeta::plane(DType::U16, w, h).unwrap(),
            tile: None,
            largest: largest.clone(),
        };
        let image = Image::new(lazy);
        let grid = TileGrid::new(w, h, 512);
        let key = TileKey {
            plane: 0,
            level: 4,
            tx: 0,
            ty: 0,
        };
        let tile = read_tile(&image, &grid, key).unwrap();
        assert_eq!(tile.as_slice::<u16>().unwrap(), &expected(&grid, key)[..]);
        assert_eq!(TileGrid::sample_position(4, 0, w), 8);
        assert_eq!(TileGrid::sample_position(4, 562, w), 8999);
        // 512 samples 16 apart span 8177 columns; only single rows are read.
        assert_eq!(*largest.lock().unwrap(), 8177);
        // With 64-row source tiles, bands are whole tile rows within budget.
        let largest = Arc::new(Mutex::new(0));
        let tiled = Image::new(Lazy {
            meta: ImageMeta::plane(DType::U16, w, h).unwrap(),
            tile: Some((64, 64)),
            largest: largest.clone(),
        });
        let tile = read_tile(&tiled, &grid, key).unwrap();
        assert_eq!(tile.as_slice::<u16>().unwrap(), &expected(&grid, key)[..]);
        assert!(*largest.lock().unwrap() <= BAND_SAMPLES);
        assert!(read_tile(&image, &grid, TileKey { tx: 2, ..key }).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn tiles_equal_the_decimated_plane(
            w in 1usize..300, h in 1usize..300,
            tile in 1usize..80,
            level in 0u8..4,
            source_tile in prop::option::of((1usize..70, 1usize..70)),
            in_memory in any::<bool>(),
        ) {
            let meta = ImageMeta::plane(DType::U16, w, h).unwrap();
            let image = if in_memory {
                let data = (0..w * h).map(|i| value(i % w, i / w)).collect();
                Image::from_vec(w, h, data).unwrap()
            } else {
                Image::new(Lazy { meta, tile: source_tile, largest: Arc::default() })
            };
            let grid = TileGrid::new(w, h, tile);
            let (lw, lh) = grid.level_size(level);
            let mut seen = 0;
            for ty in 0..lh.div_ceil(tile) as u32 {
                for tx in 0..lw.div_ceil(tile) as u32 {
                    let key = TileKey { plane: 0, level, tx, ty };
                    let got = read_tile(&image, &grid, key).unwrap();
                    let want = expected(&grid, key);
                    prop_assert_eq!(got.as_slice::<u16>().unwrap(), &want[..]);
                    seen += want.len();
                }
            }
            // The tiles of a level cover it exactly once.
            prop_assert_eq!(seen, lw * lh);
        }
    }
}
