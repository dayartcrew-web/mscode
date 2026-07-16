//! Apply logic — materialize an [`EditOperation`] against existing file content.

use crate::DEFAULT_FUZZY_THRESHOLD;
use crate::error::{EditFormatError, EditFormatResult};
use crate::fuzzy::FuzzyMatcher;
use crate::types::EditOperation;

/// Apply `op` to `content`, returning the new file content.
///
/// The matcher is supplied by the caller so it can be configured (threshold,
/// future options) without changing this function's signature.
pub fn apply_edit_operation(
    content: &str,
    op: EditOperation,
    matcher: &FuzzyMatcher,
) -> EditFormatResult<String> {
    match op {
        EditOperation::WholeFile { content: new } => Ok(new),
        EditOperation::SearchReplace {
            search,
            replace,
            anchor: _,
        } => apply_single(content, &search, &replace, matcher),
        EditOperation::ToolUseEdit { edits } => {
            let mut acc = content.to_string();
            for e in edits {
                acc = apply_single(&acc, &e.search, &e.replace, matcher)?;
            }
            Ok(acc)
        }
    }
}

fn apply_single(
    content: &str,
    search: &str,
    replace: &str,
    matcher: &FuzzyMatcher,
) -> EditFormatResult<String> {
    let Some(result) = matcher.find_block(content, search, DEFAULT_FUZZY_THRESHOLD) else {
        return Err(EditFormatError::SearchNotFound);
    };
    let range = result.map_err(|a| EditFormatError::AmbiguousMatch { count: a.count })?;
    let mut out = String::with_capacity(content.len() + replace.len());
    out.push_str(&content[..range.start]);
    out.push_str(replace);
    out.push_str(&content[range.end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> FuzzyMatcher {
        FuzzyMatcher::new()
    }

    #[test]
    fn whole_file_replaces_content() {
        let op = EditOperation::WholeFile {
            content: "fresh".into(),
        };
        let new = apply_edit_operation("old", op, &m()).unwrap();
        assert_eq!(new, "fresh");
    }

    #[test]
    fn search_replace_exact_match_swaps_text() {
        let op = EditOperation::SearchReplace {
            search: "world".into(),
            replace: "moon".into(),
            anchor: None,
        };
        let new = apply_edit_operation("hello world", op, &m()).unwrap();
        assert_eq!(new, "hello moon");
    }

    #[test]
    fn search_replace_fuzzy_match_handles_whitespace() {
        let op = EditOperation::SearchReplace {
            search: "fn main() {\n  println!(\"hi\");\n}".into(),
            replace: "fn main() {\n  println!(\"hello\");\n}".into(),
            anchor: None,
        };
        let content = "fn main() {\n    println!(\"hi\");\n}\n";
        let new = apply_edit_operation(content, op, &m()).unwrap();
        assert!(new.contains("hello"));
        assert!(!new.contains("\"hi\""));
    }

    #[test]
    fn search_replace_returns_error_when_not_found() {
        let op = EditOperation::SearchReplace {
            search: "totally missing text".into(),
            replace: "x".into(),
            anchor: None,
        };
        let err = apply_edit_operation("short file", op, &m()).unwrap_err();
        assert_eq!(err, EditFormatError::SearchNotFound);
    }

    #[test]
    fn tool_use_applies_edits_in_order() {
        let op = EditOperation::ToolUseEdit {
            edits: vec![
                crate::types::SearchReplaceEdit {
                    search: "foo".into(),
                    replace: "bar".into(),
                    anchor: None,
                },
                crate::types::SearchReplaceEdit {
                    search: "bar".into(),
                    replace: "baz".into(),
                    anchor: None,
                },
            ],
        };
        let new = apply_edit_operation("foo", op, &m()).unwrap();
        assert_eq!(new, "baz");
    }

    #[test]
    fn tool_use_aborts_on_first_failure() {
        let op = EditOperation::ToolUseEdit {
            edits: vec![
                crate::types::SearchReplaceEdit {
                    search: "foo".into(),
                    replace: "bar".into(),
                    anchor: None,
                },
                crate::types::SearchReplaceEdit {
                    search: "missing".into(),
                    replace: "x".into(),
                    anchor: None,
                },
            ],
        };
        let err = apply_edit_operation("foo", op, &m()).unwrap_err();
        assert_eq!(err, EditFormatError::SearchNotFound);
    }
}
