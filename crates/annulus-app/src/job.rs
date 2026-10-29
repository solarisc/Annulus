//! Slow work (Argon2id, RSA key generation) on a background thread, so the window keeps
//! drawing; the screen polls for the result each frame.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

/// Work running on another thread; its result is taken once.
pub struct Job<T>(Receiver<T>);

impl<T: Send + 'static> Job<T> {
    /// Runs `work` on a new thread and wakes `ctx` when it is done.
    pub fn start(ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (done, result) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = done.send(work());
            ctx.request_repaint();
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
