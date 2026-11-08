//! An app sample and a real ANSI terminal in the highlighted theme.

use super::{ThemePicker, colors};
use annulus_config::{
    schemes::{BUILT_INS, SCHEMES},
    settings::Settings,
};
use annulus_ui::gfx::{Align, Canvas as _, Color, Font, Rect};
use annulus_ui::kit::Ui;
use annulus_ui::widgets::{BODY_MEDIUM, HEADING, SMALL};

impl ThemePicker {
    #[allow(clippy::too_many_lines)] // The app sample and its terminal pane share one layout.
    pub(super) fn preview(
        &mut self,
        ui: &mut Ui<'_>,
        mut rect: Rect,
        settings: &Settings,
        system: annulus_ui::kit::Theme,
    ) {
        let name = &colors::app_schemes()[self.selected];
        ui.gfx.text(
            rect.take_top(26.0),
            name,
            HEADING,
            ui.theme.text,
            Align::Start,
        );
        let credit = self
            .selected
            .checked_sub(1)
            .and_then(|i| SCHEMES.iter().chain(BUILT_INS).nth(i))
            .map_or(
                if self.selected == 0 {
                    "Built in · follows your system"
                } else {
                    "Built in · Annulus"
                },
                |scheme| scheme.author,
            );
        ui.gfx.text(
            rect.take_top(22.0),
            credit,
            SMALL,
            ui.theme.text2,
            Align::Start,
        );
        rect.take_top(12.0);
        let target = colors::previewed_theme(settings, Some(self.selected), system);
        let (t, moving) = self.preview.step(target, ui.time(), ui.state.reduce_motion);
        ui.request_repaint_if(moving);
        ui.gfx.fill_rect(rect, 10.0, t.chrome);
        ui.gfx.stroke_rect(rect, 10.0, 1.0, t.line);
        ui.push_clip(rect);
        let mut inner = rect.inset(14.0, 14.0);
        ui.gfx.text(
            inner.take_top(28.0),
            "Annulus  /  My servers",
            BODY_MEDIUM,
            t.text,
            Align::Start,
        );
        inner.take_top(8.0);
        let card = inner.take_top(62.0);
        ui.gfx.fill_rect(card, 7.0, t.surface);
        let mut lines = card.inset(10.0, 8.0);
        ui.gfx.text(
            lines.take_top(22.0),
            "Production",
            BODY_MEDIUM,
            t.text,
            Align::Start,
        );
        ui.gfx.text(
            lines,
            "deploy@web-1  ·  Connected",
            SMALL,
            t.text2,
            Align::Start,
        );
        inner.take_top(8.0);
        let button = inner.take_top(32.0);
        ui.gfx.fill_rect(button, 6.0, t.accent);
        ui.gfx
            .text(button, "Connect", BODY_MEDIUM, t.on_accent, Align::Center);
        inner.take_top(14.0);
        let terminal_target = if name == colors::OWN_COLORS
            || name == colors::OWN_DARK
            || name == colors::OWN_LIGHT
        {
            if target.dark {
                annulus_term::Palette::annulus_dark()
            } else {
                annulus_term::Palette::annulus_light()
            }
        } else {
            colors::terminal_palette(name)
        };
        let (palette, moving) =
            self.terminal_fade
                .step(terminal_target, ui.time(), ui.state.reduce_motion);
        ui.request_repaint_if(moving);
        self.terminal_preview(ui, inner, &palette, t.text2);
        ui.pop_clip();
    }
    fn terminal_preview(
        &mut self,
        ui: &mut Ui<'_>,
        mut rect: Rect,
        palette: &annulus_term::Palette,
        ink: Color,
    ) {
        ui.gfx.text(
            rect.take_top(22.0),
            "Terminal preview",
            BODY_MEDIUM,
            ink,
            Align::Start,
        );
        let shape = rect;
        ui.gfx.fill_rect(shape, 7.0, color(palette.background));
        ui.push_clip(shape);
        let mut inner = rect.inset(8.0, 8.0);
        let mut swatches = inner.take_bottom(12.0);
        let width = swatches.w / 16.0;
        for ansi in palette.ansi {
            ui.gfx
                .fill_rect(swatches.take_left(width).inset(1.0, 0.0), 2.0, color(ansi));
        }
        inner.take_bottom(6.0);
        let font = Font::mono(11.0);
        let (columns, rows) = super::super::terminal::grid_size(ui, inner, font);
        let terminal = self.terminal.get_or_insert_with(|| {
            let mut terminal = annulus_term::Emulator::new(columns, rows.max(4));
            terminal.advance("\x1b[32mdev@web-1\x1b[0m \x1b[34m~/app\x1b[0m \x1b[35m\u{e0a0} main\x1b[0m\r\n$ cargo test\r\n\x1b[32mPASS\x1b[0m  12 tests passed\r\n\x1b[31merror:\x1b[0m example message".as_bytes());
            terminal
        });
        terminal.resize(columns, rows.max(4));
        terminal.set_palette(palette);
        super::super::terminal::draw(ui, inner, &terminal.screen(), font, false);
        ui.pop_clip();
    }
}

fn color(rgb: annulus_term::alacritty_terminal::vte::ansi::Rgb) -> Color {
    Color::rgba(rgb.r, rgb.g, rgb.b, 255)
}

#[derive(Default)]
pub(super) struct TerminalFade {
    way: Option<(annulus_term::Palette, annulus_term::Palette, f64)>,
}

impl TerminalFade {
    fn at(&self, now: f64) -> Option<(annulus_term::Palette, bool)> {
        let (from, to, since) = self.way.as_ref()?;
        #[allow(clippy::cast_possible_truncation)] // A normalized animation share.
        let share = ((now - since) / 0.22).clamp(0.0, 1.0) as f32;
        if from == to || share >= 1.0 {
            return Some((to.clone(), false));
        }
        let mix = |a, b| {
            let c = color(a).mix(color(b), annulus_ui::motion::Motion::BASE.curve.at(share));
            annulus_term::alacritty_terminal::vte::ansi::Rgb {
                r: c.r,
                g: c.g,
                b: c.b,
            }
        };
        Some((
            annulus_term::Palette {
                ansi: std::array::from_fn(|i| mix(from.ansi[i], to.ansi[i])),
                foreground: mix(from.foreground, to.foreground),
                background: mix(from.background, to.background),
                cursor: mix(from.cursor, to.cursor),
                selection: mix(from.selection, to.selection),
            },
            true,
        ))
    }

    fn step(
        &mut self,
        target: annulus_term::Palette,
        now: f64,
        instant: bool,
    ) -> (annulus_term::Palette, bool) {
        if instant {
            self.way = Some((target.clone(), target.clone(), now));
            return (target, false);
        }
        if self.way.as_ref().is_none_or(|(_, to, _)| *to != target) {
            let from = self
                .at(now)
                .map_or_else(|| target.clone(), |(shown, _)| shown);
            self.way = Some((from, target.clone(), now));
        }
        self.at(now).unwrap_or((target, false))
    }
}
