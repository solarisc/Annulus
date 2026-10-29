//! Virtualization math for long lists and card grids (DESIGN §7.2): which items are visible,
//! so only those exist as nodes. The host list with hundreds of hosts stays as cheap as a
//! screenful (P-01, P-02).

use std::ops::Range;

/// Rows kept alive above and below the viewport, so fast scrolling never shows a gap.
pub const OVERSCAN_ROWS: usize = 3;

/// The items of a fixed-row-height list that intersect the viewport, plus `overscan` rows
/// on each side. `offset` is the scroll position in DIPs.
#[must_use]
pub fn list_window(
    count: usize,
    row_height: f32,
    viewport: f32,
    offset: f32,
    overscan: usize,
) -> Range<usize> {
    if count == 0
        || row_height.is_nan()
        || row_height <= 0.0
        || !viewport.is_finite()
        || viewport <= 0.0
    {
        return 0..0;
    }
    let offset = if offset.is_finite() {
        offset.max(0.0)
    } else {
        0.0
    };
    let first = row_at(offset, row_height);
    let last = row_at(offset + viewport, row_height).saturating_add(1);
    first.saturating_sub(overscan).min(count)..last.saturating_add(overscan).min(count)
}

/// How a wrapping card grid places `count` cards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridGeometry {
    /// Cards per row.
    pub columns: usize,
    /// Width each card gets, filling the row.
    pub card_width: f32,
    /// Rows needed for all cards.
    pub rows: usize,
}

/// Lays out `count` cards at least `min_card_width` wide, `gap` apart, in `width`.
#[must_use]
pub fn grid_geometry(count: usize, width: f32, min_card_width: f32, gap: f32) -> GridGeometry {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let gap = if gap.is_finite() { gap.max(0.0) } else { 0.0 };
    let min_card = if min_card_width.is_finite() && min_card_width > 0.0 {
        min_card_width
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Floored, non-negative, small.
    let columns = (((width + gap) / (min_card + gap)).floor() as usize).max(1);
    #[allow(clippy::cast_precision_loss)] // Column counts are tiny.
    let card_width = ((width - gap * (columns - 1) as f32) / columns as f32).max(0.0);
    GridGeometry {
        columns,
        card_width,
        rows: count.div_ceil(columns),
    }
}

/// The cards of a grid that intersect the viewport, plus whole overscan rows.
#[must_use]
pub fn grid_window(
    count: usize,
    geometry: GridGeometry,
    row_height: f32,
    gap: f32,
    viewport: f32,
    offset: f32,
) -> Range<usize> {
    let rows = list_window(
        geometry.rows,
        row_height + gap.max(0.0),
        viewport,
        offset,
        OVERSCAN_ROWS,
    );
    (rows.start * geometry.columns).min(count)..(rows.end * geometry.columns).min(count)
}

fn row_at(position: f32, row_height: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Non-negative; saturates.
    let row = (position / row_height).floor() as usize;
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_window_covers_the_viewport_plus_overscan() {
        // 500 hosts in 36-DIP rows, a 720-DIP viewport scrolled to row 100.
        assert_eq!(list_window(500, 36.0, 720.0, 3600.0, 3), 97..124);
        assert_eq!(list_window(500, 36.0, 720.0, 0.0, 3), 0..24);
    }

    #[test]
    fn list_window_is_clamped_at_the_end_and_for_hostile_input() {
        assert_eq!(list_window(10, 36.0, 720.0, 0.0, 3), 0..10);
        assert_eq!(list_window(500, 36.0, 720.0, 1e9, 3), 500..500);
        assert_eq!(list_window(500, 0.0, 720.0, 0.0, 3), 0..0);
        assert_eq!(list_window(500, 36.0, f32::NAN, 0.0, 3), 0..0);
        assert_eq!(list_window(500, 36.0, 720.0, f32::NAN, 3), 0..24);
        assert_eq!(list_window(0, 36.0, 720.0, 0.0, 3), 0..0);
    }

    #[test]
    fn a_screenful_of_500_hosts_creates_few_nodes() {
        let window = list_window(500, 36.0, 1080.0, 9000.0, OVERSCAN_ROWS);
        assert!(window.len() <= 40, "{window:?}");
    }

    #[test]
    fn grid_fits_as_many_columns_as_the_minimum_width_allows() {
        let grid = grid_geometry(10, 1000.0, 220.0, 16.0);
        assert_eq!((grid.columns, grid.rows), (4, 3));
        // Four cards and three gaps fill the width exactly.
        assert!((grid.card_width * 4.0 + 16.0 * 3.0 - 1000.0).abs() < 0.01);
        assert_eq!(grid_geometry(10, 100.0, 220.0, 16.0).columns, 1);
        assert_eq!(grid_geometry(10, f32::NAN, 220.0, 16.0).columns, 1);
    }

    #[test]
    fn grid_window_returns_whole_rows_of_cards() {
        let grid = grid_geometry(500, 1000.0, 220.0, 16.0);
        let window = grid_window(500, grid, 72.0, 16.0, 600.0, 880.0);
        assert_eq!(window.start % grid.columns, 0);
        // Row 10 is at the top (880 / 88); three rows of overscan above it.
        assert_eq!(window.start, 7 * grid.columns);
    }
}
