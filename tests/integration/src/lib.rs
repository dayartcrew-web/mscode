//! Integration test crate for the mscode workspace.
//!
//! This crate depends on multiple workspace crates and exercises
//! end-to-end scenarios: session lifecycle, DAG supervisor, MCP server,
//! CLI binary invocation, and cold-start benchmarks. All tests are
//! hermetic — they never touch the network and always use `tempfile` for
//! filesystem state.

#![allow(clippy::needless_borrow)]
