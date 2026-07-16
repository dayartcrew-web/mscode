//! Test 7: Edit-format parsers are deterministic and produce errors (not
//! panics) on malformed input.
//!
//! Exercises the three concrete parsers and the fallback "ladder" pattern
//! (try search-replace, fall back to whole-file on parse failure).

use mscode_edit_formats::{
    EditFormatError, EditFormatParser, EditOperation, SearchReplaceParser, ToolUseEditParser,
    WholeFileParser,
};

const SEARCH_REPLACE_INPUT: &str = "\
<<<SEARCH
fn main() {
    println!(\"hi\");
}
<<<REPLACE
fn main() {
    println!(\"hello\");
}
<<<END
";

#[test]
fn edit_formats_deterministic_ladder() {
    // --- SearchReplaceParser ---
    let sr = SearchReplaceParser::new();
    assert_eq!(sr.name(), "search-replace");

    let op = sr.parse(SEARCH_REPLACE_INPUT).expect("parse ok");
    let op_clone = op.clone();
    let EditOperation::SearchReplace {
        search,
        replace,
        anchor,
    } = op
    else {
        panic!("expected SearchReplace variant");
    };
    assert!(search.contains("println!(\"hi\")"));
    assert!(replace.contains("println!(\"hello\")"));
    assert!(anchor.is_none(), "default parser yields no anchor");

    // Determinism: parse twice, get the same output.
    let op2 = sr.parse(SEARCH_REPLACE_INPUT).expect("parse ok 2");
    assert_eq!(op_clone, op2, "parser must be deterministic");

    // Malformed input is an Err, NOT a panic.
    let bad = sr.parse("nothing useful here");
    assert!(bad.is_err(), "malformed search-replace must error");
    match bad.unwrap_err() {
        EditFormatError::Parse { parser, .. } => assert_eq!(parser, "search-replace"),
        other => panic!("expected Parse error, got {other:?}"),
    }

    // --- WholeFileParser ---
    let wf = WholeFileParser::new();
    assert_eq!(wf.name(), "whole-file");
    let op = wf.parse("just text\nno markers").expect("parse ok");
    let EditOperation::WholeFile { content } = op else {
        panic!("expected WholeFile variant");
    };
    assert_eq!(content, "just text\nno markers");

    // WholeFileParser is total — never errors. Useful as the ladder fallback.
    let op2 = wf.parse("").expect("empty whole-file ok");
    assert!(matches!(op2, EditOperation::WholeFile { .. }));

    // --- ToolUseEditParser ---
    let tu = ToolUseEditParser::new();
    assert_eq!(tu.name(), "tool-use-edit");

    let good_json = serde_json::json!({
        "name": "edit",
        "parameters": {
            "path": "src/main.rs",
            "edits": [
                {"search": "old", "replace": "new"}
            ]
        }
    })
    .to_string();
    let op = tu.parse(&good_json).expect("parse ok");
    let EditOperation::ToolUseEdit { edits } = op else {
        panic!("expected ToolUseEdit variant");
    };
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].search, "old");
    assert_eq!(edits[0].replace, "new");

    // Malformed JSON is an Err, not a panic.
    let bad = tu.parse("not even json");
    assert!(bad.is_err());

    // Empty edits array is rejected.
    let empty_edits = serde_json::json!({
        "name": "edit",
        "parameters": {"path": "x", "edits": []}
    })
    .to_string();
    assert!(tu.parse(&empty_edits).is_err());

    // --- Fallback ladder: try search-replace, on failure use whole-file ---
    fn parse_with_fallback(input: &str) -> EditOperation {
        let sr = SearchReplaceParser::new();
        let wf = WholeFileParser::new();
        sr.parse(input)
            .unwrap_or_else(|_| wf.parse(input).expect("whole-file is total"))
    }

    // Real search-replace input survives the ladder.
    let ladder_sr = parse_with_fallback(SEARCH_REPLACE_INPUT);
    assert!(
        matches!(ladder_sr, EditOperation::SearchReplace { .. }),
        "search-replace input must not fall through"
    );

    // Non-search-replace input falls through to WholeFile.
    let ladder_fallback = parse_with_fallback("just text");
    assert!(
        matches!(ladder_fallback, EditOperation::WholeFile { .. }),
        "non-search-replace input must fall through to whole-file"
    );
}
