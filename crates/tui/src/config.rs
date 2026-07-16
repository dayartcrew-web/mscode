//! User-facing configuration for the TUI.
//!
//! [`TuiConfig::default`] produces sensible defaults so the binary can launch
//! without reading any config file. Theme + keybindings are exposed as public
//! structs so a future config-loader can override them.

use serde::{Deserialize, Serialize};

/// Top-level TUI configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Theme controlling colors of the three primary regions.
    pub theme: TuiTheme,
    /// The string shown to the left of the input box (default: `"> "`).
    pub prompt: String,
    /// Show a `[PLAN MODE]` indicator in the status bar when in Planning mode.
    pub show_plan_indicator: bool,
    /// How many lines of history the message buffer retains.
    pub history_capacity: usize,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: TuiTheme::default(),
            prompt: "> ".to_string(),
            show_plan_indicator: true,
            // 10_000 entries is plenty for interactive use; the buffer is
            // bounded so memory growth is O(history_capacity), not O(session).
            history_capacity: 10_000,
        }
    }
}

/// Color / style theme for the three primary regions of the dashboard.
///
/// Stored as plain strings so the type is `Eq + Serialize`. The render layer
/// maps these to `ratatui::style::Color` at draw time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiTheme {
    /// Foreground color for the message log.
    pub log_fg: String,
    /// Foreground color for the input box.
    pub input_fg: String,
    /// Foreground color for the status bar.
    pub status_fg: String,
    /// Background color shared across regions (or `"default"` for terminal default).
    pub bg: String,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            log_fg: "white".to_string(),
            input_fg: "white".to_string(),
            status_fg: "yellow".to_string(),
            bg: "default".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_config_has_sensible_defaults() {
        let cfg = TuiConfig::default();
        assert!(!cfg.prompt.is_empty());
        assert!(cfg.show_plan_indicator);
        assert!(cfg.history_capacity >= 100);
        // Theme must also have non-empty colors so the render layer can map them.
        assert!(!cfg.theme.log_fg.is_empty());
        assert!(!cfg.theme.input_fg.is_empty());
        assert!(!cfg.theme.status_fg.is_empty());
        assert!(!cfg.theme.bg.is_empty());
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = TuiConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: TuiConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn prompt_default_is_aesthetic() {
        // Default prompt should end with whitespace (visual separation).
        let cfg = TuiConfig::default();
        assert!(cfg.prompt.ends_with(' '));
    }
}
