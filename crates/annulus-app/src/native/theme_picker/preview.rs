//! A small server list and connection sample in the highlighted theme.

use super::{ThemePicker, colors};
use annulus_config::{
    schemes::{BUILT_INS, SCHEMES},
    settings::Settings,
};
use annulus_ui::gfx::{Align, Canvas as _, Font, Rect};
use annulus_ui::kit::Ui;
use annulus_ui::widgets::{BODY_MEDIUM, HEADING, SMALL};

impl ThemePicker {
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
        let hosts = if inner.h < 340.0 { 1 } else { 2 };
        for (label, detail) in [
            ("Production", "deploy@web-1  ·  Connected"),
            ("Development", "dev@localhost  ·  Ready"),
        ]
        .into_iter()
        .take(hosts)
        {
            let card = inner.take_top(62.0);
            ui.gfx.fill_rect(card, 7.0, t.surface);
            let mut lines = card.inset(10.0, 8.0);
            ui.gfx.text(
                lines.take_top(22.0),
                label,
                BODY_MEDIUM,
                t.text,
                Align::Start,
            );
            ui.gfx.text(lines, detail, SMALL, t.text2, Align::Start);
            inner.take_top(8.0);
        }
        let button = inner.take_top(32.0);
        ui.gfx.fill_rect(button, 6.0, t.accent);
        ui.gfx
            .text(button, "Connect", BODY_MEDIUM, t.on_accent, Align::Center);
        inner.take_top(14.0);
        for (text, ink) in [
            ("$ ssh deploy@web-1", t.text),
            ("Connected", t.ok),
            ("Updates available", t.warn),
            ("Connection refused", t.error),
        ] {
            let mut line = inner.take_top(22.0);
            let dot = line.take_left(14.0).centered(6.0, 6.0);
            ui.gfx.fill_rect(dot, 3.0, ink);
            ui.gfx
                .text(line, text, Font::mono(11.0), t.text, Align::Start);
        }
        ui.pop_clip();
    }
}
