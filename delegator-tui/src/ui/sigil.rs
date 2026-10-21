//! A sigil: an 8×8 two-tone glyph made from a hash of the wallet's key material, mirrored
//! left to right so it reads as a figure, in a double frame. The same wallet always gets the
//! same sigil, so it can be recognised at a glance; it reveals nothing about the key.

use std::hash::{DefaultHasher, Hash, Hasher};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::theme::Theme;

/// Columns the framed sigil takes.
pub const WIDTH: u16 = 10;
/// Rows the framed sigil takes.
pub const HEIGHT: u16 = 6;

/// The 8×8 bitmap for `material`: 32 bits of a hash, each row's four bits mirrored.
pub fn bitmap(material: &str) -> [[bool; 8]; 8] {
    let mut hasher = DefaultHasher::new();
    material.hash(&mut hasher);
    let bits = hasher.finish();
    let mut rows = [[false; 8]; 8];
    for (y, row) in rows.iter_mut().enumerate() {
        for x in 0..4 {
            let on = (bits >> (y * 4 + x)) & 1 == 1;
            row[x] = on;
            row[7 - x] = on;
        }
    }
    rows
}

/// The bitmap as four rows of half-block characters.
pub fn rows(material: &str) -> [String; 4] {
    let map = bitmap(material);
    let mut out: [String; 4] = Default::default();
    for (i, line) in out.iter_mut().enumerate() {
        for x in 0..8 {
            let top = map[i * 2][x];
            let bottom = map[i * 2 + 1][x];
            line.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
    }
    out
}

/// Draws the framed sigil with its top-left corner at `area`'s origin.
pub fn draw(frame: &mut Frame, area: Rect, t: &Theme, material: &str) {
    let rect = Rect {
        width: WIDTH.min(area.width),
        height: HEIGHT.min(area.height),
        ..area
    };
    if rect.width < WIDTH || rect.height < HEIGHT {
        return;
    }
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(t.accent);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let lines: Vec<Line<'static>> = rows(material)
        .into_iter()
        .map(|r| Line::from(Span::styled(r, t.accent)))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_material_gives_the_same_mirrored_glyph() {
        let a = bitmap("uviewtest1abc");
        assert_eq!(a, bitmap("uviewtest1abc"));
        assert_ne!(a, bitmap("uviewtest1abd"));
        for row in a {
            for x in 0..4 {
                assert_eq!(row[x], row[7 - x]);
            }
        }
        for row in rows("anything") {
            assert_eq!(row.chars().count(), 8);
        }
    }
}
