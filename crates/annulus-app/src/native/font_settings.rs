//! Font preferences and asynchronous discovery/import. Font files never load while drawing.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use annulus_config::settings::Settings;
use annulus_platform::fonts;
use annulus_ui::gfx::{Align, Canvas as _, Font, Rect};
use annulus_ui::kit::{Id, Ui};
use annulus_ui::widgets::{CONTROL, FieldStyle, Kind, SMALL};

use super::pages;

enum Loaded {
    Ready(String),
    Imported {
        path: PathBuf,
        name: String,
        app: bool,
    },
}

/// The shared font import form and its worker.
#[derive(Default)]
pub struct FontSettings {
    path: String,
    status: String,
    receiver: Option<Receiver<Result<Loaded, String>>>,
    initialized: bool,
    discover: bool,
    discovered: bool,
    selected: (Option<String>, Option<String>),
    directory: PathBuf,
}

impl FontSettings {
    /// Polls workers and loads saved/changed selections. Returns whether an import changed
    /// settings. Bundled defaults do not start a worker until the font controls are opened.
    #[allow(clippy::too_many_lines)] // Polling and starting this one worker lifecycle.
    pub fn frame(&mut self, ui: &mut Ui<'_>, settings: &mut Settings, directory: PathBuf) -> bool {
        self.directory = directory;
        let mut changed = false;
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(Loaded::Ready(status)) => self.status = status,
                        Ok(Loaded::Imported { path, name, app }) => {
                            if !settings.font_files.contains(&path) {
                                settings.font_files.push(path);
                            }
                            if app {
                                settings.ui_font = Some(name.clone());
                            } else {
                                settings.terminal_font = Some(name.clone());
                            }
                            self.status =
                                format!("Imported {name}. Stored privately; no OS installation.");
                            self.path.clear();
                            changed = true;
                        }
                        Err(error) => self.status = error,
                    }
                    self.receiver = None;
                    ui.request_repaint();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "The font worker stopped. Try importing again.".into();
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let selected = (settings.ui_font.clone(), settings.terminal_font.clone());
        let startup = !self.initialized;
        let discover = self.discover && !self.discovered;
        if self.receiver.is_none() && (startup || discover || selected != self.selected) {
            self.initialized = true;
            self.selected = selected.clone();
            self.discovered |= discover;
            let files = if startup {
                settings.font_files.iter().take(32).cloned().collect()
            } else {
                Vec::new()
            };
            if discover || !files.is_empty() || selected.0.is_some() || selected.1.is_some() {
                let (sender, receiver) = mpsc::channel();
                self.receiver = Some(receiver);
                std::thread::spawn(move || {
                    if discover {
                        fonts::discover();
                    }
                    let mut errors = Vec::new();
                    for path in files {
                        if let Err(error) = fonts::load(&path) {
                            errors.push(error);
                        }
                    }
                    if let Err(error) =
                        fonts::load_selected(selected.0.as_deref(), selected.1.as_deref())
                    {
                        errors.push(error);
                    }
                    let _ = sender.send(Ok(Loaded::Ready(errors.join(" "))));
                });
            }
        }
        if self.receiver.is_some() {
            ui.request_repaint_after(std::time::Duration::from_millis(50));
        }
        changed
    }

    /// The family picker used by the app card and the terminal's Text card.
    pub fn family(
        &mut self,
        ui: &mut Ui<'_>,
        row: Rect,
        settings: &mut Settings,
        app: bool,
    ) -> bool {
        if !self.discover {
            self.discover = true;
            ui.request_repaint();
        }
        let prefix = if app { "font.app" } else { "font.terminal" };
        let default = if app {
            "System default"
        } else {
            "JetBrains Mono (default)"
        };
        let names: Vec<String> = std::iter::once(default.to_owned())
            .chain(fonts::families())
            .collect();
        let preference = if app {
            &mut settings.ui_font
        } else {
            &mut settings.terminal_font
        };
        let at = preference
            .as_ref()
            .and_then(|name| names.iter().position(|known| known == name))
            .or(Some(0));
        let mut changed = false;
        if let Some(at) = ui.select(Id::new((prefix, "family")), row, &names, at, default) {
            *preference = (at > 0).then(|| names[at].clone());
            changed = true;
        }
        changed
    }

    /// Family selection, a private import and a live glyph sample for the app or terminal.
    #[allow(clippy::too_many_lines)] // The import form and its sample, top to bottom.
    pub fn draw(
        &mut self,
        ui: &mut Ui<'_>,
        page: &mut Rect,
        settings: &mut Settings,
        app: bool,
    ) -> bool {
        if !self.discover {
            self.discover = true;
            ui.request_repaint();
        }
        let t = ui.theme;
        let mut card = pages::card(
            ui,
            page,
            if app { "App font" } else { "Custom fonts" },
            if app { 218.0 } else { 164.0 },
        );
        let drop_zone = card;
        let prefix = if app { "font.app" } else { "font.terminal" };
        let mut changed = false;
        if app {
            let row = pages::setting(
                ui,
                card.take_top(54.0),
                "Family",
                "Installed, bundled or imported.",
                280.0,
            );
            changed = self.family(ui, row, settings, app);
        }
        card.take_top(6.0);
        ui.gfx.text(
            card.take_top(20.0),
            "Import a font: paste its path or drop a .ttf, .otf or .ttc file here.",
            SMALL,
            t.text2,
            Align::Start,
        );
        let mut row = card.take_top(CONTROL);
        let import = row.take_right(96.0);
        row.take_right(8.0);
        let submitted = ui
            .text_field(
                Id::new((prefix, "file")),
                row,
                &mut self.path,
                FieldStyle {
                    placeholder: "Path to font file",
                    ..FieldStyle::default()
                },
            )
            .submitted;
        let clicked = ui
            .button(
                Id::new((prefix, "import")),
                import,
                if self.receiver.is_some() {
                    "Loading…"
                } else {
                    "Import"
                },
                None,
                Kind::Secondary,
            )
            .clicked;
        if ui.hover_within(drop_zone) {
            let dropped = ui.take_dropped_files();
            if let Some(path) = dropped.first() {
                self.path = path.to_string_lossy().into_owned();
                self.import(app);
            }
        }
        if (clicked || submitted) && self.receiver.is_none() {
            self.import(app);
        }
        card.take_top(10.0);
        let sample = if app {
            "The quick brown fox · 0123456789"
        } else {
            "   ~/projects  󰊢 main  "
        };
        ui.gfx.text(
            card.take_top(26.0),
            sample,
            if app {
                Font::ui(14.0)
            } else {
                Font::mono(14.0)
            },
            t.text,
            Align::Start,
        );
        let status = if self.status.is_empty() {
            "Bundled: JetBrains Mono, Fira Mono and OFL Nerd Font symbols."
        } else {
            &self.status
        };
        ui.gfx
            .text(card.take_top(26.0), status, SMALL, t.text2, Align::Start);
        if self.receiver.is_some() {
            ui.request_repaint_after(std::time::Duration::from_millis(50));
        }
        changed
    }

    fn import(&mut self, app: bool) {
        if self.receiver.is_some() {
            return;
        }
        let source = PathBuf::from(self.path.trim().trim_matches('"'));
        if self.path.trim().is_empty() {
            self.status = "Paste or drop a font file first.".into();
            return;
        }
        let directory = self.directory.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = fonts::import(&source, &directory).and_then(|(path, names)| {
                names
                    .first()
                    .cloned()
                    .map(|name| Loaded::Imported { path, name, app })
                    .ok_or_else(|| "No font family was found.".into())
            });
            let _ = sender.send(result);
        });
    }
}
