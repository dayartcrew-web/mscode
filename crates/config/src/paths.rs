//! Default filesystem locations for each config layer.
//!
//! Centralizing these as pure functions makes them trivially testable without
//! spawning a process or touching the user's actual config dir.

use std::path::PathBuf;

use crate::ConfigLayer;

/// Returns the canonical config path for a layer, ignoring env overrides.
///
/// Returns `None` for layers that do not have a filesystem representation
/// ([`Environment`](ConfigLayer::Environment) and
/// [`CommandLine`](ConfigLayer::CommandLine)).
pub fn default_config_path(layer: ConfigLayer) -> Option<PathBuf> {
    match layer {
        ConfigLayer::System => Some(default_system_config_path()),
        ConfigLayer::User => Some(default_user_config_path()),
        ConfigLayer::Project => Some(default_project_config_path()),
        ConfigLayer::Environment | ConfigLayer::CommandLine => None,
    }
}

/// System-wide config path.
///
/// - Windows: `C:\ProgramData\mscode\config.toml`
/// - Unix: `/etc/mscode/config.toml`
pub fn default_system_config_path() -> PathBuf {
    if cfg!(target_os = "windows") {
        PathBuf::from(r"C:\ProgramData\mscode\config.toml")
    } else {
        PathBuf::from("/etc/mscode/config.toml")
    }
}

/// User-scoped config path, derived from `dirs::config_dir()`.
///
/// Returns `mscode/config.toml` under the OS config dir
/// (e.g. `%APPDATA%\mscode\config.toml` on Windows, `~/.config/mscode/config.toml` on Linux).
pub fn default_user_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mscode")
        .join("config.toml")
}

/// Project-local config path (`.mscode/config.toml` in the current directory).
pub fn default_project_config_path() -> PathBuf {
    PathBuf::from(".mscode").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_path_matches_os_convention() {
        let p = default_system_config_path();
        if cfg!(target_os = "windows") {
            assert!(p.ends_with(r"ProgramData\mscode\config.toml"));
        } else {
            assert!(p.ends_with("etc/mscode/config.toml"));
        }
    }

    #[test]
    fn project_path_is_dotmasdy_in_cwd() {
        assert_eq!(
            default_project_config_path(),
            PathBuf::from(".mscode").join("config.toml")
        );
    }

    #[test]
    fn user_path_ends_with_mscode_config_toml() {
        let p = default_user_config_path();
        let s = p.to_string_lossy();
        assert!(s.contains("mscode"));
        assert!(s.contains("config.toml"));
    }

    #[test]
    fn default_config_path_returns_none_for_non_fs_layers() {
        assert!(default_config_path(ConfigLayer::Environment).is_none());
        assert!(default_config_path(ConfigLayer::CommandLine).is_none());
    }

    #[test]
    fn default_config_path_returns_some_for_fs_layers() {
        assert!(default_config_path(ConfigLayer::System).is_some());
        assert!(default_config_path(ConfigLayer::User).is_some());
        assert!(default_config_path(ConfigLayer::Project).is_some());
    }
}
