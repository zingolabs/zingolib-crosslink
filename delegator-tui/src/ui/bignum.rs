//! Large digits for the balance: a 3x5 pixel font drawn with half blocks, three rows tall.
//! Terminal cells are about twice as tall as wide, so each half block is a square pixel.

/// Six pixel rows per glyph, `#` lit. The sixth row is below the baseline, for the comma.
fn glyph(c: char) -> Option<[&'static str; 6]> {
    Some(match c {
        '0' => ["###", "#.#", "#.#", "#.#", "###", "..."],
        '1' => [".#.", "##.", ".#.", ".#.", "###", "..."],
        '2' => ["###", "..#", "###", "#..", "###", "..."],
        '3' => ["###", "..#", ".##", "..#", "###", "..."],
        '4' => ["#.#", "#.#", "###", "..#", "..#", "..."],
        '5' => ["###", "#..", "###", "..#", "###", "..."],
        '6' => ["###", "#..", "###", "#.#", "###", "..."],
        '7' => ["###", "..#", "..#", "..#", "..#", "..."],
        '8' => ["###", "#.#", "###", "#.#", "###", "..."],
        '9' => ["###", "#.#", "###", "..#", "###", "..."],
        '.' => [".", ".", ".", ".", "#", "."],
        ',' => [".", ".", ".", ".", "#", "#"],
        _ => return None,
    })
}

/// Renders `text` as three rows, one column between glyphs. `None` if any character has no
/// glyph, so the caller can fall back to plain text.
pub fn render(text: &str) -> Option<[String; 3]> {
    let mut rows = [String::new(), String::new(), String::new()];
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c)?;
        if i > 0 {
            rows.iter_mut().for_each(|row| row.push(' '));
        }
        for (r, row) in rows.iter_mut().enumerate() {
            let (top, bottom) = (g[2 * r].as_bytes(), g[2 * r + 1].as_bytes());
            for x in 0..top.len() {
                row.push(match (top[x] == b'#', bottom[x] == b'#') {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
        }
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_character_renders_evenly() {
        let rows = render("0123456789.,").unwrap();
        let width = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == width));
        // ten digits of three, two marks of one, eleven gaps
        assert_eq!(width, 10 * 3 + 2 + 11);
    }

    #[test]
    fn digits_are_distinct() {
        let shapes: Vec<[String; 3]> = ('0'..='9')
            .map(|d| render(&d.to_string()).unwrap())
            .collect();
        for (i, a) in shapes.iter().enumerate() {
            for b in &shapes[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn eight_and_the_decimal_point_look_right() {
        assert_eq!(
            render("8").unwrap(),
            ["█▀█", "█▀█", "▀▀▀"].map(String::from)
        );
        assert_eq!(render("1.5").unwrap()[2], "▀▀▀ ▀ ▀▀▀");
    }

    #[test]
    fn unknown_characters_fall_back() {
        assert!(render("1.5 ZEC").is_none());
        assert!(render("").is_some());
    }
}
