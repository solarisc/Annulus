//! Slow work (Argon2id, RSA key generation) on a background thread, so the window keeps
//! drawing; the screen polls for the result each frame.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// Wakes the window from another thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// What background work needs from the window it reports to: a clock, a waker for other
/// threads, and a frame later on. Both UIs provide it.
pub trait Pace {
    /// Seconds on the window's clock.
    fn now(&self) -> f64;
    /// Wakes the window, from any thread.
    fn waker(&self) -> Wake;
    /// Asks for a frame after `after`, with no input.
    fn later(&self, after: Duration);
}

/// Work running on another thread; its result is taken once.
pub struct Job<T>(Receiver<T>);

impl<T: Send + 'static> Job<T> {
    /// Runs `work` on a new thread and wakes the window when it is done.
    pub fn start(pace: &dyn Pace, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (done, result) = channel();
        let wake = pace.waker();
        std::thread::spawn(move || {
            let _ = done.send(work());
            wake();
        });
        Job(result)
    }

    /// The result once there is one; [`Poll::Lost`] if the thread died without one.
    pub fn poll(&self) -> Poll<T> {
        match self.0.try_recv() {
            Ok(value) => Poll::Done(value),
            Err(TryRecvError::Empty) => Poll::Running,
            Err(TryRecvError::Disconnected) => Poll::Lost,
        }
    }
}

/// What [`Job::poll`] found.
pub enum Poll<T> {
    /// Still working.
    Running,
    /// Finished with this.
    Done(T),
    /// The thread ended without a result.
    Lost,
}
