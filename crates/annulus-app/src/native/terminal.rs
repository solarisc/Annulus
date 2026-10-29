//! A terminal drawn by our renderers: the `alacritty_terminal` screen
//! ([ADR-0016](../../../../docs/adr/0016-alacritty-terminal-core.md)) as background runs,
//! text runs of one color and weight each, and the cursor.

use annulus_term::snapshot::{Cell, Screen};
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
            ui.gfx
                .draw_text(at(row, start), text, font.weight(weight), fg(&cell));
            if cell.underline {
                let origin = at(row, start);
                #[allow(clippy::cast_precision_loss)] // Short runs.
                let width = text.chars().count() as f32 * cell_w;
                ui.gfx.fill_rect(
                    Rect::new(origin.x, origin.y + cell_h - 2.0, width, 1.0),
                    0.0,
                    fg(&cell),
                );
            }
        }
    }
    if let Some((row, column)) = screen.cursor {
        let origin = at(row, column);
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
                ui.gfx
                    .draw_text(origin, &cell.ch.to_string(), font, background);
            }
        } else {
            ui.gfx.stroke_rect(block, 1.0, 1.0, cursor);
        }
    }
    ui.pop_clip();
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
