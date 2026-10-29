//! A session: a program (a local shell, or for now `ssh`) running in a PTY, feeding a
//! terminal (ADR-0016). `alacritty_terminal`'s event loop reads the PTY on its own thread;
//! the UI thread writes input, resizes, and takes snapshots under the terminal lock.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use annulus_term::alacritty_terminal::event::{Event, EventListener, WindowSize};
use annulus_term::alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use annulus_term::alacritty_terminal::grid::Scroll;
use annulus_term::alacritty_terminal::sync::FairMutex;
use annulus_term::alacritty_terminal::term::{Term, TermMode};
use annulus_term::alacritty_terminal::tty;
use annulus_term::select::{self, SelectionKind};
use annulus_term::{Palette, Screen, Size, config, snapshot};

/// What a session tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// The program set the window title.
    Title(String),
    /// The program rang the bell.
    Bell,
    /// The program exited; the tab shows its last output.
    Exited,
}

/// The program a session runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Program {
    /// The user's default shell (`$SHELL`, or PowerShell/cmd on Windows).
    DefaultShell,
    /// A specific program with arguments, e.g. `ssh user@host -p 22`.
    Command {
        /// The program to run, found on `PATH`.
        program: String,
        /// Its arguments.
        args: Vec<String>,
    },
}

impl Program {
    /// `ssh` to `target` (`user@host`, `host`, `user@host:port`), using the system OpenSSH
    /// client until Annulus's own SSH stack lands.
    #[must_use]
    pub fn ssh(target: &str) -> Self {
        let (destination, port) = split_port(target.trim());
        let mut args = vec![destination.to_owned()];
        if let Some(port) = port {
            args.push("-p".into());
            args.push(port.to_owned());
        }
        Program::Command {
            program: "ssh".into(),
            args,
        }
    }
}

/// `host:port` splits into host and port; anything else (including IPv6 literals without
/// brackets) is a destination with no port.
fn split_port(target: &str) -> (&str, Option<&str>) {
    if let Some(rest) = target.strip_prefix('[')
        && let Some((host, port)) = rest.split_once("]:")
    {
        return (
            host,
            Some(port).filter(|port| port.chars().all(|c| c.is_ascii_digit())),
        );
    }
    match target.rsplit_once(':') {
        Some((host, port))
            if !host.contains(':')
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            (host, Some(port))
        }
        _ => (target, None),
    }
}

/// Routes the terminal's events: replies to the PTY, the rest to the UI, and a wake-up so
/// the UI repaints only when there is something new.
#[derive(Clone)]
struct Listener {
    events: Sender<SessionEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    pty: Arc<Mutex<Option<EventLoopSender>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
            Event::Title(title) => {
                let _ = self.events.send(SessionEvent::Title(title));
            }
            Event::Bell => {
                let _ = self.events.send(SessionEvent::Bell);
            }
            Event::Exit | Event::ChildExit(_) => {
                let _ = self.events.send(SessionEvent::Exited);
            }
            // Device status and color queries: the terminal's reply goes back to the program.
            Event::PtyWrite(reply) => {
                if let Ok(pty) = self.pty.lock()
                    && let Some(pty) = pty.as_ref()
                {
                    let _ = pty.send(Msg::Input(Cow::Owned(reply.into_bytes())));
                }
            }
            // OSC 52 clipboard and other requests are ignored until F-TERM settings gate them.
            _ => {}
        }
        (self.wake)();
    }
}

/// A running session.
pub struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    events: Receiver<SessionEvent>,
    palette: Palette,
}

impl Session {
    /// Starts `program` in a PTY of `size`, with cells of `cell_width` by `cell_height`
    /// pixels. `wake` is called (from the PTY thread) whenever there is something to redraw.
    ///
    /// # Errors
    /// Any OS error creating the PTY or starting the program.
    pub fn spawn(
        program: &Program,
        size: Size,
        cell_width: u16,
        cell_height: u16,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let (events_tx, events) = channel();
        let listener = Listener {
            events: events_tx,
            wake: Arc::new(wake),
            pty: Arc::new(Mutex::new(None)),
        };
        let term = Arc::new(FairMutex::new(Term::new(config(), &size, listener.clone())));
        let shell = match program {
            Program::DefaultShell => None,
            Program::Command { program, args } => {
                Some(tty::Shell::new(program.clone(), args.clone()))
            }
        };
        let options = tty::Options {
            shell,
            drain_on_exit: true,
            ..tty::Options::default()
        };
        let pty = tty::new(&options, window_size(size, cell_width, cell_height), 0)?;
        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
        let sender = event_loop.channel();
        if let Ok(mut slot) = listener.pty.lock() {
            *slot = Some(event_loop.channel());
        }
        event_loop.spawn();
        Ok(Self {
            term,
            pty: sender,
            events,
            palette: Palette::annulus_dark(),
        })
    }

    /// Sends input (typed text, encoded keys, pasted text) to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.pty.send(Msg::Input(Cow::Owned(bytes.into())));
    }

    /// Resizes the terminal and the PTY.
    pub fn resize(&self, size: Size, cell_width: u16, cell_height: u16) {
        self.term.lock().resize(size);
        let _ = self
            .pty
            .send(Msg::Resize(window_size(size, cell_width, cell_height)));
    }

    /// Scrolls the view through the scrollback by `lines` (positive scrolls back in time).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Jumps the view back to the newest output, as typing does.
    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// What the terminal shows now.
    #[must_use]
    pub fn screen(&self) -> Screen {
        snapshot(&self.term.lock(), &self.palette)
    }

    /// Starts a mouse selection at screen cell (`row`, `column`).
    pub fn select_start(&self, row: usize, column: usize, kind: SelectionKind) {
        select::start(&mut self.term.lock(), row, column, kind);
    }

    /// Extends the selection to screen cell (`row`, `column`).
    pub fn select_to(&self, row: usize, column: usize) {
        select::extend(&mut self.term.lock(), row, column);
    }

    /// The selected text, if any.
    #[must_use]
    pub fn selection_text(&self) -> Option<String> {
        select::text(&self.term.lock())
    }

    /// Removes the selection.
    pub fn clear_selection(&self) {
        select::clear(&mut self.term.lock());
    }

    /// The terminal's input modes (application cursor keys, bracketed paste).
    #[must_use]
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Events since the last call.
    pub fn take_events(&self) -> Vec<SessionEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pty.send(Msg::Shutdown);
    }
}

fn window_size(size: Size, cell_width: u16, cell_height: u16) -> WindowSize {
    let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    WindowSize {
        num_lines: clamp(size.rows),
        num_cols: clamp(size.columns),
        cell_width,
        cell_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(program: &Program) -> Vec<String> {
        match program {
            Program::Command { args, .. } => args.clone(),
            Program::DefaultShell => Vec::new(),
        }
    }

    #[test]
    fn quick_connect_targets_become_ssh_arguments() {
        assert_eq!(args(&Program::ssh("admin@prod-db")), ["admin@prod-db"]);
        assert_eq!(
            args(&Program::ssh(" admin@10.0.4.12:2222 ")),
            ["admin@10.0.4.12", "-p", "2222"]
        );
        assert_eq!(args(&Program::ssh("[fe80::1]:22")), ["fe80::1", "-p", "22"]);
        assert_eq!(args(&Program::ssh("fe80::1")), ["fe80::1"]);
        assert_eq!(args(&Program::ssh("host:notaport")), ["host:notaport"]);
    }

    #[test]
    fn a_shell_runs_and_its_output_reaches_the_screen() {
        let program = if cfg!(windows) {
            Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/C".into(), "echo annulus-ok".into()],
            }
        } else {
            Program::Command {
                program: "sh".into(),
                args: vec!["-c".into(), "echo annulus-ok".into()],
            }
        };
        let session = Session::spawn(&program, Size::new(80, 24), 8, 16, || {}).unwrap();
        let mut seen = false;
        // Waits on real process output; bounded so a broken PTY fails instead of hanging.
        for _ in 0..200 {
            let screen = session.screen();
            if screen.rows.iter().any(|row| {
                row.iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .contains("annulus-ok")
            }) {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(seen, "the program's output never appeared");
    }
}
