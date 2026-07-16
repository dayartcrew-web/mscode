//! Edit-format parsers for the mscode CLI.
//!
//! When a model proposes an edit to a file, its raw text output has to be
//! converted into a structured [`EditOperation`] that the runtime can apply.
//! This crate owns that conversion. Three formats are supported:
//!
//! | Format                | [`EditFormatParser`]      | When to use                       |
//! |-----------------------|---------------------------|-----------------------------------|
//! | `<<<SEARCH/REPLACE`   | [`SearchReplaceParser`]   | Default — broad model support     |
//! | Whole-file replacement| [`WholeFileParser`]       | Fallback when blocks are unparseable |
//! | Tool-use JSON         | [`ToolUseEditParser`]     | Preferred when model supports tools|
//!
//! udiff and apply_patch are intentionally **not** implemented, per the
//! synthesis decision — they trade parser complexity for accuracy gains the
//! fuzzy matcher already recovers.
//!
//! All parsers share [`FuzzyMatcher`] for locating the search block in the
//! existing file content; the matcher uses [`strsim`]'s normalized Levenshtein
//! distance so minor whitespace or formatting differences still produce a hit.

pub mod apply;
pub mod error;
pub mod fuzzy;
pub mod parsers;
pub mod types;

pub use apply::apply_edit_operation;
pub use error::{EditFormatError, EditFormatResult};
pub use fuzzy::FuzzyMatcher;
pub use parsers::{SearchReplaceParser, ToolUseEditParser, WholeFileParser};
pub use types::{
    EditFormatParser, EditOperation, SearchReplaceEdit, ToolUseEditCall, ToolUseEditParams,
};

/// Re-export the fuzzy threshold used as the default in matchers.
pub const DEFAULT_FUZZY_THRESHOLD: f64 = 0.7;
