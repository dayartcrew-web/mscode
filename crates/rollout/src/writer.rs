//! Append-only JSONL writer for session rollout files.
//!
//! Mirrors codex-rs's `codex-rollout` approach: open the file with append
//! mode, write one JSON event per line, flush after each write. Crash
//! recovery is the reader's responsibility — a partially written final line
//! is detected (missing trailing newline + invalid JSON) and truncated.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use mscode_protocol::SessionEvent;

use crate::error::Result;

/// Append-only writer for a session rollout log.
///
/// One [`RolloutWriter`] owns one file handle. Writes are line-buffered and
/// flushed eagerly after each event. Call [`RolloutWriter::flush`] to force
/// an fsync if durability stronger than the OS page cache is required.
pub struct RolloutWriter {
    inner: BufWriter<File>,
    path: std::path::PathBuf,
}

impl RolloutWriter {
    /// Create (or open for append) a rollout file at `path`.
    ///
    /// Parent directories are NOT created automatically — callers are
    /// responsible for ensuring the directory exists so this function remains
    /// a thin wrapper over `OpenOptions`.
    pub fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(false)
            .truncate(false)
            .open(path)?;
        Ok(Self {
            inner: BufWriter::new(file),
            path: path.to_path_buf(),
        })
    }

    /// Append a single event to the log as one JSON line.
    ///
    /// Flushes the buffered writer after each event so that an abnormal
    /// process termination leaves at most one partial line on disk (which
    /// the reader will recover from on next open).
    pub fn append(&mut self, event: &SessionEvent) -> Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.inner.write_all(&line)?;
        self.inner.flush()?;
        Ok(())
    }

    /// Flush buffered data and fsync the underlying file to disk.
    ///
    /// Use this when crash-safety matters more than throughput. Calling
    /// `append` already flushes the buffer to the OS page cache; this method
    /// additionally asks the kernel to durably persist the bytes.
    pub fn flush(&mut self) -> Result<()> {
        self.inner.flush()?;
        // fsync via the underlying file handle. `BufWriter::get_ref` gives
        // us the wrapped File without consuming the writer.
        self.inner.get_ref().sync_all()?;
        Ok(())
    }

    /// Path the writer was opened against.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for RolloutWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RolloutWriter")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mscode_protocol::SessionId;
    use tempfile::tempdir;

    fn started_event() -> SessionEvent {
        SessionEvent::SessionStarted {
            id: SessionId::new(),
            cwd: std::path::PathBuf::from("/tmp"),
            project_root: None,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn create_and_append_round_trips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let event = started_event();

        {
            let mut writer = RolloutWriter::create(&path).expect("create");
            writer.append(&event).expect("append");
        }

        let bytes = std::fs::read_to_string(&path).expect("read");
        assert_eq!(bytes.matches('\n').count(), 1, "exactly one line");
        let parsed: SessionEvent = serde_json::from_str(bytes.trim()).expect("parse");
        assert_eq!(parsed, event);
    }

    #[test]
    fn append_three_events_writes_three_lines() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let events = [started_event(), started_event(), started_event()];

        {
            let mut writer = RolloutWriter::create(&path).expect("create");
            for ev in &events {
                writer.append(ev).expect("append");
            }
        }

        let bytes = std::fs::read_to_string(&path).expect("read");
        assert_eq!(bytes.matches('\n').count(), 3);
    }

    #[test]
    fn create_is_append_only() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");

        let first = started_event();
        let second = started_event();

        {
            let mut writer = RolloutWriter::create(&path).expect("create");
            writer.append(&first).expect("append 1");
        }
        {
            // Re-open the same path — should NOT truncate.
            let mut writer = RolloutWriter::create(&path).expect("reopen");
            writer.append(&second).expect("append 2");
        }

        let bytes = std::fs::read_to_string(&path).expect("read");
        assert_eq!(bytes.matches('\n').count(), 2, "file should have two lines");
    }

    #[test]
    fn flush_does_not_error_on_valid_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut writer = RolloutWriter::create(&path).expect("create");
        writer.append(&started_event()).expect("append");
        writer.flush().expect("flush");
    }

    #[test]
    fn debug_repr_includes_path() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let writer = RolloutWriter::create(&path).expect("create");
        let s = format!("{writer:?}");
        assert!(s.contains("s.jsonl"));
    }

    #[test]
    fn path_accessor_returns_opened_path() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let writer = RolloutWriter::create(&path).expect("create");
        assert_eq!(writer.path(), path);
    }
}
