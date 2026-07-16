//! Layered configuration for the mscode CLI.
//!
//! Precedence chain (lowest → highest): [`System`](ConfigLayer::System) →
//! [`User`](ConfigLayer::User) → [`Project`](ConfigLayer::Project) →
//! [`Environment`](ConfigLayer::Environment) →
//! [`CommandLine`](ConfigLayer::CommandLine).
//!
//! Each TOML file on the chain contributes a partial [`Config`]; later layers
//! override earlier ones field-by-field. The final merge produces the
//! effective configuration used at runtime.

pub mod env;
pub mod loader;
pub mod merge;
pub mod paths;

pub use env::collect_from_env;
pub use loader::{load_config, load_partial_from_path, parse_toml_str};
pub use merge::merge_configs;
pub use paths::{
    default_config_path, default_project_config_path, default_system_config_path,
    default_user_config_path,
};

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Names a layer in the precedence chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigLayer {
    System,
    User,
    Project,
    Environment,
    CommandLine,
}

impl ConfigLayer {
    /// Layers in precedence order (lowest first).
    pub const fn precedence() -> &'static [ConfigLayer] {
        &[
            ConfigLayer::System,
            ConfigLayer::User,
            ConfigLayer::Project,
            ConfigLayer::Environment,
            ConfigLayer::CommandLine,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConfigLayer::System => "system",
            ConfigLayer::User => "user",
            ConfigLayer::Project => "project",
            ConfigLayer::Environment => "environment",
            ConfigLayer::CommandLine => "cli",
        }
    }
}

/// Provider-related configuration block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderConfig {
    /// Disable provider calls entirely (default for offline-first v1.0).
    #[default]
    None,
    Anthropic,
    OpenAI,
    Ollama,
}

/// Effective configuration after all layers merge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub data_dir: PathBuf,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub provider: ProviderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::new(),
            log_level: default_log_level(),
            provider: ProviderConfig::None,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

/// A [`Config`] sub-view that only carries explicitly-set fields.
///
/// Every field is `Option`; `None` means "this layer did not specify a value,
/// do not override the underlying layer". [`merge_configs`] collapses a stack
/// of these into a concrete [`Config`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderConfig>,
}

impl PartialConfig {
    /// Construct a partial config from an env-var iterator.
    ///
    /// `vars` yields `(name, value)` pairs. Names beginning with `MSCODE_` map
    /// to fields (case-insensitive, suffix after the prefix is the field name).
    pub fn from_env_vars<'a, I>(vars: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut out = PartialConfig::default();
        for (name, value) in vars {
            let Some(suffix) = name.strip_prefix("MSCODE_") else {
                continue;
            };
            match suffix.to_ascii_lowercase().as_str() {
                "log_level" | "log-level" => out.log_level = Some(value.to_string()),
                "data_dir" | "data-dir" => out.data_dir = Some(PathBuf::from(value)),
                "provider" => {
                    out.provider = match value.to_ascii_lowercase().as_str() {
                        "anthropic" => Some(ProviderConfig::Anthropic),
                        "openai" => Some(ProviderConfig::OpenAI),
                        "ollama" => Some(ProviderConfig::Ollama),
                        "none" => Some(ProviderConfig::None),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Convert a fully-realized [`Config`] into a saturated partial (all fields
    /// set). Useful for the CLI layer where every override is explicit.
    pub fn from_full(cfg: &Config) -> Self {
        Self {
            data_dir: Some(cfg.data_dir.clone()),
            log_level: Some(cfg.log_level.clone()),
            provider: Some(cfg.provider.clone()),
        }
    }
}

impl From<Config> for PartialConfig {
    fn from(value: Config) -> Self {
        Self::from_full(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_log_level() {
        let cfg = Config::default();
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.provider, ProviderConfig::None);
    }

    #[test]
    fn layer_precedence_order_is_documented() {
        let order = ConfigLayer::precedence();
        assert_eq!(order[0], ConfigLayer::System);
        assert_eq!(order[4], ConfigLayer::CommandLine);
        assert_eq!(order.len(), 5);
    }

    #[test]
    fn layer_as_str_matches_lowercase_canonical_names() {
        assert_eq!(ConfigLayer::System.as_str(), "system");
        assert_eq!(ConfigLayer::CommandLine.as_str(), "cli");
    }

    #[test]
    fn env_vars_populate_partial_config() {
        let vars = vec![
            ("MSCODE_LOG_LEVEL", "debug"),
            ("MSCODE_PROVIDER", "anthropic"),
            ("UNRELATED", "ignore"),
        ];
        let partial = PartialConfig::from_env_vars(vars);
        assert_eq!(partial.log_level.as_deref(), Some("debug"));
        assert_eq!(partial.provider, Some(ProviderConfig::Anthropic));
        assert!(partial.data_dir.is_none());
    }

    #[test]
    fn env_vars_skip_unknown_provider_values() {
        let vars = vec![("MSCODE_PROVIDER", "unknown-provider")];
        let partial = PartialConfig::from_env_vars(vars);
        assert!(partial.provider.is_none());
    }

    #[test]
    fn env_vars_accept_kebab_case_aliases() {
        let vars = vec![
            ("MSCODE_LOG-LEVEL", "trace"),
            ("MSCODE_DATA-DIR", "/tmp/mscode"),
        ];
        let partial = PartialConfig::from_env_vars(vars);
        assert_eq!(partial.log_level.as_deref(), Some("trace"));
        assert_eq!(
            partial.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/mscode"))
        );
    }

    #[test]
    fn from_full_round_trips_saturated_fields() {
        let cfg = Config {
            data_dir: PathBuf::from("/var/mscode"),
            log_level: "warn".into(),
            provider: ProviderConfig::Ollama,
        };
        let partial = PartialConfig::from_full(&cfg);
        assert_eq!(
            partial.data_dir.as_deref(),
            Some(std::path::Path::new("/var/mscode"))
        );
        assert_eq!(partial.log_level.as_deref(), Some("warn"));
        assert_eq!(partial.provider, Some(ProviderConfig::Ollama));
    }
}
