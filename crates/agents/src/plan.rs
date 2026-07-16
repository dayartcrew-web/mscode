//! [`Plan`], [`Critique`], [`CritiqueDecision`] — the structured payloads
//! exchanged between the Planner, Executor, and Critic.
//!
//! These types intentionally derive `Serialize + Deserialize` so they can
//! be parsed from an LLM-produced JSON value via the [`crate::Extract`]
//! trait, AND persisted into the rollout journal for replay.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single step in a [`Plan`]: the handler label to invoke and the input
/// payload to pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Handler name (must match a registered [`mscode_exec::NodeHandler`]).
    pub label: String,
    /// Input payload (references, not large blobs).
    #[serde(default)]
    pub inputs: Value,
}

impl PlanStep {
    /// Construct a step with the given label and null inputs.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            inputs: Value::Null,
        }
    }

    /// Replace the inputs.
    #[must_use]
    pub fn with_inputs(mut self, inputs: Value) -> Self {
        self.inputs = inputs;
        self
    }
}

/// Output of the Planner: an ordered list of steps plus the expected
/// output shape (used by the Critic to validate results).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Ordered list of steps. The supervisor executes them in order, marking
    /// each on the DAG.
    pub steps: Vec<PlanStep>,
    /// Free-text description of the plan goal.
    #[serde(default)]
    pub goal: String,
    /// Expected output shape (JSON schema fragment). Optional — the critic
    /// may treat absence as "any value is acceptable".
    #[serde(default)]
    pub expected_output: Value,
}

impl Plan {
    /// Construct an empty plan with the given goal.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            steps: Vec::new(),
            goal: goal.into(),
            expected_output: Value::Null,
        }
    }

    /// Append a step.
    pub fn with_step(mut self, step: PlanStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Number of steps in the plan.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Returns `true` if the plan has no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Critic's verdict on the executor's results.
///
/// Serialized as externally-tagged JSON:
/// `"accept"`, `{"reflect": "feedback"}`, `{"reject": "reason"}`. The
/// [`crate::critic`] module parses the LLM's `{"kind": ...}` wire format
/// into this type via an intermediate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CritiqueDecision {
    /// Results are good; the supervisor should return them.
    Accept,
    /// Results need improvement; retry with the given feedback.
    Reflect(String),
    /// Results are fundamentally wrong; abort.
    Reject(String),
}

impl CritiqueDecision {
    /// Returns `true` if `Accept`.
    pub fn is_accept(&self) -> bool {
        matches!(self, Self::Accept)
    }

    /// Returns `true` if `Reflect`.
    pub fn is_reflect(&self) -> bool {
        matches!(self, Self::Reflect(_))
    }
}

/// Full critique: decision + reasoning text (useful for logs and for the
/// planner's next iteration when reflecting).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Critique {
    /// The verdict.
    pub decision: CritiqueDecision,
    /// Free-text reasoning from the critic. Always non-empty.
    pub reasoning: String,
}

impl Critique {
    /// Construct an Accept critique.
    pub fn accept(reasoning: impl Into<String>) -> Self {
        Self {
            decision: CritiqueDecision::Accept,
            reasoning: reasoning.into(),
        }
    }

    /// Construct a Reflect critique with feedback.
    pub fn reflect(feedback: impl Into<String>, reasoning: impl Into<String>) -> Self {
        Self {
            decision: CritiqueDecision::Reflect(feedback.into()),
            reasoning: reasoning.into(),
        }
    }

    /// Construct a Reject critique with a reason.
    pub fn reject(reason: impl Into<String>, reasoning: impl Into<String>) -> Self {
        Self {
            decision: CritiqueDecision::Reject(reason.into()),
            reasoning: reasoning.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_step_new_uses_null_inputs() {
        let s = PlanStep::new("fetch");
        assert_eq!(s.label, "fetch");
        assert_eq!(s.inputs, Value::Null);
    }

    #[test]
    fn plan_step_with_inputs_overrides() {
        let s = PlanStep::new("fetch").with_inputs(json!({"u": "x"}));
        assert_eq!(s.inputs, json!({"u": "x"}));
    }

    #[test]
    fn plan_builder_chain() {
        let p = Plan::new("do thing")
            .with_step(PlanStep::new("a"))
            .with_step(PlanStep::new("b"));
        assert_eq!(p.len(), 2);
        assert!(!p.is_empty());
        assert_eq!(p.steps[0].label, "a");
        assert_eq!(p.steps[1].label, "b");
        assert_eq!(p.goal, "do thing");
    }

    #[test]
    fn plan_empty_when_no_steps() {
        let p = Plan::new("nothing");
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn plan_round_trips_through_json() {
        let p = Plan::new("goal").with_step(PlanStep::new("a").with_inputs(json!({"k": 1})));
        let v = serde_json::to_value(&p).unwrap();
        let back: Plan = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn critique_accept_constructor() {
        let c = Critique::accept("looks good");
        assert!(matches!(c.decision, CritiqueDecision::Accept));
        assert_eq!(c.reasoning, "looks good");
        assert!(c.decision.is_accept());
    }

    #[test]
    fn critique_reflect_constructor() {
        let c = Critique::reflect("add error handling", "missing err path");
        match &c.decision {
            CritiqueDecision::Reflect(fb) => assert_eq!(fb, "add error handling"),
            other => panic!("expected Reflect, got {other:?}"),
        }
        assert!(c.decision.is_reflect());
    }

    #[test]
    fn critique_reject_constructor() {
        let c = Critique::reject("bad plan", "wrong tool");
        match c.decision {
            CritiqueDecision::Reject(r) => assert_eq!(r, "bad plan"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn critique_round_trips_through_json() {
        let c = Critique::reflect("fb", "rs");
        let v = serde_json::to_value(&c).unwrap();
        let back: Critique = serde_json::from_value(v).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn critique_decision_serializes_as_snake_case_tag() {
        let v = serde_json::to_value(CritiqueDecision::Accept).unwrap();
        assert_eq!(v, serde_json::json!("accept"));
        let v = serde_json::to_value(CritiqueDecision::Reflect("fb".into())).unwrap();
        assert_eq!(v, serde_json::json!({"reflect": "fb"}));
        let v = serde_json::to_value(CritiqueDecision::Reject("r".into())).unwrap();
        assert_eq!(v, serde_json::json!({"reject": "r"}));
    }

    #[test]
    fn critique_decision_is_helpers() {
        assert!(CritiqueDecision::Accept.is_accept());
        assert!(!CritiqueDecision::Accept.is_reflect());
        assert!(CritiqueDecision::Reflect("x".into()).is_reflect());
        assert!(!CritiqueDecision::Reflect("x".into()).is_accept());
    }
}
