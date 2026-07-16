//! High-level session orchestration for the mscode CLI.
//!
//! This crate sits above [`mscode_rollout`] and [`mscode_protocol`] and
//! provides the in-memory reducer ([`state::SessionState`]) used to rebuild
//! state from a session log, plus [`orchestrator::Orchestrator`] which binds
//! a rollout writer to a state reducer for a single session.

pub mod error;
pub mod orchestrator;
pub mod state;

pub use error::{CoreError, Result};
pub use orchestrator::{Orchestrator, session_log_path};
pub use state::{SessionSnapshot, SessionState, SessionStatus};
