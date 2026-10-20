//! QR code rendering for the terminal.

use qrcode::{EcLevel, QrCode};

/// Renders `data` as lines of text, in Unicode half-blocks or, with `ascii`, in `#` characters.
///
/// With `inverted` false, glyphs mark the dark modules: draw the result in a dark colour on a
/// light background. With `inverted` true, glyphs mark the light modules instead, which reads
/// correctly in a light colour on a dark background, the usual terminal default.
pub fn render(data: &str, ascii: bool, inverted: bool) -> Result<Vec<String>, String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::L)
        .map_err(|e| format!("cannot encode as QR: {e}"))?;
    let text = if ascii {
        let (dark, light) = if inverted { (' ', '#') } else { ('#', ' ') };
        code.render::<char>()
            .quiet_zone(true)
            .module_dimensions(2, 1)
            .dark_color(dark)
            .light_color(light)
            .build()
    } else {
        use qrcode::render::unicode::Dense1x2;
        let (dark, light) = if inverted {
            (Dense1x2::Light, Dense1x2::Dark)
        } else {
            (Dense1x2::Dark, Dense1x2::Light)
        };
        code.render::<Dense1x2>()
            .quiet_zone(true)
            .dark_color(dark)
            .light_color(light)
            .build()
    };
    Ok(text.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_square_ish_output() {
        let lines = render("u1testaddress", false, false).unwrap();
        assert!(lines.len() > 10);
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width));
        let ascii = render("u1testaddress", true, true).unwrap();
        assert!(
            ascii
                .iter()
                .all(|l| l.chars().all(|c| c == '#' || c == ' '))
        );
    }

    #[test]
    fn inversion_swaps_the_glyphs() {
        // the quiet zone is light, so its first row is blank when glyphs mark dark modules and
        // solid when they mark light ones
        let normal = render("u1testaddress", true, false).unwrap();
        let inverted = render("u1testaddress", true, true).unwrap();
        assert!(normal[0].chars().all(|c| c == ' '));
        assert!(inverted[0].chars().all(|c| c == '#'));
    }
}
