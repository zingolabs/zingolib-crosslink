//! User preferences that outlive a session.
//!
//! Stored as `key = value` lines in `delegator-tui/config` under the platform configuration
//! directory. Preferences are per user, not per wallet, so they do not live next to the
//! wallet file. Unknown keys and comments are kept when the file is rewritten.

use std::path::{Path, PathBuf};

/// `~/.config/delegator-tui/config` on Linux, `~/Library/Application Support/delegator-tui/config`
/// on macOS.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("delegator-tui").join("config"))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub theme: Option<String>,
}

impl Settings {
    /// Reads the file. A missing or unreadable file yields the defaults.
    pub fn read(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot read {}: {e}", path.display());
                }
                Self::default()
            }
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for (key, value) in text.lines().filter_map(pair) {
            if key == "theme" && !value.is_empty() {
                settings.theme = Some(value.to_string());
            }
        }
        settings
    }
}

/// Splits a `key = value` line. Comments and blank lines yield `None`.
fn pair(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

/// Records `theme` in the file, creating it and its directory when needed. Every other line
/// is left as it was.
pub fn save_theme(path: &Path, theme: &str) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let entry = format!("theme = {theme}");
    let mut replaced = false;
    let mut lines = Vec::new();
    for line in existing.lines() {
        match pair(line) {
            // the first theme line is rewritten in place, any later duplicate is dropped
            Some(("theme", _)) => {
                if !replaced {
                    lines.push(entry.clone());
                    replaced = true;
                }
            }
            _ => lines.push(line.to_string()),
        }
    }
    if !replaced {
        lines.push(entry);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_and_ignores_the_rest() {
        let s = Settings::parse("# preferences\n\n  theme =  nord  \nunknown = 1\n");
        assert_eq!(s.theme.as_deref(), Some("nord"));
        assert_eq!(Settings::parse("theme =").theme, None);
        assert_eq!(Settings::parse("").theme, None);
    }

    #[test]
    fn a_missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::read(&dir.path().join("absent")),
            Settings::default()
        );
    }

    #[test]
    fn saving_creates_then_replaces_and_keeps_other_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config");
        save_theme(&path, "nord").unwrap();
        assert_eq!(Settings::read(&path).theme.as_deref(), Some("nord"));

        std::fs::write(&path, "# mine\nfuture = yes\ntheme = nord\n").unwrap();
        save_theme(&path, "gruvbox-light").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# mine\nfuture = yes\ntheme = gruvbox-light\n");
    }
}
