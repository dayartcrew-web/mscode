//! State-machine modes for the TUI.
//!
//! These are split into two enums:
//!
//! - [`InputMode`] — what the user is currently typing into (free text,
//!   command filter, or nothing).
//! - [`PlanMode`] — whether the agent runtime is allowed to execute or the
//!   user is still composing a plan.

/// What the input box is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Not typing — arrow keys navigate history.
    #[default]
    Normal,
    /// Free-text entry — keys are appended to the draft.
    Insert,
    /// The user typed `/` and is filtering the slash-command list.
    SlashCommand,
}

/// Whether the agent runtime is allowed to execute.
///
/// In [`PlanMode::Planning`], submitting a message does NOT call the executor
/// — the message is queued for approval. This is the "plan mode gates
/// execution" invariant the hard constraints call out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanMode {
    /// Default state: messages are queued for review and not auto-executed.
    #[default]
    Planning,
    /// User has approved — Enter submits to the agent runtime.
    Executing,
}

impl PlanMode {
    /// Returns `true` when in [`PlanMode::Planning`].
    pub fn is_planning(self) -> bool {
        matches!(self, PlanMode::Planning)
    }

    /// Returns `true` when in [`PlanMode::Executing`].
    pub fn is_executing(self) -> bool {
        matches!(self, PlanMode::Executing)
    }
}

impl InputMode {
    /// Returns `true` when in [`InputMode::Insert`].
    pub fn is_insert(self) -> bool {
        matches!(self, InputMode::Insert)
    }

    /// Returns `true` when in [`InputMode::SlashCommand`].
    pub fn is_slash_command(self) -> bool {
        matches!(self, InputMode::SlashCommand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_mode_default_is_normal() {
        assert_eq!(InputMode::default(), InputMode::Normal);
    }

    #[test]
    fn plan_mode_default_is_planning() {
        // Per the hard constraints, plan mode gates execution by default.
        assert_eq!(PlanMode::default(), PlanMode::Planning);
    }

    #[test]
    fn plan_mode_predicate_helpers() {
        assert!(PlanMode::Planning.is_planning());
        assert!(!PlanMode::Planning.is_executing());
        assert!(PlanMode::Executing.is_executing());
        assert!(!PlanMode::Executing.is_planning());
    }

    #[test]
    fn input_mode_predicate_helpers() {
        assert!(InputMode::Insert.is_insert());
        assert!(!InputMode::Normal.is_insert());
        assert!(InputMode::SlashCommand.is_slash_command());
        assert!(!InputMode::Normal.is_slash_command());
    }
}
