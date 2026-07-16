//! Sandbox policy struct and validation entry points.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{SandboxError, SandboxResult};
use crate::matcher::PathMatcher;

/// Default per-file size cap (10 MiB).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Default wall-clock cap for an exec call.
pub const DEFAULT_MAX_RUNTIME: Duration = Duration::from_secs(30);

/// The default exec allowlist. Tools that need to shell out must match one of
/// these command stems; arbitrary commands are rejected.
///
/// This is intentionally a static array (rather than computed at runtime) so
/// it is auditable in one place. Add entries here when a new tool genuinely
/// needs shell access.
pub const DEFAULT_EXEC_ALLOWLIST: &[&str] = &[
    "git",
    "cargo",
    "rustc",
    "rustup",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "node",
    "python",
    "python3",
    "pip",
    "go",
    "make",
    "cmake",
    "ls",
    "dir",
    "cat",
    "type",
    "echo",
    "grep",
    "findstr",
    "rg",
    "head",
    "tail",
    "wc",
    "sort",
    "uniq",
    "bash",
    "sh",
    "cmd",
    "powershell",
    "pwsh",
];

/// Default deny globs applied on top of any user-supplied configuration.
pub const DEFAULT_DENY_PATTERNS: &[&str] = &[
    "**/.env",
    "**/.env.*",
    "**/*.pem",
    "**/*.key",
    "**/id_rsa",
    "**/id_ed25519",
    "**/.aws/credentials",
    "**/.ssh/*",
];

/// Exec allowlist wrapper used by [`Sandbox`].
///
/// Stored as a set of lowercased command stems so lookups are O(1) and
/// case-insensitive across platforms.
#[derive(Debug, Clone, Default)]
pub struct ExecAllowlist {
    inner: HashSet<String>,
}

impl ExecAllowlist {
    /// Construct from an iterator of command names. Each entry is trimmed,
    /// lowercased, and deduplicated.
    pub fn from_names<I, S>(iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let inner = iter
            .into_iter()
            .map(|s| s.as_ref().trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        Self { inner }
    }

    /// Construct from a static slice of strings.
    pub fn from_static(list: &[&'static str]) -> Self {
        Self::from_names(list.iter().copied())
    }

    /// Returns `true` if the given command stem is on the allowlist.
    pub fn allows(&self, command_stem: &str) -> bool {
        self.inner.contains(&command_stem.to_ascii_lowercase())
    }

    /// Number of entries currently on the allowlist.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the allowlist is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Tunable configuration for a [`Sandbox`].
///
/// All fields are owned so the config can be moved freely; the [`Sandbox`]
/// itself borrows nothing.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Root directory under which reads and writes are allowed by default.
    pub workspace_root: PathBuf,
    /// Maximum file size for read/write operations.
    pub max_file_size: u64,
    /// Maximum exec runtime. (Enforced by the caller, not by Sandbox itself.)
    pub max_runtime: Duration,
    /// Optional allowlist of file extensions; if empty, all extensions pass.
    pub allowed_extensions: HashSet<String>,
    /// Deny list of glob matchers; matches are always rejected.
    pub denied_paths: Vec<PathMatcher>,
    /// Exec command allowlist.
    pub exec_allowlist: ExecAllowlist,
    /// Whether to also allow reads from the system temp directory.
    pub allow_system_temp: bool,
}

impl SandboxConfig {
    /// Construct a default config rooted at `workspace_root`.
    ///
    /// Inherits the static [`DEFAULT_EXEC_ALLOWLIST`] and [`DEFAULT_DENY_PATTERNS`].
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        let denied_paths = DEFAULT_DENY_PATTERNS
            .iter()
            .filter_map(|p| PathMatcher::new(*p).ok())
            .collect();
        Self {
            workspace_root: workspace_root.into(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_runtime: DEFAULT_MAX_RUNTIME,
            allowed_extensions: HashSet::new(),
            denied_paths,
            exec_allowlist: ExecAllowlist::from_static(DEFAULT_EXEC_ALLOWLIST),
            allow_system_temp: true,
        }
    }

    /// Builder: override the max file size.
    pub fn with_max_file_size(mut self, size: u64) -> Self {
        self.max_file_size = size;
        self
    }

    /// Builder: override the max exec runtime.
    pub fn with_max_runtime(mut self, runtime: Duration) -> Self {
        self.max_runtime = runtime;
        self
    }

    /// Builder: replace the extension allowlist.
    pub fn with_allowed_extensions<I, S>(mut self, exts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_extensions = exts
            .into_iter()
            .map(|s| {
                let raw = s.into();
                raw.trim_start_matches('.').to_ascii_lowercase()
            })
            .collect();
        self
    }

    /// Builder: append a deny glob.
    pub fn with_deny_pattern(mut self, pattern: impl Into<String>) -> Result<Self, SandboxError> {
        let matcher = PathMatcher::new(pattern)
            .map_err(|e| SandboxError::Denied(format!("invalid deny pattern: {e}")))?;
        self.denied_paths.push(matcher);
        Ok(self)
    }
}

/// The policy enforcer for tool actions.
///
/// Cheap to clone — only holds owned data. Construct one per session and
/// thread it through tool invocations.
#[derive(Debug, Clone)]
pub struct Sandbox {
    config: SandboxConfig,
    /// Cached, normalized (forward-slash) workspace root for cheap comparisons.
    workspace_root_normalized: String,
    /// Cached system temp path, if enabled.
    system_temp: Option<PathBuf>,
}

impl Sandbox {
    /// Construct a [`Sandbox`] with default configuration rooted at `workspace_root`.
    pub fn new(workspace_root: &Path) -> Self {
        Self::with_config(SandboxConfig::new(workspace_root.to_path_buf()))
    }

    /// Construct a [`Sandbox`] from a fully-specified config.
    pub fn with_config(config: SandboxConfig) -> Self {
        let workspace_root_normalized = normalize_path(&config.workspace_root);
        let system_temp = if config.allow_system_temp {
            std::env::temp_dir().into()
        } else {
            None
        };
        Self {
            config,
            workspace_root_normalized,
            system_temp,
        }
    }

    /// Borrow the configuration.
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Borrow the workspace root.
    pub fn workspace_root(&self) -> &Path {
        &self.config.workspace_root
    }

    /// Validate a read against the sandbox policy.
    pub fn validate_read(&self, path: &Path) -> SandboxResult<()> {
        check_no_dotdot(path)?;
        let normalized = normalize_path(path);
        self.check_deny(&normalized)?;
        self.check_extension(&normalized)?;
        if self.is_within_workspace(&normalized) {
            return Ok(());
        }
        if let Some(temp) = &self.system_temp {
            let temp_norm = normalize_path(temp);
            if normalized.starts_with(&temp_norm) {
                return Ok(());
            }
        }
        Err(SandboxError::OutsideWorkspace(normalized))
    }

    /// Validate a write against the sandbox policy. Writes are never allowed
    /// inside the system temp directory even when reads are.
    pub fn validate_write(&self, path: &Path) -> SandboxResult<()> {
        check_no_dotdot(path)?;
        let normalized = normalize_path(path);
        self.check_deny(&normalized)?;
        self.check_extension(&normalized)?;
        if self.is_within_workspace(&normalized) {
            return Ok(());
        }
        Err(SandboxError::OutsideWorkspace(normalized))
    }

    /// Validate an exec command against the sandbox policy.
    ///
    /// The full command string is parsed for argv[0]; only the stem is matched
    /// against the allowlist.
    pub fn validate_exec(&self, command: &str) -> SandboxResult<()> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err(SandboxError::InvalidCommand(command.to_string()));
        }
        let stem = command_stem(trimmed)
            .ok_or_else(|| SandboxError::InvalidCommand(command.to_string()))?;
        let stem_lower = stem.to_ascii_lowercase();
        let stem_name = Path::new(&stem_lower)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&stem_lower);
        if self.config.exec_allowlist.allows(stem_name) {
            Ok(())
        } else {
            Err(SandboxError::ExecDenied(stem.to_string()))
        }
    }

    fn is_within_workspace(&self, normalized: &str) -> bool {
        if normalized == self.workspace_root_normalized {
            return true;
        }
        // Ensure prefix matches a path boundary — not just a substring.
        let root = &self.workspace_root_normalized;
        normalized.starts_with(root) && normalized[root.len()..].starts_with('/')
    }

    fn check_deny(&self, normalized: &str) -> SandboxResult<()> {
        for matcher in &self.config.denied_paths {
            if matcher.matches(normalized) {
                return Err(SandboxError::Denied(normalized.to_string()));
            }
        }
        Ok(())
    }

    fn check_extension(&self, normalized: &str) -> SandboxResult<()> {
        if self.config.allowed_extensions.is_empty() {
            return Ok(());
        }
        let Some(ext) = Path::new(normalized).extension().and_then(|s| s.to_str()) else {
            return Err(SandboxError::ExtensionDenied(normalized.to_string()));
        };
        if self
            .config
            .allowed_extensions
            .contains(&ext.to_ascii_lowercase())
        {
            Ok(())
        } else {
            Err(SandboxError::ExtensionDenied(normalized.to_string()))
        }
    }
}

/// Reject any path whose components contain `..` — this is the most
/// important guard against escapes.
fn check_no_dotdot(path: &Path) -> SandboxResult<()> {
    for comp in path.components() {
        if let std::path::Component::ParentDir = comp {
            return Err(SandboxError::DotDotEscape(path.display().to_string()));
        }
    }
    Ok(())
}

/// Normalize a path to forward-slash form, lowercase on Windows drive letters
/// is *not* applied (case sensitivity is a filesystem concern). This is only
/// for the purposes of prefix comparison.
fn normalize_path(path: &Path) -> String {
    let mut buf = String::with_capacity(path.as_os_str().len() + 1);
    let mut first = true;
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            Prefix(p) => {
                buf.push_str(&p.as_os_str().to_string_lossy());
                first = false;
            }
            RootDir => {
                buf.push('/');
                first = false;
            }
            CurDir => {}
            ParentDir => {
                // Should have been caught already; normalize defensively.
                buf.push_str("../");
            }
            Normal(s) => {
                if !first && !buf.ends_with('/') {
                    buf.push('/');
                }
                buf.push_str(&s.to_string_lossy());
                first = false;
            }
        }
    }
    buf
}

/// Extract argv[0] from a command string. Handles quoting and leading env
/// assignments (`FOO=bar cmd`) defensively.
fn command_stem(command: &str) -> Option<&str> {
    // Strip a leading env assignment segment: `FOO=bar baz` -> `baz`.
    let mut cursor = command;
    while let Some(space) = cursor.find(char::is_whitespace) {
        let head = cursor[..space].trim();
        if head.contains('=') && !head.contains('/') && !head.contains('\\') {
            cursor = cursor[space..].trim_start();
            continue;
        }
        break;
    }
    let trimmed = cursor.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        // Quoted stem — find closing quote.
        rest.split_once('"').map(|(stem, _)| stem)
    } else if let Some(rest) = trimmed.strip_prefix('\'') {
        rest.split_once('\'').map(|(stem, _)| stem)
    } else {
        trimmed.split_whitespace().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:/workspace")
        } else {
            PathBuf::from("/workspace")
        }
    }

    #[test]
    fn allows_read_inside_workspace() {
        let s = Sandbox::new(&ws());
        let p = ws().join("src/main.rs");
        assert!(s.validate_read(&p).is_ok());
    }

    #[test]
    fn rejects_read_outside_workspace() {
        // Use a path that is not the system temp_dir, so the temp allowance
        // doesn't accidentally rescue the test.
        let outside = if cfg!(windows) {
            PathBuf::from("C:/Windows/System32/drivers/etc/hosts")
        } else {
            PathBuf::from("/etc/passwd")
        };
        // Only run this assertion when the system temp dir is not under the
        // outside path (which is always true on supported platforms).
        let temp = std::env::temp_dir();
        if outside.starts_with(&temp) || temp.starts_with(&outside) {
            return;
        }
        let s = Sandbox::new(&ws());
        assert!(matches!(
            s.validate_read(&outside),
            Err(SandboxError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn rejects_dotdot_escape() {
        let s = Sandbox::new(&ws());
        let p = ws().join("../secret.txt");
        assert!(matches!(
            s.validate_read(&p),
            Err(SandboxError::DotDotEscape(_))
        ));
    }

    #[test]
    fn write_allowed_inside_workspace_only() {
        let s = Sandbox::new(&ws());
        let inside = ws().join("out.txt");
        assert!(s.validate_write(&inside).is_ok());

        let outside = std::env::temp_dir().join("mscode_sandbox_should_reject.txt");
        // The system temp allowance is read-only — writes outside workspace
        // must always be denied, even inside temp.
        assert!(matches!(
            s.validate_write(&outside),
            Err(SandboxError::OutsideWorkspace(_))
        ));
    }

    #[test]
    fn exec_allows_known_command() {
        let s = Sandbox::new(&ws());
        assert!(s.validate_exec("git status").is_ok());
        assert!(s.validate_exec("cargo build --release").is_ok());
        assert!(s.validate_exec("npm install").is_ok());
    }

    #[test]
    fn exec_rejects_unknown_command() {
        let s = Sandbox::new(&ws());
        assert!(matches!(
            s.validate_exec("rm -rf /"),
            Err(SandboxError::ExecDenied(_))
        ));
    }

    #[test]
    fn exec_handles_quoted_command() {
        let s = Sandbox::new(&ws());
        assert!(s.validate_exec("\"git\" commit -m hello").is_ok());
        assert!(s.validate_exec("'cargo' fmt").is_ok());
    }

    #[test]
    fn exec_strips_env_assignment_prefix() {
        let s = Sandbox::new(&ws());
        assert!(s.validate_exec("FOO=bar cargo build").is_ok());
        assert!(matches!(
            s.validate_exec("FOO=bar naughty --flag"),
            Err(SandboxError::ExecDenied(_))
        ));
    }

    #[test]
    fn exec_rejects_empty_command() {
        let s = Sandbox::new(&ws());
        assert!(matches!(
            s.validate_exec("   "),
            Err(SandboxError::InvalidCommand(_))
        ));
    }

    #[test]
    fn deny_pattern_rejects_path() {
        let s = Sandbox::new(&ws());
        // Default deny list contains `**/.env*`.
        let p = ws().join(".env");
        assert!(matches!(s.validate_read(&p), Err(SandboxError::Denied(_))));
        let p = ws().join("config/.env.local");
        assert!(matches!(s.validate_read(&p), Err(SandboxError::Denied(_))));
    }

    #[test]
    fn extension_allowlist_is_enforced() {
        let cfg = SandboxConfig::new(ws()).with_allowed_extensions(["rs", "toml"]);
        let s = Sandbox::with_config(cfg);
        assert!(s.validate_read(&ws().join("main.rs")).is_ok());
        assert!(matches!(
            s.validate_read(&ws().join("secret.bin")),
            Err(SandboxError::ExtensionDenied(_))
        ));
    }

    #[test]
    fn exec_allowlist_case_insensitive() {
        let allow = ExecAllowlist::from_static(&["Git"]);
        assert!(allow.allows("git"));
        assert!(allow.allows("GIT"));
        assert!(allow.allows("Git"));
        assert!(!allow.allows("rm"));
    }

    #[test]
    fn builder_with_deny_pattern_appends() {
        let cfg = SandboxConfig::new(ws())
            .with_deny_pattern("**/secrets.json")
            .unwrap();
        let s = Sandbox::with_config(cfg);
        assert!(matches!(
            s.validate_read(&ws().join("secrets.json")),
            Err(SandboxError::Denied(_))
        ));
    }

    #[test]
    fn command_stem_handles_inputs() {
        assert_eq!(command_stem("git status"), Some("git"));
        assert_eq!(command_stem("\"cargo\" build"), Some("cargo"));
        assert_eq!(command_stem("FOO=bar python -V"), Some("python"));
        assert_eq!(command_stem(""), None);
        assert_eq!(command_stem("   "), None);
    }
}
