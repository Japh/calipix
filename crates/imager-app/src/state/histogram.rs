//! Histograms of displayed planes, computed once and kept.
//!
//! The Histogram and Display panels ask for the same few histograms every
//! frame. Each is computed once per plane of samples and reused until the
//! samples change; changing only how an image is displayed keeps them.
//! Small planes that are in memory are counted on the spot. Anything else
//! is counted on a worker thread, newest request first, so stepping
//! through a stack never waits for a histogram.
//!
//! A plane too large to read whole from its file is not read whole: its
//! histogram is estimated from an even grid of regions (see
//! [`SAMPLE_ABOVE`]) and marked as an estimate.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use imager_command::DatasetId;
use imager_core::{AxisKind, Buffer, ColorModel, Image, Pixel, Rect, dispatch};
use imager_render::{Histogram, HistogramOptions, buffer_histogram, rgb_to_gray};

use super::Waker;

/// Planes of at most this many samples that are in memory are counted
/// without a worker thread: it takes a few milliseconds.
pub const INLINE_BELOW: usize = 1 << 22;

/// A plane of more than this many samples that is not in memory is
/// estimated from a sample rather than read whole.
pub const SAMPLE_ABOVE: usize = 1 << 25;

/// Side of each region read when sampling, and how many regions there are
/// along each axis: 8 x 8 regions of 256 x 256 samples, about four million.
const SAMPLE_REGION: usize = 256;
const SAMPLE_GRID: usize = 8;

/// Histograms kept before the least recently used are dropped.
const CAPACITY: usize = 96;

/// Requests waiting for the worker beyond this many are dropped, oldest
/// first: they are for planes scrolled past long ago.
const BACKLOG: usize = 6;

/// Which samples a histogram counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HistogramKey {
    pub image: DatasetId,
    /// Plane number within the image.
    pub plane: usize,
    /// Count the grey values of an RGB image's three channels, as ImageJ
    /// does, rather than the samples of `plane` alone.
    pub gray: bool,
}

/// A histogram with how it was obtained.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneHistogram {
    pub histogram: Histogram,
    /// For an estimate, the fraction of the plane's samples counted.
    pub sampled: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistogramState {
    /// Being counted; it will be there on a later frame.
    Pending,
    Ready(Arc<PlaneHistogram>),
    /// Why the plane could not be read or counted.
    Failed(String),
}

impl HistogramState {
    pub fn ready(&self) -> Option<&PlaneHistogram> {
        match self {
            HistogramState::Ready(histogram) => Some(histogram),
            _ => None,
        }
    }
}

struct Entry {
    /// The samples version of the image this was counted from.
    version: u64,
    state: HistogramState,
    used: u64,
}

struct Job {
    key: HistogramKey,
    version: u64,
    image: Image,
}

type Outcome = (HistogramKey, u64, Result<PlaneHistogram, String>);

#[derive(Default)]
struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    ready: Condvar,
}

/// The cache. Owned by the application state; see the module docs.
pub struct Histograms {
    entries: HashMap<HistogramKey, Entry>,
    clock: u64,
    queue: Arc<Queue>,
    results: Receiver<Outcome>,
    /// Kept so the worker can be started on first use.
    sender: Sender<Outcome>,
    worker_started: bool,
    waker: Option<Waker>,
    /// How many histograms were counted, for tests of caching.
    computed: u64,
}

impl Default for Histograms {
    fn default() -> Histograms {
        let (sender, results) = channel();
        Histograms {
            entries: HashMap::new(),
            clock: 0,
            queue: Arc::new(Queue::default()),
            results,
            sender,
            worker_started: false,
            waker: None,
            computed: 0,
        }
    }
}

impl Histograms {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// How many histograms have been counted so far.
    pub fn computed(&self) -> u64 {
        self.computed
    }

    /// The histogram for `key`, counted from `image`, whose samples are at
    /// `version`. Starts counting if it is not there yet.
    pub fn get(&mut self, key: HistogramKey, image: &Image, version: u64) -> HistogramState {
        self.collect();
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&key)
            && entry.version == version
        {
            entry.used = self.clock;
            return entry.state.clone();
        }
        let meta = image.meta();
        let state = if image.is_in_memory() && meta.plane_len() <= INLINE_BELOW {
            self.computed += 1;
            match count(image, key) {
                Ok(histogram) => HistogramState::Ready(Arc::new(histogram)),
                Err(problem) => HistogramState::Failed(problem),
            }
        } else {
            self.enqueue(Job {
                key,
                version,
                image: image.clone(),
            });
            HistogramState::Pending
        };
        self.entries.insert(
            key,
            Entry {
                version,
                state: state.clone(),
                used: self.clock,
            },
        );
        self.trim();
        state
    }

    /// Forgets the histograms of images for which `open` is false.
    pub fn retain(&mut self, open: impl Fn(DatasetId) -> bool) {
        self.entries.retain(|key, _| open(key.image));
    }

    /// Blocks until nothing is being counted. For tests.
    pub fn wait(&mut self) {
        while self
            .entries
            .values()
            .any(|entry| entry.state == HistogramState::Pending)
        {
            match self.results.recv() {
                Ok(outcome) => self.store(outcome),
                Err(_) => return,
            }
        }
    }

    fn collect(&mut self) {
        while let Ok(outcome) = self.results.try_recv() {
            self.store(outcome);
        }
    }

    fn store(&mut self, (key, version, result): Outcome) {
        self.computed += 1;
        // A result for samples that have since changed, or for a request
        // that was dropped, is of no use.
        if let Some(entry) = self.entries.get_mut(&key)
            && entry.version == version
        {
            entry.state = match result {
                Ok(histogram) => HistogramState::Ready(Arc::new(histogram)),
                Err(problem) => HistogramState::Failed(problem),
            };
        }
    }

    fn enqueue(&mut self, job: Job) {
        if !self.worker_started {
            self.worker_started = true;
            let (queue, sender, waker) =
                (self.queue.clone(), self.sender.clone(), self.waker.clone());
            let spawned = std::thread::Builder::new()
                .name("ImageR2 histograms".to_string())
                .spawn(move || work(&queue, &sender, waker.as_ref()));
            if spawned.is_err() {
                self.worker_started = false;
            }
        }
        let mut jobs = self.queue.jobs.lock().expect("histogram queue poisoned");
        jobs.retain(|waiting| waiting.key != job.key);
        jobs.push_back(job);
        let mut dropped = Vec::new();
        while jobs.len() > BACKLOG {
            dropped.extend(jobs.pop_front().map(|job| job.key));
        }
        drop(jobs);
        self.queue.ready.notify_one();
        // Dropped requests are asked for again if they are still wanted.
        for key in dropped {
            self.entries.remove(&key);
        }
    }

    fn trim(&mut self) {
        while self.entries.len() > CAPACITY {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.state != HistogramState::Pending)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key);
            match oldest {
                Some(key) => self.entries.remove(&key),
                None => return,
            };
        }
    }
}

/// The worker: counts the newest request first, since that is the plane
/// on screen.
fn work(queue: &Queue, results: &Sender<Outcome>, waker: Option<&Waker>) {
    loop {
        let job = {
            let mut jobs = queue.jobs.lock().expect("histogram queue poisoned");
            loop {
                match jobs.pop_back() {
                    Some(job) => break job,
                    None => jobs = queue.ready.wait(jobs).expect("histogram queue poisoned"),
                }
            }
        };
        let result = count(&job.image, job.key);
        if results.send((job.key, job.version, result)).is_err() {
            // The application has gone.
            return;
        }
        if let Some(wake) = waker {
            wake();
        }
    }
}

/// Counts the histogram `key` asks for, reading no more of a huge plane
/// than a sample of it.
pub fn count(image: &Image, key: HistogramKey) -> Result<PlaneHistogram, String> {
    let meta = image.meta();
    let problem = |e: imager_core::Error| format!("{}: {e}", meta.name);
    let (width, height) = (meta.width(), meta.height());
    let sample = !image.is_in_memory()
        && meta.plane_len() > SAMPLE_ABOVE
        && width >= SAMPLE_REGION
        && height >= SAMPLE_REGION;
    let regions: Vec<Rect> = if sample {
        let at = |index: usize, length: usize| (length - SAMPLE_REGION) * index / (SAMPLE_GRID - 1);
        (0..SAMPLE_GRID * SAMPLE_GRID)
            .map(|i| {
                Rect::new(
                    at(i % SAMPLE_GRID, width),
                    at(i / SAMPLE_GRID, height),
                    SAMPLE_REGION,
                    SAMPLE_REGION,
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    // The samples of one plane: all of them, or the sampled regions.
    let read = |plane: usize| -> Result<Buffer, String> {
        if !sample {
            return image.read_plane(plane).map_err(problem);
        }
        let parts = regions
            .iter()
            .map(|&region| image.read_region(plane, region))
            .collect::<Result<Vec<_>, _>>()
            .map_err(problem)?;
        concat(&parts).ok_or_else(|| format!("{}: its regions differ in type", meta.name))
    };

    let samples = if key.gray && meta.color == ColorModel::Rgb {
        let axis = meta
            .axis_index(&AxisKind::Channel)
            .ok_or_else(|| format!("{}: an RGB image without channels", meta.name))?;
        let mut position = meta.plane_position(key.plane).map_err(problem)?;
        let mut channel = |c: usize| {
            position[axis - 2] = c;
            read(meta.plane_index(&position).map_err(problem)?)
        };
        rgb_to_gray(&channel(0)?, &channel(1)?, &channel(2)?).map_err(problem)?
    } else {
        read(key.plane)?
    };
    let histogram = buffer_histogram(&samples, &HistogramOptions::default()).map_err(problem)?;
    Ok(PlaneHistogram {
        histogram,
        sampled: sample.then(|| samples.len() as f64 / meta.plane_len() as f64),
    })
}

/// Joins buffers of one pixel type end to end.
fn concat(parts: &[Buffer]) -> Option<Buffer> {
    fn join<T: Pixel>(_first: &[T], parts: &[Buffer]) -> Option<Buffer> {
        let mut all = Vec::with_capacity(parts.iter().map(Buffer::len).sum());
        for part in parts {
            all.extend_from_slice(part.as_slice::<T>()?);
        }
        Some(Buffer::from_vec(all))
    }
    let first = parts.first()?;
    dispatch!(first, |samples| join(samples, parts))
}
