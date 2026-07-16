//! Public types for edit-format parsing.

use crate::error::EditFormatResult;

/// A single search-replace edit, used both standalone and as an item inside
/// a tool-use batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SearchReplaceEdit {
    /// Text to locate (may match fuzzily).
    pub search: String,
    /// Replacement text.
    pub replace: String,
    /// Optional anchor (e.g. nearby unique line) used to disambiguate when
    /// `search` appears multiple times.
    #[serde(default)]
    pub anchor: Option<String>,
}

/// The structure of a parsed `tool_use` edit call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ToolUseEditParams {
    /// Path of the file to edit. The apply step uses this only as a sanity
    /// check — it does not perform file I/O itself.
    pub path: String,
    /// One or more search/replace edits to apply in order.
    pub edits: Vec<SearchReplaceEdit>,
}

/// The parsed JSON envelope of a tool-use edit call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ToolUseEditCall {
    /// Tool name, conventionally `edit` or `apply_edit`.
    pub name: String,
    /// Parameters of the call.
    pub parameters: ToolUseEditParams,
}

/// A parsed edit operation. The apply step knows how to materialize each
/// variant against an existing file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOperation {
    /// A single search/replace block (optionally with an anchor).
    SearchReplace {
        /// Text to locate.
        search: String,
        /// Replacement text.
        replace: String,
        /// Optional anchor.
        anchor: Option<String>,
    },
    /// Replace the entire file with the supplied content.
    WholeFile {
        /// New full content.
        content: String,
    },
    /// A batch of edits delivered via a tool-use call.
    ToolUseEdit {
        /// Edits to apply, in order.
        edits: Vec<SearchReplaceEdit>,
    },
}

/// Trait implemented by every supported edit format.
pub trait EditFormatParser: Send + Sync {
    /// Parse the model output into an [`EditOperation`].
    fn parse(&self, model_output: &str) -> EditFormatResult<EditOperation>;
    /// Stable identifier for this parser (e.g. `"search-replace"`).
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_operation_is_debug_clone() {
        let op = EditOperation::WholeFile {
            content: "hi".into(),
        };
        let cloned = op.clone();
        assert_eq!(op, cloned);
        assert!(format!("{op:?}").contains("WholeFile"));
    }

    #[test]
    fn tool_use_edit_call_round_trips() {
        let call = ToolUseEditCall {
            name: "edit".into(),
            parameters: ToolUseEditParams {
                path: "a.rs".into(),
                edits: vec![SearchReplaceEdit {
                    search: "old".into(),
                    replace: "new".into(),
                    anchor: None,
                }],
            },
        };
        let json = serde_json::to_string(&call).unwrap();
        let back: ToolUseEditCall = serde_json::from_str(&json).unwrap();
        assert_eq!(call, back);
    }
}
