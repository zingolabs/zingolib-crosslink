//! Pure formatting helpers shared by the views.

pub const ZATS_PER_ZEC: u64 = 100_000_000;

/// Formats zatoshis as ZEC with display precision chosen by magnitude: two places at 100 ZEC
/// and above, four places below that, and all eight below 0.01 ZEC. Trailing zeros are
/// trimmed. A nonzero amount is never shown as zero.
pub fn zec(zats: u64) -> String {
    if zats == 0 {
        return "0".into();
    }
    let places = if zats >= 100 * ZATS_PER_ZEC {
        2
    } else if zats >= ZATS_PER_ZEC / 100 {
        4
    } else {
        8
    };
    let unit = 10u64.pow(8 - places);
    let rounded = (zats + unit / 2) / unit;
    if rounded == 0 {
        trim(zec_full(zats))
    } else {
        trim(with_places(rounded, places))
    }
}

/// Formats zatoshis as ZEC with all eight decimal places.
pub fn zec_full(zats: u64) -> String {
    with_places(zats, 8)
}

fn with_places(units: u64, places: u32) -> String {
    let scale = 10u64.pow(places);
    format!(
        "{}.{:0width$}",
        group(units / scale),
        units % scale,
        width = places as usize
    )
}

fn trim(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Groups thousands with commas.
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Local date and time for a unix timestamp.
pub fn datetime(ts: u32) -> String {
    match chrono::DateTime::from_timestamp(i64::from(ts), 0) {
        Some(utc) if ts > 0 => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        _ => "unknown time".into(),
    }
}

/// Local date only.
pub fn date(ts: u32) -> String {
    match chrono::DateTime::from_timestamp(i64::from(ts), 0) {
        Some(utc) if ts > 0 => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
        _ => "unknown".into(),
    }
}

pub fn duration(seconds: u64) -> String {
    match seconds {
        s if s >= 3600 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{s}s"),
    }
}

/// Keeps the head and tail of a long string, joined by an ellipsis.
pub fn truncate_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 5 {
        return s.chars().take(max.max(n.min(max))).collect();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let start: String = s.chars().take(head).collect();
    let end: String = s.chars().skip(n - tail).collect();
    format!("{start}…{end}")
}

/// Cuts a string to `max` characters, ending with an ellipsis when cut.
pub fn truncate_end(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Filled and empty cell counts of a progress bar `width` cells wide.
pub fn bar_split(frac: f64, width: usize) -> (usize, usize) {
    let filled = ((frac.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    (filled, width - filled)
}

/// Text progress bar of `width` cells.
#[cfg(test)]
pub fn bar(frac: f64, width: usize) -> String {
    let (filled, empty) = bar_split(frac, width);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// Wraps a string into lines of at most `width` characters, breaking anywhere. Meant for
/// addresses and keys, which have no natural word boundaries.
pub fn wrap_chars(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let chars: Vec<char> = s.chars().collect();
    chars
        .chunks(width)
        .map(|c| c.iter().collect())
        .collect::<Vec<String>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zec_precision_follows_magnitude() {
        assert_eq!(zec(0), "0");
        assert_eq!(zec(150_000_000), "1.5");
        assert_eq!(zec(10_000_000_000), "100");
        assert_eq!(zec(12_345_678_901), "123.46");
        assert_eq!(zec(1_234_567), "0.0123");
        assert_eq!(zec(999_999), "0.00999999");
        assert_eq!(zec(4), "0.00000004");
        assert_eq!(zec(123_456_789_012_345), "1,234,567.89");
    }

    #[test]
    fn zec_full_keeps_all_places() {
        assert_eq!(zec_full(150_000_000), "1.50000000");
        assert_eq!(zec_full(1), "0.00000001");
    }

    #[test]
    fn grouping() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(2_600_000), "2,600,000");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate_middle("abcdefghij", 20), "abcdefghij");
        assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
        assert_eq!(truncate_end("abcdef", 4), "abc…");
        assert_eq!(truncate_end("abc", 4), "abc");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(5), "5s");
        assert_eq!(duration(65), "1m05s");
        assert_eq!(duration(3661), "1h01m");
    }

    #[test]
    fn bars_and_wrapping() {
        assert_eq!(bar(0.5, 4), "██░░");
        assert_eq!(wrap_chars("abcdefg", 3), vec!["abc", "def", "g"]);
    }
}
