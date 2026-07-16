//! Extractor pattern for structured LLM output.
//!
//! Borrowed from `rig`'s `SubmitTool<T>` mechanism: instead of trying to
//! parse a free-form completion into a typed struct with ad-hoc code, the
//! LLM is asked to emit JSON matching a known schema, and the
//! [`Extract`] trait is the single narrowing point that turns the raw
//! [`Value`] into a typed result.
//!
//! We do NOT depend on `rig` itself — this is a re-implementation of the
//! useful idea, kept local so the dependency surface stays small and the
//! cold-start budget is preserved.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{AgentError, AgentResult};

/// Trait for types that can be extracted from an LLM-produced JSON value.
///
/// Implementations typically just call [`serde_json::from_value`], but the
/// trait lets callers swap in custom validation (e.g. rejecting empty
/// plans, normalizing casing) at the boundary.
pub trait Extract: Sized {
    /// Extract `Self` from a JSON value produced by the LLM.
    fn extract(value: Value) -> AgentResult<Self>;
}

/// Blanket implementation for any `DeserializeOwned` type. Covers the
/// overwhelming majority of cases — most types just want
/// `serde_json::from_value`.
impl<T: DeserializeOwned> Extract for T {
    fn extract(value: Value) -> AgentResult<Self> {
        serde_json::from_value(value)
            .map_err(|e| AgentError::Llm(format!("failed to extract structured output: {e}")))
    }
}

/// Convenience: extract from a JSON string (as returned by the provider's
/// `MessageContent::Text`).
pub fn extract_from_str<T: Extract>(s: &str) -> AgentResult<T> {
    let value: Value = serde_json::from_str(s)
        .map_err(|e| AgentError::Llm(format!("response was not valid JSON: {e}")))?;
    T::extract(value)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Sample {
        name: String,
        count: u32,
    }

    #[test]
    fn extract_trait_parses_valid_json() {
        let v = serde_json::json!({"name": "x", "count": 7});
        let s: Sample = Sample::extract(v).unwrap();
        assert_eq!(
            s,
            Sample {
                name: "x".into(),
                count: 7
            }
        );
    }

    #[test]
    fn extract_trait_fails_on_invalid_json() {
        let v = serde_json::json!({"name": "x"}); // missing count
        let result: Result<Sample, _> = Sample::extract(v);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)));
    }

    #[test]
    fn extract_from_str_parses_valid_json_string() {
        let s = r#"{"name": "y", "count": 3}"#;
        let parsed: Sample = extract_from_str(s).unwrap();
        assert_eq!(parsed.name, "y");
        assert_eq!(parsed.count, 3);
    }

    #[test]
    fn extract_from_str_fails_on_non_json() {
        let s = "not even close to json";
        let result: Result<Sample, _> = extract_from_str(s);
        assert!(result.is_err());
    }

    #[test]
    fn extract_works_for_primitive_types() {
        // u32 is DeserializeOwned so the blanket impl applies.
        let v = serde_json::json!(42);
        let n: u32 = u32::extract(v).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn extract_handles_array_value() {
        let v = serde_json::json!([1, 2, 3]);
        let arr: Vec<i32> = Vec::<i32>::extract(v).unwrap();
        assert_eq!(arr, vec![1, 2, 3]);
    }
}
