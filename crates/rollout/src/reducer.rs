//! The reducer abstraction used by [`crate::reader::RolloutReader::replay`].
//!
//! Implementors own whatever mutable state needs updating as events stream in.
//! The trait is deliberately synchronous: rollout I/O is sync, and reducers
//! should never perform blocking work that benefits from async.

use mscode_protocol::SessionEvent;
use std::fmt::Debug;

use crate::error::Result;

/// Trait implemented by anything that can fold session events into state.
///
/// Equivalent in spirit to a reducer in event-sourcing terminology: given the
/// current state and an event, produce the next state in place.
pub trait StateReducer: Debug {
    /// Apply a single event to the reducer.
    ///
    /// Implementations should mutate `self` and return `Ok(())` on success.
    /// Returning `Err` aborts replay and propagates upward.
    fn apply_event(&mut self, event: &SessionEvent) -> Result<()>;
}
