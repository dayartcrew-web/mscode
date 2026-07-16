//! Append-only JSONL event log for the mscode CLI.
//!
//! This crate provides a tiny, hand-rolled equivalent of codex-rs's
//! `codex-rollout`. There are no durable-execution framework dependencies —
//! just `std::fs`, serde, and a reducer trait.
//!
//! ## Crash safety
//!
//! Writes go through a `BufWriter` that is flushed eagerly after each event.
//! Callers that need stronger durability (fsync) should call
//! [`writer::RolloutWriter::flush`]. If a process is killed mid-write, the
//! next reader detects the malformed trailing line and trims it back to the
//! last complete event (see [`reader::RolloutReader::open`]).

pub mod error;
pub mod reader;
pub mod reducer;
pub mod writer;

pub use error::{Result, RolloutError};
pub use reader::RolloutReader;
pub use reducer::StateReducer;
pub use writer::RolloutWriter;
