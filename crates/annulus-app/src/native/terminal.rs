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
        // Text: one run per color and weight, blanks skipped.
        let mut column = 0;
        while column < cells.len() {
            let cell = cells[column];
            if cell.ch == ' ' || cell.spacer {
                column += 1;
                continue;
            }
            let start = column;
            let mut text = String::new();
            while column < cells.len() {
                let next = cells[column];
                if !text.is_empty() && (!fixed || nerd_symbol(cell.ch) || nerd_symbol(next.ch)) {
                    break;
                }
                if next.spacer {
                    column += 1;
                    continue;
                }
                if fg(&next) != fg(&cell)
                    || next.bold != cell.bold
                    || next.ch == ' ' && column > start
                {
                    // A space ends the run only when what follows changes style: keep words
                    // of one style together, but never draw runs of blanks.
                    if next.ch != ' ' || !same_style_after_blanks(cells, column, &cell) {
                        break;
                    }
                }
                text.push(next.ch);
                column += 1;
            }
            let text = text.trim_end();
            let weight = if cell.bold { Weight::Bold } else { font.weight };
            if !fixed || nerd_symbol(cell.ch) {
                // A fallback icon (or a proportional custom face) must not push the next
                // character off its terminal column. Shape it alone and fit its cell.
                let width = cell_w
                    * if cells.get(start + 1).is_some_and(|next| next.spacer) {
                        2.0
                    } else {
                        1.0
                    };
                let origin = at(row, start);
                draw_cell(
                    ui,
                    Rect::new(origin.x, origin.y, width, cell_h),
                    text,
                    font.weight(weight),
                    fg(&cell),
                );
            } else {
                ui.gfx
                    .draw_text(at(row, start), text, font.weight(weight), fg(&cell));
            }
            if cell.underline {
                underline(ui, at(row, start), text, (cell_w, cell_h), fg(&cell));
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

fn underline(
    ui: &mut Ui<'_>,
    origin: Point,
    text: &str,
    (cell_w, cell_h): (f32, f32),
    color: Color,
) {
    #[allow(clippy::cast_precision_loss)] // A short terminal run.
    let width = text.chars().count() as f32 * cell_w;
    ui.gfx.fill_rect(
        Rect::new(origin.x, origin.y + cell_h - 2.0, width, 1.0),
        0.0,
        color,
    );
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
                if !fixed || nerd_symbol(cell.ch) {
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

/// Fits a fallback symbol or proportional glyph into its terminal cell without moving
/// subsequent text. Wide characters receive a two-cell rectangle from the caller.
fn draw_cell(ui: &mut Ui<'_>, rect: Rect, text: &str, font: Font, color: Color) {
    let measured = ui.gfx.measure(text, font).width;
    let scale = (rect.w / measured.max(0.01)).min(1.0);
    let font = Font {
        size: font.size * scale,
        ..font
    };
    let shown = ui.gfx.measure(text, font);
    ui.push_clip(rect);
    ui.gfx.draw_text(
        Point::new(
            rect.x + (rect.w - shown.width).max(0.0) / 2.0,
            rect.y + (rect.h - shown.height).max(0.0) / 2.0,
        ),
        text,
        font,
        color,
    );
    ui.pop_clip();
}

/// Nerd Font symbols occupy private-use code points, including supplementary-plane icons.
fn nerd_symbol(ch: char) -> bool {
    matches!(u32::from(ch), 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x0010_0000..=0x0010_FFFD)
}

/// Whether the blanks starting at `from` are followed by text in `style`'s color and weight
/// (so the run can carry on through them).
fn same_style_after_blanks(cells: &[Cell], from: usize, style: &Cell) -> bool {
    cells[from..]
        .iter()
        .find(|cell| cell.ch != ' ' && !cell.spacer)
        .is_some_and(|cell| fg(cell) == fg(style) && cell.bold == style.bold)
}

#[cfg(test)]
mod tests {
    use annulus_term::Emulator;

    use super::*;

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
