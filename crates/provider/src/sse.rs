//! Minimal incremental Server-Sent Events parser (always compiled).
//!
//! See [`crate::adapters`] for usage notes. This module is feature-agnostic
//! so its unit tests run in every build, including default (no live_tests)
//! builds.

/// A single decoded SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` field value, or `"message"` if absent.
    pub name: String,
    /// Concatenated `data:` lines joined by `\n`.
    pub data: String,
}

/// Incremental SSE parser. Owns a buffer of in-flight bytes that have not
/// yet produced a complete event.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct SseParser {
    buf: String,
    pending_name: Option<String>,
    pending_data: String,
}

impl SseParser {
    /// Construct a fresh parser with an empty buffer.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append raw bytes to the parser. UTF-8 boundaries that span chunks are
    /// handled by appending to a `String`; in practice SSE responses are
    /// ASCII-framed with UTF-8 payloads delivered in complete chunks.
    #[cfg_attr(not(feature = "live_tests"), allow(dead_code))]
    pub fn feed(&mut self, bytes: &[u8]) {
        if let Ok(s) = std::str::from_utf8(bytes) {
            self.buf.push_str(s);
        }
    }

    /// Pull the next complete event out of the buffer, if one is ready.
    #[cfg_attr(not(feature = "live_tests"), allow(dead_code))]
    pub fn next_event(&mut self) -> Option<SseEvent> {
        while let Some(idx) = self.buf.find('\n') {
            let mut line: String = self.buf.drain(..=idx).collect();
            // Trim a trailing CR if present (CRLF framing).
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }

            // Blank line terminates an event.
            if line.is_empty() {
                if self.pending_data.is_empty() && self.pending_name.is_none() {
                    continue;
                }
                let name = self
                    .pending_name
                    .take()
                    .unwrap_or_else(|| "message".to_owned());
                let data = std::mem::take(&mut self.pending_data);
                // Drop a single trailing newline (multi-line data joins on \n).
                let data = data
                    .strip_suffix('\n')
                    .map(|s| s.to_owned())
                    .unwrap_or(data);
                return Some(SseEvent { name, data });
            }

            // Comment.
            if line.starts_with(':') {
                continue;
            }

            let (field, value) = match line.find(':') {
                Some(i) => {
                    let (f, rest) = line.split_at(i);
                    let mut v = &rest[1..];
                    if let Some(stripped) = v.strip_prefix(' ') {
                        v = stripped;
                    }
                    (f.to_owned(), v.to_owned())
                }
                None => (line.clone(), String::new()),
            };

            match field.as_str() {
                "event" => self.pending_name = Some(value),
                "data" => {
                    self.pending_data.push_str(&value);
                    self.pending_data.push('\n');
                }
                "id" | "retry" => { /* ignored */ }
                _ => { /* unknown field — ignore */ }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_complete_event() {
        let mut p = SseParser::new();
        p.feed(b"event: hi\ndata: {\"x\":1}\n\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.name, "hi");
        assert_eq!(ev.data, "{\"x\":1}");
        assert!(p.next_event().is_none());
    }

    #[test]
    fn coalesces_multiline_data() {
        let mut p = SseParser::new();
        p.feed(b"data: line1\ndata: line2\n\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.data, "line1\nline2");
    }

    #[test]
    fn defaults_event_name_to_message() {
        let mut p = SseParser::new();
        p.feed(b"data: hello\n\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.name, "message");
        assert_eq!(ev.data, "hello");
    }

    #[test]
    fn handles_crlf_framing() {
        let mut p = SseParser::new();
        p.feed(b"data: hello\r\n\r\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.data, "hello");
    }

    #[test]
    fn ignores_comments_and_unknown_fields() {
        let mut p = SseParser::new();
        p.feed(b": a comment\nevent: real\nid: 42\nretry: 1000\ndata: ok\n\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.name, "real");
        assert_eq!(ev.data, "ok");
    }

    #[test]
    fn incremental_chunks_produce_one_event() {
        let mut p = SseParser::new();
        p.feed(b"event: ");
        assert!(p.next_event().is_none());
        p.feed(b"hi\n");
        assert!(p.next_event().is_none());
        p.feed(b"data: payload\n\n");
        let ev = p.next_event().expect("event");
        assert_eq!(ev.name, "hi");
        assert_eq!(ev.data, "payload");
    }

    #[test]
    fn yields_multiple_events_in_order() {
        let mut p = SseParser::new();
        p.feed(b"data: one\n\ndata: two\n\n");
        assert_eq!(p.next_event().unwrap().data, "one");
        assert_eq!(p.next_event().unwrap().data, "two");
        assert!(p.next_event().is_none());
    }

    #[test]
    fn empty_data_field_is_dropped() {
        let mut p = SseParser::new();
        // Two blank lines in a row with no preceding data should not produce
        // a phantom event.
        p.feed(b"\n\n");
        assert!(p.next_event().is_none());
    }
}
