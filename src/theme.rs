//! Runtime theming sourced from the active Omarchy theme palette.
//!
//! Reads `~/.config/omarchy/current/theme/colors.toml` so the spotlight always
//! matches whatever Omarchy theme is currently selected. Falls back to a sane
//! built-in palette if the file is missing or malformed.

use std::path::PathBuf;

use ratatui::style::Color;
use serde::Deserialize;

/// Colors used to paint the UI, already converted to ratatui colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    /// Used for dimmed/secondary text (hints, sources).
    pub muted: Color,
    /// Used for error messages.
    pub error: Color,
    /// Foreground color for a selected list row.
    pub selection_fg: Color,
    /// Background color for a selected list row.
    pub selection_bg: Color,
}

impl Default for Theme {
    /// Built-in fallback palette (matches the user's current Omarchy theme so
    /// the app still looks at home even when the theme file cannot be read).
    fn default() -> Self {
        Theme {
            background: Color::Rgb(0x06, 0x0B, 0x1E),
            foreground: Color::Rgb(0xff, 0xce, 0xad),
            accent: Color::Rgb(0x7d, 0x82, 0xd9),
            muted: Color::Rgb(0x6d, 0x7d, 0xb6),
            error: Color::Rgb(0xED, 0x5B, 0x5A),
            selection_fg: Color::Rgb(0x06, 0x0B, 0x1E),
            selection_bg: Color::Rgb(0xff, 0xce, 0xad),
        }
    }
}

/// Raw shape of the Omarchy `colors.toml`. All fields optional so a partial or
/// differently-structured file degrades gracefully to the fallback values.
#[derive(Debug, Default, Deserialize)]
struct RawColors {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    selection_foreground: Option<String>,
    selection_background: Option<String>,
    /// "bright black" — a good muted/secondary tone in the Omarchy palette.
    color8: Option<String>,
    /// "red" — used for error states.
    color1: Option<String>,
}

impl Theme {
    /// Load the theme from the default Omarchy path, falling back on any error.
    pub fn load() -> Theme {
        match default_colors_path() {
            Some(path) => Theme::from_toml_path(&path),
            None => Theme::default(),
        }
    }

    /// Load from a specific path, falling back to defaults on any failure.
    pub fn from_toml_path(path: &std::path::Path) -> Theme {
        match std::fs::read_to_string(path) {
            Ok(contents) => Theme::from_toml_str(&contents),
            Err(_) => Theme::default(),
        }
    }

    /// Parse a colors.toml string into a Theme, using defaults for any field
    /// that is missing or fails to parse as a hex color.
    pub fn from_toml_str(contents: &str) -> Theme {
        let raw: RawColors = toml::from_str(contents).unwrap_or_default();
        let fallback = Theme::default();

        Theme {
            background: opt_color(&raw.background, fallback.background),
            foreground: opt_color(&raw.foreground, fallback.foreground),
            accent: opt_color(&raw.accent, fallback.accent),
            muted: opt_color(&raw.color8, fallback.muted),
            error: opt_color(&raw.color1, fallback.error),
            selection_fg: opt_color(&raw.selection_foreground, fallback.selection_fg),
            selection_bg: opt_color(&raw.selection_background, fallback.selection_bg),
        }
    }
}

/// Resolve the standard Omarchy active-theme colors.toml path.
fn default_colors_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut p = PathBuf::from(home);
    p.push(".config/omarchy/current/theme/colors.toml");
    Some(p)
}

/// Parse an optional hex string, returning `fallback` when absent or invalid.
fn opt_color(value: &Option<String>, fallback: Color) -> Color {
    value
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or(fallback)
}

/// Parse a `#rrggbb` (or `rrggbb`) hex string into a ratatui RGB color.
pub fn parse_hex_color(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_hex() {
        assert_eq!(parse_hex_color("#7d82d9"), Some(Color::Rgb(0x7d, 0x82, 0xd9)));
        assert_eq!(parse_hex_color("060B1E"), Some(Color::Rgb(0x06, 0x0B, 0x1E)));
        assert_eq!(parse_hex_color("  #FFCEAD  "), Some(Color::Rgb(0xff, 0xce, 0xad)));
    }

    #[test]
    fn rejects_invalid_hex() {
        assert_eq!(parse_hex_color("#fff"), None); // too short
        assert_eq!(parse_hex_color("#gggggg"), None); // non-hex
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(parse_hex_color("#1234567"), None); // too long
    }

    #[test]
    fn parses_sample_colors_toml() {
        let sample = r##"
accent = "#7d82d9"
foreground = "#ffcead"
background = "#060B1E"
selection_foreground = "#060B1E"
selection_background = "#ffcead"
color1 = "#ED5B5A"
color8 = "#6d7db6"
"##;
        let theme = Theme::from_toml_str(sample);
        assert_eq!(theme.accent, Color::Rgb(0x7d, 0x82, 0xd9));
        assert_eq!(theme.background, Color::Rgb(0x06, 0x0B, 0x1E));
        assert_eq!(theme.foreground, Color::Rgb(0xff, 0xce, 0xad));
        assert_eq!(theme.muted, Color::Rgb(0x6d, 0x7d, 0xb6));
        assert_eq!(theme.error, Color::Rgb(0xED, 0x5B, 0x5A));
        assert_eq!(theme.selection_bg, Color::Rgb(0xff, 0xce, 0xad));
    }

    #[test]
    fn malformed_toml_falls_back_to_default() {
        let theme = Theme::from_toml_str("this is not = valid = toml ===");
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn missing_fields_use_fallback() {
        // Only accent provided; everything else should be default.
        let theme = Theme::from_toml_str(r##"accent = "#abcdef""##);
        assert_eq!(theme.accent, Color::Rgb(0xab, 0xcd, 0xef));
        assert_eq!(theme.background, Theme::default().background);
        assert_eq!(theme.foreground, Theme::default().foreground);
    }

    #[test]
    fn invalid_hex_in_toml_uses_fallback_for_that_field() {
        let theme = Theme::from_toml_str(r#"accent = "not-a-color""#);
        assert_eq!(theme.accent, Theme::default().accent);
    }
}
