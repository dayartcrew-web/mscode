//! Multi-agent quartet: [`Supervisor`] + [`Planner`] + [`Critic`] + the
//! [`mscode_exec::Executor`] wrapper.
//!
//! ## Architecture
//!
//! ```text
//!                   ┌──────────────┐
//!                   │  Supervisor  │  (orchestrator)
//!                   └──────┬───────┘
//!                          │
//!         ┌────────────────┼────────────────┐
//!         │                │                │
//!         ▼                ▼                ▼
//!   ┌──────────┐    ┌────────────┐    ┌──────────┐
//!   │ Planner  │ -> │  Executor  │ -> │  Critic  │
//!   └──────────┘    └────────────┘    └──────────┘
//!         ▲                              │
//!         └─────── reflect feedback ────┘
//! ```
//!
//! The supervisor runs `plan → execute → critique` in a loop, with a
//! hardcoded cap of [`MAX_REFLECTIONS`] iterations. The cap is NOT
//! configurable — see [`supervisor`] module docs for the reasoning.
//!
//! ## Cold start
//!
//! Construction is O(1) per agent: just stores an `Arc<dyn LlmProvider>` and
//! a model name. No LLM calls happen until the first `plan`/`critique`
//! invocation. This honors the sub-200ms cold-start budget.
//!
//! ## Local-first
//!
//! All agents talk to the provider through [`mscode_provider::LlmProvider`].
//! The mock provider works offline; real HTTP adapters are gated behind the
//! `live_tests` feature in `mscode-provider`.

pub mod critic;
pub mod error;
pub mod extract;
pub mod plan;
pub mod planner;
pub mod supervisor;

pub use critic::Critic;
pub use error::{AgentError, AgentResult, PromptError};
pub use extract::{Extract, extract_from_str};
pub use plan::{Critique, CritiqueDecision, Plan, PlanStep};
pub use planner::Planner;
pub use supervisor::{MAX_REFLECTIONS, Supervisor, TurnOutcome};
