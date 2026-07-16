//! Input draft + history buffer.
//!
//! [`MessageBuffer`] is the unit of input state for the TUI. It holds:
//!
//! - the current draft (mutable as the user types),
//! - a bounded history of submitted messages,
//! - a navigation cursor for arrow-key history browsing,
//! - the queue of pending plan-mode messages awaiting approval.
//!
//! The buffer is **pure data + pure functions** — it has no I/O, no async, no
//! terminal awareness. The [`crate::App`] layer drives it.

use std::collections::VecDeque;

/// One row in the message history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The submitted text (newline-normalized).
    pub text: String,
    /// `true` if the entry was queued in plan mode and is awaiting approval.
    pub pending_approval: bool,
}

/// Input draft + bounded history + plan-mode queue.
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    draft: String,
    history: VecDeque<HistoryEntry>,
    /// Navigation cursor: `None` means "editing the live draft", `Some(i)`
    /// means "viewing entry `i` from the tail of `history`" (0 = newest).
    history_cursor: Option<usize>,
    /// Maximum number of entries kept in `history`. Older entries are evicted.
    capacity: usize,
}

impl MessageBuffer {
    /// Construct an empty buffer with the given history capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            draft: String::new(),
            history: VecDeque::with_capacity(cap),
            history_cursor: None,
            capacity: cap,
        }
    }

    /// Current draft text (mutable borrow for callers that want to render it).
    pub fn draft(&self) -> &str {
        &self.draft
    }

    /// Append a character to the draft.
    pub fn push_char(&mut self, c: char) {
        self.draft.push(c);
    }

    /// Append an arbitrary string to the draft (used for paste / multi-key
    /// sequences).
    pub fn push_str(&mut self, s: &str) {
        self.draft.push_str(s);
    }

    /// Remove the last character from the draft. Returns `true` if anything
    /// was removed.
    pub fn backspace(&mut self) -> bool {
        self.draft.pop().is_some()
    }

    /// Replace the draft with the given string.
    pub fn set_draft(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
        self.history_cursor = None;
    }

    /// Queue the current draft as a new line in multi-line entry (Tab). The
    /// draft is reset to empty so the user can type the next line.
    ///
    /// Returns the queued line, or `None` if the draft was empty.
    pub fn tab_queue(&mut self) -> Option<String> {
        if self.draft.is_empty() {
            return None;
        }
        let line = std::mem::take(&mut self.draft);
        // Append a trailing newline so subsequent lines stay visually separated
        // when the user finally submits. The submit step collapses trailing
        // newlines.
        self.draft = String::new();
        // Stash the queued line in the draft itself with a newline separator;
        // the queued text persists in `draft` until Enter submits it.
        // NOTE: We re-append because the queue is *within* the draft.
        self.draft.push_str(&line);
        self.draft.push('\n');
        Some(line)
    }

    /// Submit the current draft as a completed message.
    ///
    /// Returns `None` if the draft was empty (or whitespace only). Otherwise
    /// returns the trimmed message and pushes a new entry into history.
    ///
    /// `pending_approval` is set on the new history entry — when `true`, the
    /// message is queued for plan-mode approval rather than dispatched.
    pub fn submit(&mut self, pending_approval: bool) -> Option<String> {
        let trimmed = self.draft.trim().to_string();
        if trimmed.is_empty() {
            // Still reset the draft so the user starts fresh next time.
            self.draft.clear();
            self.history_cursor = None;
            return None;
        }
        self.push_history(trimmed.clone(), pending_approval);
        self.draft.clear();
        self.history_cursor = None;
        Some(trimmed)
    }

    /// Move the cursor one entry back in history (toward older entries).
    /// Returns the resulting draft, or `None` if already at the oldest entry.
    pub fn history_prev(&mut self) -> Option<&str> {
        if self.history.is_empty() {
            return None;
        }
        let next = self
            .history_cursor
            .map_or(0, |c| (c + 1).min(self.history.len() - 1));
        self.history_cursor = Some(next);
        // Map from tail-index to VecDeque index. Cursor 0 = newest = back().
        let idx = self.history.len() - 1 - next;
        self.history.get(idx).map(|h| h.text.as_str())
    }

    /// Move the cursor one entry forward in history (toward newer entries).
    /// Returns `Some(draft)` when the cursor has returned to the live draft.
    pub fn history_next(&mut self) -> HistoryNav<'_> {
        match self.history_cursor {
            None => HistoryNav::AtDraft,
            Some(0) => {
                self.history_cursor = None;
                HistoryNav::AtDraft
            }
            Some(n) => {
                self.history_cursor = Some(n - 1);
                let idx = self.history.len() - 1 - (n - 1);
                match self.history.get(idx) {
                    Some(h) => HistoryNav::AtEntry(&h.text),
                    None => HistoryNav::AtDraft,
                }
            }
        }
    }

    /// Read-only view over history, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.history.iter()
    }

    /// Number of entries currently in history.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// All entries currently pending plan-mode approval, oldest first.
    pub fn pending(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.history.iter().filter(|h| h.pending_approval)
    }

    /// Approve all pending entries, returning them in chronological order.
    /// Each approved entry has its `pending_approval` flag cleared.
    pub fn approve_pending(&mut self) -> Vec<String> {
        let mut approved = Vec::new();
        for entry in self.history.iter_mut() {
            if entry.pending_approval {
                entry.pending_approval = false;
                approved.push(entry.text.clone());
            }
        }
        approved
    }

    fn push_history(&mut self, text: String, pending_approval: bool) {
        if self.history.len() >= self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(HistoryEntry {
            text,
            pending_approval,
        });
    }
}

/// Outcome of [`MessageBuffer::history_next`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryNav<'a> {
    /// Cursor returned to the live draft.
    AtDraft,
    /// Cursor is pointing at the entry with this text.
    AtEntry(&'a str),
}

impl Default for MessageBuffer {
    fn default() -> Self {
        Self::new(10_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_buffer_appends_on_enter() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("hello world");
        let msg = buf.submit(false).expect("submitted");
        assert_eq!(msg, "hello world");
        assert!(buf.draft().is_empty());
        assert_eq!(buf.history_len(), 1);
    }

    #[test]
    fn submit_ignores_empty_draft() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("   ");
        assert!(buf.submit(false).is_none());
        assert_eq!(buf.history_len(), 0);
    }

    #[test]
    fn message_buffer_tab_queues_multiline() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("line one");
        let queued = buf.tab_queue().expect("tab queue");
        assert_eq!(queued, "line one");
        // Draft retains the queued line + newline so a future Enter sees it.
        assert_eq!(buf.draft(), "line one\n");

        buf.push_str("line two");
        let submitted = buf.submit(false).expect("submit");
        assert_eq!(submitted, "line one\nline two");
    }

    #[test]
    fn tab_queue_on_empty_draft_returns_none() {
        let mut buf = MessageBuffer::new(8);
        assert!(buf.tab_queue().is_none());
    }

    #[test]
    fn message_buffer_arrow_keys_navigate_history() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("first");
        let _ = buf.submit(false);
        buf.push_str("second");
        let _ = buf.submit(false);
        buf.push_str("third");
        let _ = buf.submit(false);

        // From a fresh draft, prev() should walk backward: newest -> oldest.
        let first_nav = buf.history_prev().expect("prev");
        assert_eq!(first_nav, "third");

        let second_nav = buf.history_prev().expect("prev");
        assert_eq!(second_nav, "second");

        let third_nav = buf.history_prev().expect("prev");
        assert_eq!(third_nav, "first");

        // At oldest: cursor stays put.
        let stuck = buf.history_prev().expect("prev");
        assert_eq!(stuck, "first");

        // Walking forward returns to the live draft.
        assert_eq!(buf.history_next(), HistoryNav::AtEntry("second"));
        assert_eq!(buf.history_next(), HistoryNav::AtEntry("third"));
        assert_eq!(buf.history_next(), HistoryNav::AtDraft);
    }

    #[test]
    fn history_prev_on_empty_buffer_returns_none() {
        let mut buf = MessageBuffer::new(8);
        assert!(buf.history_prev().is_none());
    }

    #[test]
    fn history_capped_to_capacity() {
        let mut buf = MessageBuffer::new(3);
        for i in 0..5 {
            buf.push_str(&format!("m{i}"));
            let _ = buf.submit(false);
        }
        assert_eq!(buf.history_len(), 3);
        // Oldest entries evicted; current newest is "m4".
        let newest = buf.history_prev().expect("prev");
        assert_eq!(newest, "m4");
    }

    #[test]
    fn submit_marks_pending_approval_when_requested() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("plan A");
        let _ = buf.submit(true);
        let pending: Vec<_> = buf.pending().map(|h| h.text.clone()).collect();
        assert_eq!(pending, vec!["plan A".to_string()]);
    }

    #[test]
    fn approve_pending_clears_flag() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("plan A");
        let _ = buf.submit(true);
        buf.push_str("plan B");
        let _ = buf.submit(true);
        let approved = buf.approve_pending();
        assert_eq!(approved, vec!["plan A".to_string(), "plan B".to_string()]);
        // No more pending after approval.
        assert_eq!(buf.pending().count(), 0);
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("abc");
        assert!(buf.backspace());
        assert_eq!(buf.draft(), "ab");
        // Backspace on empty draft returns false without panicking.
        buf.draft.clear();
        assert!(!buf.backspace());
    }

    #[test]
    fn set_draft_resets_history_cursor() {
        let mut buf = MessageBuffer::new(8);
        buf.push_str("first");
        let _ = buf.submit(false);
        let _ = buf.history_prev();
        assert!(buf.history_cursor.is_some());
        buf.set_draft("overridden");
        assert!(buf.history_cursor.is_none());
        assert_eq!(buf.draft(), "overridden");
    }
}
