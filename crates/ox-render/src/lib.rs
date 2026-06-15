//! Semantic color roles and terminal styling for OxyMake CLI output.
//!
//! All colored output in OxyMake flows through the [`Theme`] struct, which maps
//! semantic roles (success, error, warning, etc.) to concrete `console::Style`
//! values. Use [`Theme::from_env`] to auto-detect color support based on
//! `--color` flag, `NO_COLOR`, `TERM=dumb`, `CI`, and TTY detection.

use std::io::IsTerminal;

use console::Style;

/// Color mode selected by `--color=auto|always|never`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for ColorMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(format!(
                "invalid color mode: '{other}' (expected: auto, always, never)"
            )),
        }
    }
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Always => write!(f, "always"),
            Self::Never => write!(f, "never"),
        }
    }
}

/// Unicode symbols for job status display.
#[derive(Clone)]
pub struct Symbols {
    pub success: &'static str,
    pub failure: &'static str,
    pub warning: &'static str,
    pub running: &'static str,
    pub cached: &'static str,
    pub skip: &'static str,
}

impl Symbols {
    fn unicode() -> Self {
        Self {
            success: "\u{2713}", // ✓
            failure: "\u{2717}", // ✗
            warning: "\u{26A0}", // ⚠
            running: "\u{25B8}", // ▸
            cached: "\u{2713}",  // ✓ (same glyph, different color)
            skip: "\u{2014}",    // —
        }
    }
}

/// Progress bar style parameters.
#[derive(Clone)]
pub struct ProgressStyle {
    pub fill: char,
    pub empty: char,
    pub width: usize,
}

impl Default for ProgressStyle {
    fn default() -> Self {
        Self {
            fill: '\u{2588}',  // █
            empty: '\u{2591}', // ░
            width: 30,
        }
    }
}

/// Semantic color roles resolved to concrete styles.
#[derive(Clone)]
pub struct Theme {
    pub success: Style,
    pub error: Style,
    pub warning: Style,
    pub info: Style,
    pub running: Style,
    pub cached: Style,
    pub highlight: Style,
    pub muted: Style,
    pub header: Style,
    pub command: Style,
    pub symbols: Symbols,
    pub progress: ProgressStyle,
}

impl Theme {
    /// Default theme with full color.
    pub fn colored() -> Self {
        Self {
            success: Style::new().green().bold(),
            error: Style::new().red().bold(),
            warning: Style::new().yellow(),
            info: Style::new().blue(),
            running: Style::new().yellow(),
            cached: Style::new().green().dim(),
            highlight: Style::new().cyan().bold(),
            muted: Style::new().dim(),
            header: Style::new().bold(),
            command: Style::new().dim(),
            symbols: Symbols::unicode(),
            progress: ProgressStyle::default(),
        }
    }

    /// Plain theme with no color (all styles are identity).
    pub fn plain() -> Self {
        Self {
            success: Style::new(),
            error: Style::new(),
            warning: Style::new(),
            info: Style::new(),
            running: Style::new(),
            cached: Style::new(),
            highlight: Style::new(),
            muted: Style::new(),
            header: Style::new(),
            command: Style::new(),
            symbols: Symbols::unicode(),
            progress: ProgressStyle::default(),
        }
    }

    /// Select colored or plain based on CLI flag and environment.
    pub fn from_env(cli_flag: Option<ColorMode>, stream: &dyn IsTerminal) -> Self {
        if should_color(cli_flag, stream) {
            Self::colored()
        } else {
            Self::plain()
        }
    }
}

/// Determine whether color output should be enabled.
///
/// Priority order:
/// 1. `--color=always|never` (highest)
/// 2. `NO_COLOR` environment variable
/// 3. `TERM=dumb`
/// 4. `CI=true`
/// 5. TTY detection (lowest)
pub fn should_color(cli_flag: Option<ColorMode>, stream: &dyn IsTerminal) -> bool {
    match cli_flag {
        Some(ColorMode::Always) => return true,
        Some(ColorMode::Never) => return false,
        Some(ColorMode::Auto) | None => {}
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var("TERM").ok().as_deref() == Some("dumb") {
        return false;
    }
    if std::env::var_os("CI").is_some() {
        return false;
    }
    stream.is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_always_wins() {
        // Always should enable color regardless of stream.
        assert!(should_color(Some(ColorMode::Always), &std::io::stderr()));
    }

    #[test]
    fn explicit_never_wins() {
        // Never should disable color regardless of stream.
        assert!(!should_color(Some(ColorMode::Never), &std::io::stderr()));
    }

    #[test]
    fn theme_colored_styles_apply() {
        let t = Theme::colored();
        let s = t.success.apply_to("ok");
        assert!(format!("{s}").contains("ok"));
    }

    #[test]
    fn theme_plain_no_ansi() {
        let t = Theme::plain();
        let s = t.success.apply_to("ok");
        assert_eq!(format!("{s}"), "ok");
    }

    #[test]
    fn color_mode_from_str() {
        assert_eq!("auto".parse::<ColorMode>().unwrap(), ColorMode::Auto);
        assert_eq!("always".parse::<ColorMode>().unwrap(), ColorMode::Always);
        assert_eq!("never".parse::<ColorMode>().unwrap(), ColorMode::Never);
        assert!("invalid".parse::<ColorMode>().is_err());
    }

    #[test]
    fn theme_clone() {
        let t = Theme::colored();
        let t2 = t.clone();
        assert!(format!("{}", t2.success.apply_to("x")).contains("x"));
    }
}
