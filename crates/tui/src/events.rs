//! Event types consumed by the [`crate::App`] event loop.
//!
//! [`TuiEvent`] is the union of crossterm key events, internal ticks, resize
//! notifications, and out-of-band events from the agent runtime. Keeping these
//! in one enum lets the event loop be a single `match`.

/// An external event produced by the agent runtime or another subsystem,
/// delivered via the TUI's out-of-band channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEvent {
    /// A new message arrived from the agent.
    AgentMessage(String),
    /// A plugin emitted a status update.
    PluginStatus { name: String, status: String },
    /// The user's compaction request finished.
    CompactionDone,
}

/// Events the TUI event loop can react to.
///
/// `KeyEvent` is intentionally a thin wrapper around the crossterm type so the
/// rest of the state machine can stay backend-agnostic in tests (we never need
/// to construct raw `crossterm::event::KeyEvent` in unit tests — we drive the
/// state machine through public methods on [`crate::App`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiEvent {
    /// A keyboard event arrived. The inner value is the crossterm key.
    KeyEvent(crossterm::event::KeyEvent),
    /// Periodic tick fired by the event-loop timer.
    Tick,
    /// The terminal was resized.
    Resize(u16, u16),
    /// An external event arrived via the out-of-band channel.
    External(ExternalEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn external_event_equality() {
        let a = ExternalEvent::AgentMessage("hi".into());
        let b = ExternalEvent::AgentMessage("hi".into());
        assert_eq!(a, b);

        let c = ExternalEvent::AgentMessage("bye".into());
        assert_ne!(a, c);
    }

    #[test]
    fn tui_event_key_wraps_crossterm() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let ev = TuiEvent::KeyEvent(key);
        match ev {
            TuiEvent::KeyEvent(k) => assert_eq!(k.code, KeyCode::Enter),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn tui_event_resize_carries_dims() {
        let ev = TuiEvent::Resize(80, 24);
        assert_eq!(ev, TuiEvent::Resize(80, 24));
    }

    #[test]
    fn tui_event_external_wraps_inner() {
        let ev = TuiEvent::External(ExternalEvent::CompactionDone);
        match ev {
            TuiEvent::External(ExternalEvent::CompactionDone) => {}
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
