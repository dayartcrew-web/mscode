//! Loader that walks the precedence chain and produces a merged [`Config`].
//!
//! Layer order: System → User → Project → Environment → CommandLine.
//! Each filesystem layer is loaded best-effort — a missing file at a layer is
//! treated as an empty partial (does not contribute any field values).

use std::path::Path;

use mscode_shared::Result;

use crate::{
    Config, ConfigLayer, PartialConfig, collect_from_env, default_config_path, merge_configs,
};

/// Load the effective config from the precedence chain.
///
/// `cli_overrides` participates as the highest-precedence layer when supplied.
/// All filesystem layers are loaded best-effort — a missing or unreadable file
/// at a given layer is logged at `warn!` and skipped.
///
/// # Errors
///
/// Returns [`MscodeError::Io`](mscode_shared::MscodeError::Io) only for I/O
/// failures from the CLI-supplied overrides (which the caller controls).
/// Failures in the system/user/project layers are swallowed and logged so the
/// CLI remains usable even on a fresh machine with no config files.
pub fn load_config(cli_overrides: Option<PartialConfig>) -> Result<Config> {
    let mut stack: Vec<PartialConfig> = Vec::new();

    for layer in ConfigLayer::precedence() {
        match layer {
            ConfigLayer::Environment => {
                stack.push(collect_from_env());
            }
            ConfigLayer::CommandLine => {
                if let Some(cli) = cli_overrides.clone() {
                    stack.push(cli);
                }
            }
            fs_layer => {
                if let Some(path) = default_config_path(*fs_layer) {
                    if let Some(partial) = read_optional_partial(&path) {
                        stack.push(partial);
                    }
                }
            }
        }
    }

    Ok(merge_configs(&stack))
}

/// Read a partial config from `path`, returning `None` for missing files.
///
/// Parse errors are logged but not propagated — a corrupt system config should
/// not prevent the user from running `mscode version`.
fn read_optional_partial(path: &Path) -> Option<PartialConfig> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to read config layer");
            return None;
        }
    };

    match toml::from_str::<PartialConfig>(&contents) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "failed to parse config layer");
            None
        }
    }
}

/// Lower-level helper: parse a TOML string into a [`PartialConfig`].
///
/// Exposed for callers (CLI, tests) that need to parse a project file from an
/// already-loaded string rather than a path on disk.
pub fn parse_toml_str(text: &str) -> Result<PartialConfig> {
    toml::from_str(text)
        .map_err(|err| mscode_shared::MscodeError::Config(format!("toml parse error: {err}")))
}

/// Lower-level helper: load a [`PartialConfig`] directly from a path.
///
/// Returns `Ok(None)` if the file does not exist. Returns `Err` on read or
/// parse failure — unlike [`read_optional_partial`], this helper surfaces the
/// error so tests and explicit CLI overrides can fail loudly.
pub fn load_partial_from_path(path: &Path) -> Result<Option<PartialConfig>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    parse_toml_str(&contents).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderConfig;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_toml(text: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(text.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn parse_toml_str_round_trips_full_partial() {
        let text = r#"
data_dir = "/var/mscode"
log_level = "debug"
provider = "anthropic"
"#;
        let parsed = parse_toml_str(text).unwrap();
        assert_eq!(parsed.log_level.as_deref(), Some("debug"));
        assert_eq!(parsed.provider, Some(ProviderConfig::Anthropic));
    }

    #[test]
    fn parse_toml_str_accepts_empty_input() {
        let parsed = parse_toml_str("").unwrap();
        assert_eq!(parsed, PartialConfig::default());
    }

    #[test]
    fn parse_toml_str_returns_err_on_malformed_input() {
        let parsed = parse_toml_str("this is not = = toml");
        assert!(parsed.is_err());
    }

    #[test]
    fn load_partial_from_path_returns_none_for_missing_file() {
        let path =
            std::env::temp_dir().join(format!("mscode-nonexistent-{}.toml", std::process::id()));
        assert!(load_partial_from_path(&path).unwrap().is_none());
    }

    #[test]
    fn load_partial_from_path_reads_existing_file() {
        let file = write_temp_toml("log_level = \"trace\"\n");
        let parsed = load_partial_from_path(file.path()).unwrap().unwrap();
        assert_eq!(parsed.log_level.as_deref(), Some("trace"));
    }

    #[test]
    fn load_config_with_no_files_and_no_overrides_uses_defaults() {
        let cfg = load_config(None).unwrap();
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn load_config_applies_cli_overrides_above_env() {
        // This test verifies merge precedence indirectly: the CLI override
        // we pass in must produce a Config that has the overridden values,
        // regardless of what random env vars may be set on the runner.
        let cli = PartialConfig {
            log_level: Some("error".into()),
            data_dir: Some(std::path::PathBuf::from("/cli/override")),
            provider: Some(ProviderConfig::Ollama),
        };
        let cfg = load_config(Some(cli)).unwrap();
        assert_eq!(cfg.log_level, "error");
        assert_eq!(cfg.data_dir, std::path::PathBuf::from("/cli/override"));
        assert_eq!(cfg.provider, ProviderConfig::Ollama);
    }
}
