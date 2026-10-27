//! A big clock in blocks, in the style of clock-tui's "bricks" font: each
//! glyph is 6 blocks wide and 5 tall, and a block is twice as tall as it is
//! wide (the shape of a terminal cell).

use chrono::{DateTime, Local, Timelike};

/// Width of a glyph, in blocks.
const GLYPH_WIDTH: u32 = 6;
/// Height of a glyph, in blocks.
const GLYPH_ROWS: u32 = 5;
/// Height of a block, in block widths.
const BLOCK_HEIGHT: u32 = 2;
/// Blocks between two glyphs.
const SPACING: u32 = 1;

/// Each row alternates the length of an "off" run and an "on" run, starting
/// with "off": `[0, 2, 2, 2]` is `██  ██`.
fn glyph(c: char) -> Option<[&'static [u32]; 5]> {
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

/// A filled rectangle of the clock, in block units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// `text` as blocks, with the width and height of the whole drawing in
/// block units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drawing {
    pub blocks: Vec<Block>,
    pub width: u32,
    pub height: u32,
}

/// Local time as 24-hour `HH:MM:SS`.
pub fn clock_text(now: &DateTime<Local>) -> String {
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

/// Local date, e.g. "Monday, 28 September 2026".
pub fn date_text(now: &DateTime<Local>) -> String {
    now.format("%A, %-d %B %Y").to_string()
}

/// `text` drawn in blocks. Unknown characters take their place but draw
/// nothing.
pub fn drawing(text: &str) -> Drawing {
    let count = text.chars().count() as u32;
    let mut blocks = Vec::new();
    for (index, c) in text.chars().enumerate() {
        let left = index as u32 * (GLYPH_WIDTH + SPACING);
        let Some(rows) = glyph(c) else { continue };
        for (row, runs) in rows.iter().enumerate() {
            let mut x = left;
            for (run, length) in runs.iter().enumerate() {
                if run % 2 == 1 {
                    blocks.push(Block {
                        x,
                        y: row as u32 * BLOCK_HEIGHT,
                        width: *length,
                        height: BLOCK_HEIGHT,
                    });
                }
                x += length;
            }
        }
    }
    Drawing {
        blocks,
        width: (count * (GLYPH_WIDTH + SPACING)).saturating_sub(SPACING),
        height: GLYPH_ROWS * BLOCK_HEIGHT,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// The clock reads `HH:MM:SS`, and the date names the weekday.
    #[test]
    fn texts_are_padded_and_spelled_out() {
        let now = Local.with_ymd_and_hms(2026, 9, 28, 9, 5, 7).unwrap();
        assert_eq!(clock_text(&now), "09:05:07");
        assert_eq!(date_text(&now), "Monday, 28 September 2026");
    }

    /// `HH:MM:SS` is eight glyphs wide with a block between them, and a
    /// `1` is the narrow shape of the font.
    #[test]
    fn draws_the_bricks_font() {
        let whole = drawing("00:00:00");
        assert_eq!((whole.width, whole.height), (8 * 7 - 1, 10));

        let one = drawing("1");
        assert_eq!(one.width, 6);
        assert_eq!(
            one.blocks,
            vec![
                Block {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 2
                },
                Block {
                    x: 2,
                    y: 2,
                    width: 2,
                    height: 2
                },
                Block {
                    x: 2,
                    y: 4,
                    width: 2,
                    height: 2
                },
                Block {
                    x: 2,
                    y: 6,
                    width: 2,
                    height: 2
                },
                Block {
                    x: 0,
                    y: 8,
                    width: 6,
                    height: 2
                },
            ]
        );
        // The second glyph starts after the first one and the gap.
        let two = drawing("11");
        assert_eq!(two.blocks[5].x, 7);
        assert!(drawing("x").blocks.is_empty());
    }
}
