//! Replay-side reader for session rollout logs.
//!
//! The reader is crash-tolerant: if the final line of a log file is malformed
//! (e.g. the producer was killed mid-write), [`RolloutReader::open`] will
//! truncate the file back to the last good newline and log a warning. This
//! mirrors the recovery semantics documented by codex-rs's codex-rollout.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use mscode_protocol::SessionEvent;
use tracing::warn;

use crate::error::{Result, RolloutError};
use crate::reducer::StateReducer;

/// Reader for a session rollout log.
///
/// Open with [`RolloutReader::open`]; iterate with [`RolloutReader::iter`] or
/// fold into a reducer with [`RolloutReader::replay`].
pub struct RolloutReader {
    path: PathBuf,
}

impl RolloutReader {
    /// Open a rollout file for reading, repairing a truncated final line if
    /// necessary.
    ///
    /// ## Truncation recovery
    ///
    /// If the last line of the file is missing its trailing newline AND
    /// cannot be parsed as JSON, the file is truncated back to the byte
    /// position of the last good newline. This is the only mutation the
    /// reader performs — append-only history is otherwise preserved.
    pub fn open(path: &Path) -> Result<Self> {
        repair_truncated_tail(path)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Iterate over all events in the log, in append order.
    ///
    /// Each item is a `Result<SessionEvent, RolloutError>` so callers can
    /// decide whether to abort or skip on parse errors. The iterator owns
    /// the file handle for its lifetime.
    pub fn iter(&self) -> impl Iterator<Item = Result<SessionEvent>> + '_ {
        EventIter {
            _path: self.path.clone(),
            lines: match OpenOptions::new().read(true).open(&self.path) {
                Ok(file) => EventLines::Open {
                    lines: BufReader::new(file).lines(),
                    idx: 0,
                },
                // A missing file is equivalent to an empty event stream — the
                // caller may create it later. Surface any other I/O error.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => EventLines::Empty,
                Err(e) => EventLines::Failed(Some(RolloutError::Io(e))),
            },
        }
    }

    /// Replay the entire log into a state reducer.
    ///
    /// Iterates all events in order; returns the first error encountered.
    /// Empty files are a no-op.
    pub fn replay<S: StateReducer>(&self, reducer: &mut S) -> Result<()> {
        for item in self.iter() {
            let event = item?;
            reducer.apply_event(&event)?;
        }
        Ok(())
    }

    /// Path this reader was opened against.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Hand-rolled iterator type that supports a one-shot failure when the file
/// cannot be opened. `iter()` returns this directly so callers can drive it
/// with `for` loops and `?`-propagate parse errors.
struct EventIter {
    _path: PathBuf,
    lines: EventLines,
}

enum EventLines {
    Open {
        lines: std::io::Lines<BufReader<File>>,
        idx: usize,
    },
    Failed(Option<RolloutError>),
    Empty,
}

impl Iterator for EventIter {
    type Item = Result<SessionEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.lines {
                EventLines::Empty => break None,
                EventLines::Failed(slot) => break slot.take().map(Err),
                EventLines::Open { lines, idx } => match lines.next() {
                    None => break None,
                    Some(Err(e)) => break Some(Err(RolloutError::Io(e))),
                    Some(Ok(raw)) => {
                        let line_no = *idx + 1;
                        *idx += 1;
                        let trimmed = raw.trim();
                        if trimmed.is_empty() {
                            // Skip blank lines without surfacing them.
                            continue;
                        }
                        match serde_json::from_str::<SessionEvent>(trimmed) {
                            Ok(ev) => break Some(Ok(ev)),
                            Err(source) => {
                                break Some(Err(RolloutError::Parse {
                                    line: line_no,
                                    source,
                                }));
                            }
                        }
                    }
                },
            }
        }
    }
}

impl std::fmt::Debug for RolloutReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RolloutReader")
            .field("path", &self.path)
            .finish()
    }
}

/// Detect a truncated final line and trim it back to the last full line.
///
/// Detection rule: read the file as bytes. If it is non-empty AND does not
/// end with `\n`, attempt to parse the trailing bytes (after the last `\n`)
/// as JSON. If parsing fails, truncate the file back to the byte position
/// just after the last `\n` and log a warning.
fn repair_truncated_tail(path: &Path) -> Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(RolloutError::Io(e)),
    };
    if bytes.is_empty() {
        return Ok(());
    }
    // Already terminated cleanly — nothing to do.
    if bytes.last() == Some(&b'\n') {
        return Ok(());
    }
    // Find the last newline; everything after it is the suspect tail.
    let last_newline = bytes.iter().rposition(|&b| b == b'\n');
    let tail_start = match last_newline {
        Some(idx) => idx + 1,
        None => 0,
    };
    let tail = &bytes[tail_start..];
    let tail_str = match std::str::from_utf8(tail) {
        Ok(s) => s.trim(),
        Err(_) => {
            warn!(
                path = %path.display(),
                "truncated non-utf8 tail detected; repairing"
            );
            truncate_to(path, tail_start)?;
            return Ok(());
        }
    };
    if tail_str.is_empty() {
        return Ok(());
    }
    let probe: std::result::Result<serde_json::Value, _> = serde_json::from_str(tail_str);
    if probe.is_ok() {
        // The tail is valid JSON even though it lacks a trailing newline.
        // Treat it as a complete event — leave the file alone so the next
        // writer's append will start on a fresh line.
        return Ok(());
    }
    warn!(
        path = %path.display(),
        bytes = tail.len(),
        "truncated final line detected; trimming back to last complete event"
    );
    truncate_to(path, tail_start)?;
    Ok(())
}

fn truncate_to(path: &Path, len: usize) -> Result<()> {
    let f = OpenOptions::new().write(true).open(path)?;
    f.set_len(len as u64)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mscode_protocol::{SessionEndReason, SessionEvent, SessionId};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn write_raw(path: &Path, contents: &str) {
        std::fs::write(path, contents).expect("write");
    }

    fn started_event() -> SessionEvent {
        SessionEvent::SessionStarted {
            id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            project_root: None,
            timestamp: Utc::now(),
        }
    }

    #[derive(Debug, Default)]
    struct Counter {
        count: usize,
    }

    impl StateReducer for Counter {
        fn apply_event(&mut self, _event: &SessionEvent) -> Result<()> {
            self.count += 1;
            Ok(())
        }
    }

    #[test]
    fn empty_file_is_no_op_for_replay() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("empty.jsonl");
        write_raw(&path, "");
        let reader = RolloutReader::open(&path).expect("open");
        let mut counter = Counter::default();
        reader.replay(&mut counter).expect("replay");
        assert_eq!(counter.count, 0);
    }

    #[test]
    fn iter_returns_events_in_order() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let e2 = started_event();
        let lines = format!(
            "{}\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e2).unwrap()
        );
        write_raw(&path, &lines);

        let reader = RolloutReader::open(&path).expect("open");
        let collected: Vec<SessionEvent> = reader.iter().map(|r| r.unwrap()).collect();
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0], e1);
        assert_eq!(collected[1], e2);
    }

    #[test]
    fn replay_applies_all_events_to_reducer() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let e2 = started_event();
        let e3 = SessionEvent::SessionEnded {
            reason: SessionEndReason::Stopped,
            timestamp: Utc::now(),
        };
        let lines = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e2).unwrap(),
            serde_json::to_string(&e3).unwrap()
        );
        write_raw(&path, &lines);

        let reader = RolloutReader::open(&path).expect("open");
        let mut counter = Counter::default();
        reader.replay(&mut counter).expect("replay");
        assert_eq!(counter.count, 3);
    }

    #[test]
    fn truncated_final_line_is_repaired() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let line1 = serde_json::to_string(&e1).unwrap();
        // First line is well-formed; second line is junk with no newline.
        write_raw(&path, &format!("{line1}\n{{\"type\":\"bogus\""));
        let reader = RolloutReader::open(&path).expect("open");
        let mut counter = Counter::default();
        reader.replay(&mut counter).expect("replay");
        assert_eq!(counter.count, 1, "only the well-formed event survives");
        let repaired = std::fs::read_to_string(&path).expect("read");
        assert!(
            repaired.ends_with('\n'),
            "repaired file ends with a newline"
        );
    }

    #[test]
    fn truncated_final_line_with_partial_json_is_repaired() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let line1 = serde_json::to_string(&e1).unwrap();
        write_raw(&path, &format!("{line1}\n{{\"type\":\"session_started\","));
        let reader = RolloutReader::open(&path).expect("open");
        let events: Vec<SessionEvent> = reader.iter().map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn missing_file_is_treated_as_empty() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("never_existed.jsonl");
        // Should not error — caller may create it later.
        let reader = RolloutReader::open(&path).expect("open");
        let mut counter = Counter::default();
        reader.replay(&mut counter).expect("replay");
        assert_eq!(counter.count, 0);
    }

    #[test]
    fn well_formed_final_line_without_newline_is_preserved() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let line1 = serde_json::to_string(&e1).unwrap();
        // Valid JSON but no trailing newline — the reader should not strip it.
        write_raw(&path, &line1);
        let reader = RolloutReader::open(&path).expect("open");
        let events: Vec<SessionEvent> = reader.iter().map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn blank_lines_are_skipped_silently() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let e1 = started_event();
        let line1 = serde_json::to_string(&e1).unwrap();
        write_raw(&path, &format!("{line1}\n\n\n"));
        let reader = RolloutReader::open(&path).expect("open");
        let events: Vec<SessionEvent> = reader.iter().map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn path_accessor_returns_opened_path() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        write_raw(&path, "");
        let reader = RolloutReader::open(&path).expect("open");
        assert_eq!(reader.path(), path);
    }
}
