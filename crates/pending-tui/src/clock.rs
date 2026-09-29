//! A big clock in blocks, in the style of clock-tui's "bricks" font (the web
//! dashboard draws the same glyphs, see `frontend/src/clock.ts`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Width of a glyph, in blocks.
const GLYPH_WIDTH: u16 = 6;
/// Height of a glyph, in blocks.
pub const GLYPH_ROWS: u16 = 5;
/// Cells between two glyphs, whatever the size.
const SPACING: u16 = 2;

/// Each row alternates the length of an "off" run and an "on" run, starting
/// with "off": `[0, 2, 2, 2]` is `██  ██`.
fn glyph(c: char) -> Option<[&'static [u16]; 5]> {
    Some(match c {
        '0' => [
            &[0, 6],
            &[0, 2, 2, 2],
            &[0, 2, 2, 2],
            &[0, 2, 2, 2],
            &[0, 6],
        ],
        '1' => [&[0, 4], &[2, 2], &[2, 2], &[2, 2], &[0, 6]],
        '2' => [&[0, 6], &[4, 2], &[0, 6], &[0, 2], &[0, 6]],
        '3' => [&[0, 6], &[4, 2], &[0, 6], &[4, 2], &[0, 6]],
        '4' => [&[0, 2, 2, 2], &[0, 2, 2, 2], &[0, 6], &[4, 2], &[4, 2]],
        '5' => [&[0, 6], &[0, 2], &[0, 6], &[4, 2], &[0, 6]],
        '6' => [&[0, 6], &[0, 2], &[0, 6], &[0, 2, 2, 2], &[0, 6]],
        '7' => [&[0, 6], &[4, 2], &[4, 2], &[4, 2], &[4, 2]],
        '8' => [&[0, 6], &[0, 2, 2, 2], &[0, 6], &[0, 2, 2, 2], &[0, 6]],
        '9' => [&[0, 6], &[0, 2, 2, 2], &[0, 6], &[4, 2], &[0, 6]],
        ':' => [&[], &[2, 2], &[], &[2, 2], &[]],
        _ => return None,
    })
}

/// Width and height, in cells, of `chars` glyphs drawn at `size` (cells per
/// block side).
pub fn text_size(chars: usize, size: u16) -> (u16, u16) {
    let chars = chars as u16;
    let width = chars * GLYPH_WIDTH * size + chars.saturating_sub(1) * SPACING;
    (width, GLYPH_ROWS * size)
}

/// The largest size at which `chars` glyphs fit in `width` × `height` cells.
pub fn fit(chars: usize, width: u16, height: u16) -> Option<u16> {
    let chars = chars as u16;
    if chars == 0 {
        return None;
    }
    let spacing = (chars - 1) * SPACING;
    let by_width = width.saturating_sub(spacing) / (chars * GLYPH_WIDTH);
    let by_height = height / GLYPH_ROWS;
    Some(by_width.min(by_height)).filter(|size| *size > 0)
}

/// Draws `text` at `size`, centered horizontally in `area`, from its top row.
pub fn draw(text: &str, size: u16, area: Rect, style: Style, buf: &mut Buffer) {
    let (width, _) = text_size(text.chars().count(), size);
    let mut x = area.x + area.width.saturating_sub(width) / 2;
    for c in text.chars() {
        if let Some(rows) = glyph(c) {
            for (row, runs) in rows.iter().enumerate() {
                let mut left = x;
                for (run, length) in runs.iter().enumerate() {
                    let length = length * size;
                    if run % 2 == 1 {
                        let top = area.y + row as u16 * size;
                        for y in top..top + size {
                            for column in left..left + length {
                                if column < area.right() && y < area.bottom() {
                                    buf[(column, y)].set_symbol("█").set_style(style);
                                }
                            }
                        }
                    }
                    left += length;
                }
            }
        }
        x += GLYPH_WIDTH * size + SPACING;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, size: u16) -> Vec<String> {
        let (width, height) = text_size(text.chars().count(), size);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        draw(text, size, area, Style::default(), &mut buf);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// Glyphs are drawn from the bricks table, two cells apart.
    #[test]
    fn digits_are_drawn_in_blocks() {
        assert_eq!(
            rows("14:0", 1),
            [
                "████    ██  ██          ██████",
                "  ██    ██  ██    ██    ██  ██",
                "  ██    ██████          ██  ██",
                "  ██        ██    ██    ██  ██",
                "██████      ██          ██████",
            ]
        );
    }

    /// A bigger size scales every block in both directions.
    #[test]
    fn size_scales_blocks() {
        let rows = rows("7", 2);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0], "████████████");
        assert_eq!(rows[1], "████████████");
        assert_eq!(rows[2], "        ████");
    }

    /// The size is the largest that fits both the width and the height.
    #[test]
    fn fit_picks_the_largest_size_that_fits() {
        // HH:MM:SS is 8 glyphs: 48 cells per size step plus 14 of spacing.
        assert_eq!(fit(8, 62, 5), Some(1));
        assert_eq!(fit(8, 61, 5), None);
        assert_eq!(fit(8, 110, 5), Some(1), "limited by height");
        assert_eq!(fit(8, 110, 14), Some(2));
        assert_eq!(fit(8, 200, 100), Some(3));
        assert_eq!(fit(8, 100, 4), None);
    }
}
