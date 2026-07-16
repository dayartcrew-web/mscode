//! Session protocol types for the mscode CLI.
//!
//! This crate owns the canonical event schema written to session rollout logs.
//! It deliberately has no I/O: events are pure data, and the [`crate::event`]
//! module is the only authority on what shapes can appear on disk.
//!
//! ## Forward compatibility
//!
//! [`event::SessionEvent`] is serialized with an internal `type` tag. Unknown
//! tags (variants introduced by newer producers) are preserved verbatim in
//! [`event::SessionEvent::Unknown`], so older readers never drop data.

pub mod event;
pub mod ids;
pub mod message;

pub use event::{Checkpoint, SessionEndReason, SessionEvent};
pub use ids::{CheckpointId, MessageId, SessionId, ToolCallId};
pub use message::{ContentBlock, MessageContent, Role, SessionMessage};
