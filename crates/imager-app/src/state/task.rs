//! A command running on a worker thread.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use imager_command::{
    Args, CancelToken, CommandSpec, DatasetId, Executor, Outputs, Progress, Result, RunOptions,
    Session,
};

/// Called from the worker thread when there is something new to show.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct ProgressCell {
    latest: Mutex<Option<(f64, String)>>,
    waker: Option<Waker>,
}

impl Progress for ProgressCell {
    fn report(&self, fraction: f64, message: &str) {
        *self.latest.lock().expect("progress poisoned") = Some((fraction, message.to_string()));
        if let Some(wake) = &self.waker {
            wake();
        }
    }
}

/// What a finished run hands back: its outcome and the session it ran
/// against, which becomes the application's session if it succeeded.
pub(super) struct Finished {
    pub result: Result<Outputs>,
    pub session: Session,
}

pub(super) struct Task {
    pub spec: CommandSpec,
    pub args: Args,
    /// The active image when the run began, to notice the person switching
    /// tabs meanwhile.
    pub base_active: Option<DatasetId>,
    started: Instant,
    cancel: CancelToken,
    progress: Arc<ProgressCell>,
    done: Receiver<Finished>,
}

/// A snapshot of the running command for the progress display.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub title: String,
    /// From 0 to 1, or `None` if the command has not reported yet.
    pub fraction: Option<f64>,
    pub message: String,
    pub elapsed: Duration,
}

impl Running {
    /// Commands that finish quickly never show a progress bar.
    pub const SHOW_AFTER: Duration = Duration::from_millis(500);

    pub fn visible(&self) -> bool {
        self.elapsed >= Running::SHOW_AFTER
    }
}

impl Task {
    /// Starts `spec`'s command against `session`, a copy of the
    /// application's session. Nothing the command does is visible to the
    /// application until the result is taken with [`poll`](Self::poll).
    pub fn spawn(
        executor: Executor,
        session: Session,
        spec: &CommandSpec,
        args: Args,
        waker: Option<Waker>,
    ) -> std::io::Result<Task> {
        let (sender, done) = channel();
        let cancel = CancelToken::new();
        let progress = Arc::new(ProgressCell {
            latest: Mutex::new(None),
            waker: waker.clone(),
        });
        let base_active = session.workspace.active();
        let worker = {
            let (id, args) = (spec.id.clone(), args.clone());
            let (cancel, progress) = (cancel.clone(), progress.clone());
            move || {
                let mut session = session;
                let options = RunOptions {
                    progress: &*progress,
                    cancel,
                };
                let result = executor.run_with(&mut session, &id, &args, &options);
                // The receiver is gone if the run was cancelled.
                let _ = sender.send(Finished { result, session });
                if let Some(wake) = waker {
                    wake();
                }
            }
        };
        std::thread::Builder::new()
            .name(format!("ImageR2 {}", spec.id))
            .spawn(worker)?;
        Ok(Task {
            spec: spec.clone(),
            args,
            base_active,
            started: Instant::now(),
            cancel,
            progress,
            done,
        })
    }

    /// The result if the run has finished. A worker that died without
    /// reporting (a panic in the command) counts as a failure.
    pub fn poll(&self) -> Option<std::result::Result<Finished, String>> {
        match self.done.try_recv() {
            Ok(finished) => Some(Ok(finished)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(self.crashed())),
        }
    }

    /// Blocks until the run has finished.
    pub fn wait(&self) -> std::result::Result<Finished, String> {
        self.done.recv().map_err(|_| self.crashed())
    }

    fn crashed(&self) -> String {
        "the command stopped unexpectedly; this is a bug in the command".to_string()
    }

    /// Asks the command to stop. Its result is discarded either way.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn running(&self) -> Running {
        let latest = self.progress.latest.lock().expect("progress poisoned");
        Running {
            title: self.spec.title.clone(),
            fraction: latest.as_ref().map(|(fraction, _)| *fraction),
            message: latest
                .as_ref()
                .map_or_else(String::new, |(_, message)| message.clone()),
            elapsed: self.started.elapsed(),
        }
    }
}
