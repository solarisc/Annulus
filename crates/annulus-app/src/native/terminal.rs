//! A terminal drawn by our renderers: the `alacritty_terminal` screen
//! ([ADR-0016](../../../../docs/adr/0016-alacritty-terminal-core.md)) as background runs,
//! text runs of one color and weight each, and the cursor.

use annulus_term::snapshot::{Cell, Screen};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Color, Font, Point, Rect, Weight};
use annulus_ui::kit::Ui;

/// The size of one cell for `font`. The width is the font's own advance, unrounded, so a
/// run of text drawn at once lands exactly where its cells are; the height is snapped to
/// device pixels so rows never blur.
pub fn cell_size(ui: &mut Ui<'_>, font: Font) -> (f32, f32) {
    let scale = ui.gfx.scale();
    let sample = ui.gfx.measure("MMMMMMMMMM", font);
    let height = (sample.height * 1.08 * scale).round().max(1.0) / scale;
    (sample.width / 10.0, height)
}

/// How many columns and rows of `font` fit in `rect`.
pub fn grid_size(ui: &mut Ui<'_>, rect: Rect, font: Font) -> (usize, usize) {
    let (width, height) = cell_size(ui, font);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small and positive.
    let fit = |space: f32, cell: f32| (space / cell).floor().max(1.0) as usize;
    (fit(rect.w, width), fit(rect.h, height))
}

fn color(rgb: impl Into<[u8; 3]>) -> Color {
    let [r, g, b] = rgb.into();
    Color::rgba(r, g, b, 255)
}

fn fg(cell: &Cell) -> Color {
    color([cell.fg.r, cell.fg.g, cell.fg.b])
}

fn bg(cell: &Cell) -> Color {
    color([cell.bg.r, cell.bg.g, cell.bg.b])
}

/// Draws `screen` into `rect` with `font`, the cursor as a block when `focused` and as an
/// outline otherwise.
pub fn draw(ui: &mut Ui<'_>, rect: Rect, screen: &Screen, font: Font, focused: bool) {
    let background = color([
        screen.background.r,
        screen.background.g,
        screen.background.b,
    ]);
    ui.gfx.fill_rect(rect, 0.0, background);
    let (cell_w, cell_h) = cell_size(ui, font);
    let fixed =
        (ui.gfx.measure("iiii", font).width - ui.gfx.measure("WWWW", font).width).abs() < 0.01;
    ui.push_clip(rect);
    #[allow(clippy::cast_precision_loss)] // Column and row counts are small.
    let at = |row: usize, column: usize| {
        Point::new(
            rect.x + column as f32 * cell_w,
            rect.y + row as f32 * cell_h,
        )
    };
    for (row, cells) in screen.rows.iter().enumerate() {
        // Backgrounds: one rectangle per run of the same color, the default left out.
        let mut column = 0;
        while column < cells.len() {
            let color = bg(&cells[column]);
            let start = column;
            while column < cells.len() && bg(&cells[column]) == color {
                column += 1;
            }
            if color != background {
                let origin = at(row, start);
                #[allow(clippy::cast_precision_loss)] // Short runs.
                let width = (column - start) as f32 * cell_w;
                ui.gfx
                    .fill_rect(Rect::new(origin.x, origin.y, width, cell_h), 0.0, color);
            }
        }
        // Text: runs of plain ASCII in one color and weight, every other glyph in its cell.
        for run in runs(cells, fixed) {
            let cell = cells[run.start];
            let weight = if cell.bold { Weight::Bold } else { font.weight };
            let origin = at(row, run.start);
            if run.alone {
                #[allow(clippy::cast_precision_loss)] // One or two cells.
                let width = cell_w * run.cells as f32;
                draw_cell(
                    ui,
                    Rect::new(origin.x, origin.y, width, cell_h),
                    &run.text,
                    font.weight(weight),
                    fg(&cell),
                );
            } else {
                ui.gfx
                    .draw_text(origin, &run.text, font.weight(weight), fg(&cell));
            }
            if cell.underline {
                #[allow(clippy::cast_precision_loss)] // A short run.
                let width = cell_w * run.cells as f32;
                ui.gfx.fill_rect(
                    Rect::new(origin.x, origin.y + cell_h - 2.0, width, 1.0),
                    0.0,
                    fg(&cell),
                );
            }
        }
    }
    draw_cursor(
        ui,
        screen,
        font,
        focused,
        screen.cursor.map(|(row, column)| at(row, column)),
        (cell_w, cell_h, fixed),
        background,
    );
    ui.pop_clip();
}

fn draw_cursor(
    ui: &mut Ui<'_>,
    screen: &Screen,
    font: Font,
    focused: bool,
    origin: Option<Point>,
    (cell_w, cell_h, fixed): (f32, f32, bool),
    background: Color,
) {
    if let Some((row, column)) = screen.cursor {
        let Some(origin) = origin else {
            return;
        };
        let block = Rect::new(origin.x, origin.y, cell_w, cell_h);
        let cursor = color([
            screen.cursor_color.r,
            screen.cursor_color.g,
            screen.cursor_color.b,
        ]);
        if focused {
            ui.gfx.fill_rect(block, 1.0, cursor);
            if let Some(cell) = screen.rows.get(row).and_then(|cells| cells.get(column))
                && cell.ch != ' '
            {
                if !fixed || !runs_together(cell.ch) {
                    draw_cell(ui, block, &cell.ch.to_string(), font, background);
                } else {
                    ui.gfx
                        .draw_text(origin, &cell.ch.to_string(), font, background);
                }
            }
        } else {
            ui.gfx.stroke_rect(block, 1.0, 1.0, cursor);
        }
    }
}

/// Fits one glyph into its terminal cell (two for a wide character) so it never moves the
/// text after it: a glyph from a fallback font may be wider than the cell, and is then
/// shrunk to fit; a narrower one is centered. One that fits sits on the same line as the
/// runs around it.
fn draw_cell(ui: &mut Ui<'_>, rect: Rect, text: &str, font: Font, color: Color) {
    let full = ui.gfx.measure(text, font);
    if full.width <= rect.w + 0.01 {
        ui.gfx.draw_text(
            Point::new(rect.x + (rect.w - full.width) / 2.0, rect.y),
            text,
            font,
            color,
        );
        return;
    }
    let font = Font {
        size: font.size * rect.w / full.width,
        ..font
    };
    let shown = ui.gfx.measure(text, font);
    ui.push_clip(rect);
    ui.gfx.draw_text(
        Point::new(
            rect.x + (rect.w - shown.width).max(0.0) / 2.0,
            rect.y + (full.height - shown.height).max(0.0) / 2.0,
        ),
        text,
        font,
        color,
    );
    ui.pop_clip();
}

/// Text drawn at once from cell `start` on, over `cells` columns.
#[derive(Debug, PartialEq)]
struct Run {
    start: usize,
    cells: usize,
    text: String,
    /// One glyph fitted to its cells rather than a run trusted to advance one cell a glyph.
    alone: bool,
}

/// Whether `ch` may share a run: plain ASCII, which every terminal font has, so each glyph
/// advances exactly one cell. Anything else (box drawing, blocks, braille, arrows, wide
/// characters, icons) may come from a fallback font with another advance, and a run of
/// those would drift off the grid: btop's braille graphs spilled over their borders.
fn runs_together(ch: char) -> bool {
    ch.is_ascii_graphic() || ch == ' '
}

/// How a row of `cells` is drawn: runs of ASCII in one color and weight (through blanks
/// only when the same style follows), every other glyph alone in its cells, blanks
/// skipped. A proportional face (`fixed` false) draws every glyph alone.
fn runs(cells: &[Cell], fixed: bool) -> Vec<Run> {
    let mut out = Vec::new();
    let mut column = 0;
    while column < cells.len() {
        let cell = cells[column];
        if cell.ch == ' ' || cell.spacer {
            column += 1;
            continue;
        }
        let start = column;
        if !fixed || !runs_together(cell.ch) {
            let wide = cells.get(start + 1).is_some_and(|next| next.spacer);
            let cells = if wide { 2 } else { 1 };
            out.push(Run {
                start,
                cells,
                text: cell.ch.to_string(),
                alone: true,
            });
            column += cells;
            continue;
        }
        let mut text = String::new();
        while column < cells.len() {
            let next = cells[column];
            if next.spacer || !runs_together(next.ch) {
                break;
            }
            if fg(&next) != fg(&cell) || next.bold != cell.bold || next.underline != cell.underline
            {
                break;
            }
            // A space carries the run on only into more text of its style: keep words of
            // one style together, but never draw runs of blanks.
            if next.ch == ' ' && !same_style_after_blanks(cells, column, &cell) {
                break;
            }
            text.push(next.ch);
            column += 1;
        }
        let text = text.trim_end().to_owned();
        out.push(Run {
            start,
            cells: text.chars().count(),
            text,
            alone: false,
        });
    }
    out
}

/// Whether the blanks starting at `from` are followed by text that runs on in `style`'s
/// color, weight and underline (so the run can carry on through them).
fn same_style_after_blanks(cells: &[Cell], from: usize, style: &Cell) -> bool {
    cells[from..]
        .iter()
        .find(|cell| cell.ch != ' ' && !cell.spacer)
        .is_some_and(|cell| {
            runs_together(cell.ch)
                && fg(cell) == fg(style)
                && cell.bold == style.bold
                && cell.underline == style.underline
        })
}

#[cfg(test)]
mod tests {
    use annulus_term::Emulator;

    use super::*;

    /// A canvas whose fonts measure as Direct2D's do with the default face: ASCII 0.6 em, and
    /// braille, from a fallback font, 0.75 em. It keeps where each text was drawn and how wide.
    #[derive(Default)]
    struct Fallback {
        texts: Vec<(f32, f32, String)>,
    }

    impl annulus_ui::gfx::Canvas for Fallback {
        fn scale(&self) -> f32 {
            1.0
        }
        fn fill_rect(&mut self, _: Rect, _: f32, _: Color) {}
        fn stroke_rect(&mut self, _: Rect, _: f32, _: f32, _: Color) {}
        fn shadow(&mut self, _: Rect, _: f32, _: f32, _: f32, _: Color) {}
        fn fill_circle(&mut self, _: Point, _: f32, _: Color) {}
        fn polyline(&mut self, _: &[Point], _: bool, _: f32, _: Color) {}
        fn stroke_circle(&mut self, _: Point, _: f32, _: f32, _: Color) {}
        fn fill_polygon(&mut self, _: &[Point], _: Color) {}
        fn measure(&mut self, text: &str, font: Font) -> annulus_ui::gfx::TextSize {
            let em: f32 = text
                .chars()
                .map(|ch| if ch.is_ascii() { 0.6 } else { 0.75 })
                .sum();
            annulus_ui::gfx::TextSize {
                width: em * font.size,
                height: font.size * 1.2,
            }
        }
        fn draw_text(&mut self, origin: Point, text: &str, font: Font, _: Color) {
            let width = self.measure(text, font).width;
            self.texts.push((origin.x, width, text.to_owned()));
        }
        fn push_clip(&mut self, _: Rect) {}
        fn pop_clip(&mut self) {}
    }

    #[test]
    fn glyphs_wider_than_a_cell_never_push_text_off_its_column() {
        // A btop meter row: braille bars, then the border and the percentage after them.
        let mut emulator = Emulator::new(40, 2);
        emulator
            .advance("Used:\u{28ff}\u{28ff}\u{28ff}\u{28ff}\u{28ff}\u{28ff}\u{2502}45%".as_bytes());
        let screen = emulator.screen();
        let mut canvas = Fallback::default();
        let mut state = annulus_ui::kit::UiState::default();
        state.begin_frame(&[], 0.0);
        let area = Rect::new(0.0, 0.0, 400.0, 40.0);
        let font = Font::mono(10.0);
        let mut ui = Ui::new(
            &mut canvas,
            annulus_ui::kit::Theme::new(annulus_config::tokens::Appearance::Dark),
            &mut state,
            area,
        );
        draw(&mut ui, area, &screen, font, false);
        let (cell_w, _) = cell_size(&mut ui, font);
        drop(ui);
        let column_of = |x: f32| ((x + 0.01) / cell_w).floor();
        for (x, width, text) in &canvas.texts {
            let start = column_of(*x);
            #[allow(clippy::cast_precision_loss)] // A short test row.
            let end = (start + text.chars().count() as f32) * cell_w;
            assert!(
                x + width <= end + 0.01,
                "{text:?} at column {start} runs past its cells: {} > {end}",
                x + width
            );
        }
        let percent = canvas.texts.iter().find(|(.., text)| text == "45%");
        assert_eq!(percent.map(|&(x, ..)| column_of(x)), Some(12.0));
    }

    fn row(columns: usize, text: &str) -> Vec<Cell> {
        let mut emulator = Emulator::new(columns, 1);
        emulator.advance(text.as_bytes());
        emulator.screen().rows[0].clone()
    }

    fn plan(cells: &[Cell], fixed: bool) -> Vec<(usize, usize, String, bool)> {
        runs(cells, fixed)
            .into_iter()
            .map(|run| (run.start, run.cells, run.text, run.alone))
            .collect()
    }

    #[test]
    fn only_ascii_shares_a_run_and_every_other_glyph_keeps_its_own_column() {
        // A btop meter: a label, braille graph cells, box drawing, a percentage, an arrow.
        let cells = row(20, "Used:\u{28ff}\u{28c0}\u{2502}45%\u{25b2}");
        let alone = |start: usize, ch: char| (start, 1, ch.to_string(), true);
        assert_eq!(
            plan(&cells, true),
            [
                (0, 5, "Used:".to_owned(), false),
                alone(5, '\u{28ff}'),
                alone(6, '\u{28c0}'),
                alone(7, '\u{2502}'),
                (8, 3, "45%".to_owned(), false),
                alone(11, '\u{25b2}'),
            ]
        );
    }

    #[test]
    fn a_wide_character_gets_both_its_cells_and_the_text_after_it_keeps_its_column() {
        let cells = row(10, "a\u{4e2d}b");
        assert_eq!(
            plan(&cells, true),
            [
                (0, 1, "a".to_owned(), false),
                (1, 2, "\u{4e2d}".to_owned(), true),
                (3, 1, "b".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_proportional_face_draws_every_glyph_alone() {
        let drawn = plan(&row(10, "ab c"), false);
        assert_eq!(drawn.len(), 3);
        assert!(
            drawn
                .iter()
                .all(|&(_, cells, _, alone)| alone && cells == 1)
        );
    }

    #[test]
    fn a_run_ends_where_the_style_does() {
        let cells = row(20, "ab cd\x1b[1mef\x1b[0m \x1b[4mgh\x1b[0m");
        let texts: Vec<String> = plan(&cells, true).into_iter().map(|run| run.2).collect();
        assert_eq!(texts, ["ab cd", "ef", "gh"]);
    }

    #[test]
    fn runs_continue_through_spaces_only_into_the_same_style() {
        let mut emulator = Emulator::new(20, 2);
        emulator.advance(b"ab  cd\x1b[31m ef\x1b[0m");
        let screen = emulator.screen();
        let row = &screen.rows[0];
        assert!(same_style_after_blanks(row, 2, &row[0]));
        // After "cd" come a red space and red "ef": a new run.
        assert!(!same_style_after_blanks(row, 6, &row[4]));
    }
}
