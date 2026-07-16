//! Environment-variable adapter for the Environment config layer.
//!
//! Reads the live process environment, filters to `MSCODE_*` variables, and
//! produces a [`PartialConfig`] that participates in the layered merge.

use std::collections::HashMap;

use crate::PartialConfig;

/// Build a [`PartialConfig`] from the current process's environment.
///
/// Only variables whose names start with `MSCODE_` are considered; everything
/// else is ignored. See [`PartialConfig::from_env_vars`] for the supported
/// field-name mapping.
pub fn collect_from_env() -> PartialConfig {
    let pairs: Vec<(String, String)> = std::env::vars().collect();
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut sorted: HashMap<&str, &str> = HashMap::new();
    for (k, v) in &borrowed {
        sorted.insert(k, v);
    }
    let vec: Vec<(&str, &str)> = sorted.into_iter().collect();
    PartialConfig::from_env_vars(vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_from_env_picks_up_mscode_prefix() {
        // SAFETY: tests run serially within a single binary; setting an env
        // var scoped to a unique name avoids collisions.
        unsafe {
            std::env::set_var("MSCODE_TEST_ENV_LOG_LEVEL", "should_be_ignored");
            std::env::set_var("MSCODE_LOG_LEVEL", "debug");
        }
        let partial = collect_from_env();
        assert_eq!(partial.log_level.as_deref(), Some("debug"));
        unsafe {
            std::env::remove_var("MSCODE_LOG_LEVEL");
            std::env::remove_var("MSCODE_TEST_ENV_LOG_LEVEL");
        }
    }
}
