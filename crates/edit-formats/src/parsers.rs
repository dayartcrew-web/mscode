//! Concrete parsers for the three supported edit formats.

use crate::error::{EditFormatError, EditFormatResult};
use crate::types::{EditFormatParser, EditOperation, SearchReplaceEdit, ToolUseEditCall};

/// Marker for the search block start in the default format.
pub const SEARCH_MARKER: &str = "<<<SEARCH";
/// Marker for the replace block start.
pub const REPLACE_MARKER: &str = "<<<REPLACE";
/// Marker terminating a single search-replace triple.
pub const END_MARKER: &str = "<<<END";

/// Default format: triple-delimited `<<<SEARCH / <<<REPLACE / <<<END`.
///
/// Example:
///
/// ```text
/// <<<SEARCH
/// fn main() {
///     println!("hi");
/// }
/// <<<REPLACE
/// fn main() {
///     println!("hello");
/// }
/// <<<END
/// ```
pub struct SearchReplaceParser;

impl SearchReplaceParser {
    /// Construct a new parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SearchReplaceParser {
    fn default() -> Self {
        Self::new()
    }
}

impl EditFormatParser for SearchReplaceParser {
    fn name(&self) -> &'static str {
        "search-replace"
    }

    fn parse(&self, model_output: &str) -> EditFormatResult<EditOperation> {
        let anchor: Option<String> = None;
        let mut search: Option<String> = None;
        let mut replace: Option<String> = None;
        let mut cursor = model_output;

        while !cursor.is_empty() {
            // Skip forward to the next marker.
            let next = cursor.find("<<<");
            let Some(at) = next else { break };
            let after_marker = &cursor[at..];
            // Identify which marker this is.
            let (consumed, body) = match (
                after_marker.find(SEARCH_MARKER),
                after_marker.find(REPLACE_MARKER),
                after_marker.find(END_MARKER),
            ) {
                (Some(0), _, _) => consume_block(cursor, at, SEARCH_MARKER, REPLACE_MARKER),
                (_, Some(0), _) => consume_block(cursor, at, REPLACE_MARKER, END_MARKER),
                (_, _, Some(0)) => {
                    // END marker — advance past it; we may be done or see a new SEARCH.
                    cursor = &cursor[at + END_MARKER.len()..];
                    continue;
                }
                _ => {
                    // Some other `<<<` — skip past it to avoid infinite loop.
                    cursor = &cursor[at + 3..];
                    continue;
                }
            };
            let _ = consumed;
            let body = body.trim_matches('\n');
            match (search.is_none(), replace.is_none(), anchor.is_none()) {
                (true, true, _) => search = Some(body.to_string()),
                (false, true, _) => replace = Some(body.to_string()),
                // Anything beyond SEARCH+REPLACE in a single triple is treated
                // as an anchor for the *next* edit; we don't support that here.
                _ => {}
            }
            // Move the cursor past the consumed block.
            cursor = &cursor[at..];
            // Advance past the current SEARCH/REPLACE marker so the next
            // iteration looks for the matching terminator.
            if let Some(idx) = cursor.find('\n') {
                cursor = &cursor[idx + 1..];
            } else {
                break;
            }
        }

        // Optional leading anchor block (`<<<ANCHOR`) is rare; ignored here.

        let search = search.ok_or_else(|| EditFormatError::Parse {
            parser: "search-replace",
            reason: "no SEARCH block found".into(),
        })?;
        let replace = replace.ok_or_else(|| EditFormatError::Parse {
            parser: "search-replace",
            reason: "no REPLACE block found".into(),
        })?;

        Ok(EditOperation::SearchReplace {
            search,
            replace,
            anchor,
        })
    }
}

/// Pull the body between two markers. Returns (cursor_advance, body).
fn consume_block<'a>(
    cursor: &'a str,
    start_at: usize,
    start_marker: &str,
    end_marker: &str,
) -> (usize, &'a str) {
    let body_start = start_at + start_marker.len();
    let body_end = cursor[body_start..]
        .find(end_marker)
        .map(|i| body_start + i)
        .unwrap_or(cursor.len());
    (body_end + end_marker.len(), &cursor[body_start..body_end])
}

/// Whole-file replacement parser. The entire model output is treated as the
/// new file content.
pub struct WholeFileParser;

impl WholeFileParser {
    /// Construct a new parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for WholeFileParser {
    fn default() -> Self {
        Self::new()
    }
}

impl EditFormatParser for WholeFileParser {
    fn name(&self) -> &'static str {
        "whole-file"
    }

    fn parse(&self, model_output: &str) -> EditFormatResult<EditOperation> {
        Ok(EditOperation::WholeFile {
            content: model_output.to_string(),
        })
    }
}

/// Tool-use JSON parser. Expects a JSON document matching [`ToolUseEditCall`].
pub struct ToolUseEditParser;

impl ToolUseEditParser {
    /// Construct a new parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolUseEditParser {
    fn default() -> Self {
        Self::new()
    }
}

impl EditFormatParser for ToolUseEditParser {
    fn name(&self) -> &'static str {
        "tool-use-edit"
    }

    fn parse(&self, model_output: &str) -> EditFormatResult<EditOperation> {
        let trimmed = model_output.trim();
        let call: ToolUseEditCall = serde_json::from_str(trimmed)
            .map_err(|e| EditFormatError::InvalidToolUse(format!("json parse error: {e}")))?;
        if call.parameters.edits.is_empty() {
            return Err(EditFormatError::InvalidToolUse(
                "tool-use edit batch is empty".into(),
            ));
        }
        // Sanity-check each edit has a non-empty search field.
        for (i, e) in call.parameters.edits.iter().enumerate() {
            if e.search.is_empty() {
                return Err(EditFormatError::InvalidToolUse(format!(
                    "edit {i} has empty search field"
                )));
            }
        }
        let edits: Vec<SearchReplaceEdit> = call.parameters.edits;
        Ok(EditOperation::ToolUseEdit { edits })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_replace_parses_single_block() {
        let input = "\
<<<SEARCH
foo
<<<REPLACE
bar
<<<END
";
        let op = SearchReplaceParser::new().parse(input).unwrap();
        match op {
            EditOperation::SearchReplace {
                search, replace, ..
            } => {
                assert_eq!(search.trim(), "foo");
                assert_eq!(replace.trim(), "bar");
            }
            _ => panic!("expected SearchReplace"),
        }
    }

    #[test]
    fn search_replace_rejects_missing_replace() {
        let input = "<<<SEARCH\nfoo\n<<<END\n";
        let err = SearchReplaceParser::new().parse(input).unwrap_err();
        assert!(matches!(err, EditFormatError::Parse { .. }));
    }

    #[test]
    fn search_replace_rejects_missing_search() {
        let input = "<<<REPLACE\nbar\n<<<END\n";
        let err = SearchReplaceParser::new().parse(input).unwrap_err();
        assert!(matches!(err, EditFormatError::Parse { .. }));
    }

    #[test]
    fn whole_file_returns_full_content() {
        let input = "entire file goes here\n";
        let op = WholeFileParser::new().parse(input).unwrap();
        match op {
            EditOperation::WholeFile { content } => assert_eq!(content, input),
            _ => panic!("expected WholeFile"),
        }
    }

    #[test]
    fn tool_use_parses_valid_payload() {
        let input = serde_json::json!({
            "name": "edit",
            "parameters": {
                "path": "a.rs",
                "edits": [
                    {"search": "old", "replace": "new", "anchor": null}
                ]
            }
        })
        .to_string();
        let op = ToolUseEditParser::new().parse(&input).unwrap();
        match op {
            EditOperation::ToolUseEdit { edits } => {
                assert_eq!(edits.len(), 1);
                assert_eq!(edits[0].search, "old");
                assert_eq!(edits[0].replace, "new");
            }
            _ => panic!("expected ToolUseEdit"),
        }
    }

    #[test]
    fn tool_use_rejects_invalid_json() {
        let err = ToolUseEditParser::new().parse("not json").unwrap_err();
        assert!(matches!(err, EditFormatError::InvalidToolUse(_)));
    }

    #[test]
    fn tool_use_rejects_empty_edit_list() {
        let input = serde_json::json!({
            "name": "edit",
            "parameters": { "path": "a.rs", "edits": [] }
        })
        .to_string();
        let err = ToolUseEditParser::new().parse(&input).unwrap_err();
        assert!(matches!(err, EditFormatError::InvalidToolUse(_)));
    }

    #[test]
    fn tool_use_rejects_empty_search_field() {
        let input = serde_json::json!({
            "name": "edit",
            "parameters": {
                "path": "a.rs",
                "edits": [ {"search": "", "replace": "x"} ]
            }
        })
        .to_string();
        let err = ToolUseEditParser::new().parse(&input).unwrap_err();
        assert!(matches!(err, EditFormatError::InvalidToolUse(_)));
    }

    #[test]
    fn parser_names_are_stable() {
        assert_eq!(SearchReplaceParser::new().name(), "search-replace");
        assert_eq!(WholeFileParser::new().name(), "whole-file");
        assert_eq!(ToolUseEditParser::new().name(), "tool-use-edit");
    }
}
