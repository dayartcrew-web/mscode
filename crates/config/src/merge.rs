//! Field-by-field merge of stacked partial configs.
//!
//! Higher layers override lower ones; absent fields are skipped. The merge is
//! pure — no I/O, no mutation of the input stack.

use crate::{Config, PartialConfig};

/// Reduce an ordered stack of [`PartialConfig`] (lowest precedence first)
/// into a concrete [`Config`].
///
/// Layers default to [`Config::default()`] when the stack is empty.
pub fn merge_configs(stack: &[PartialConfig]) -> Config {
    let mut data_dir = None;
    let mut log_level = None;
    let mut provider = None;

    for layer in stack {
        if let Some(v) = &layer.data_dir {
            data_dir = Some(v.clone());
        }
        if let Some(v) = &layer.log_level {
            log_level = Some(v.clone());
        }
        if let Some(v) = &layer.provider {
            provider = Some(v.clone());
        }
    }

    let defaults = Config::default();
    Config {
        data_dir: data_dir.unwrap_or(defaults.data_dir),
        log_level: log_level.unwrap_or(defaults.log_level),
        provider: provider.unwrap_or(defaults.provider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderConfig;
    use std::path::PathBuf;

    #[test]
    fn empty_stack_returns_defaults() {
        let cfg = merge_configs(&[]);
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn single_layer_is_applied() {
        let partial = PartialConfig {
            log_level: Some("debug".into()),
            ..Default::default()
        };
        let cfg = merge_configs(&[partial]);
        assert_eq!(cfg.log_level, "debug");
    }

    #[test]
    fn later_layer_overrides_earlier_for_set_fields() {
        let low = PartialConfig {
            log_level: Some("info".into()),
            provider: Some(ProviderConfig::OpenAI),
            ..Default::default()
        };
        let high = PartialConfig {
            log_level: Some("trace".into()),
            ..Default::default()
        };
        let cfg = merge_configs(&[low, high]);
        assert_eq!(cfg.log_level, "trace");
        assert_eq!(cfg.provider, ProviderConfig::OpenAI);
    }

    #[test]
    fn merge_handles_data_dir_overrides() {
        let low = PartialConfig {
            data_dir: Some(PathBuf::from("/a")),
            ..Default::default()
        };
        let high = PartialConfig {
            data_dir: Some(PathBuf::from("/b")),
            ..Default::default()
        };
        let cfg = merge_configs(&[low, high]);
        assert_eq!(cfg.data_dir, PathBuf::from("/b"));
    }
}
