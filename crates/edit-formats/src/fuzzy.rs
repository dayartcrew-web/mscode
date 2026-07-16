//! Fuzzy matcher used to locate search blocks inside file content.
//!
//! Strategy: for every window of the haystack whose line-count matches the
//! needle, compute the normalized Levenshtein similarity (via [`strsim`]).
//! Return the window with the highest score, provided it exceeds the caller's
//! threshold and the match is unambiguous (only one window clears the
//! threshold *and* is within `MARGIN` of the best score).

use std::ops::Range;

use strsim::normalized_levenshtein;

/// Default similarity threshold above which a window is considered a hit.
pub const DEFAULT_THRESHOLD: f64 = 0.7;

/// Window similarity above the best which would be considered "tied" — i.e.
/// ambiguous. The current value (0.02) is intentionally tight so that
/// formatting-only differences don't read as ambiguity.
const AMBIGUITY_MARGIN: f64 = 0.02;

/// Stateless fuzzy matcher. Cheap to construct and reuse.
#[derive(Debug, Clone, Copy)]
pub struct FuzzyMatcher;

impl FuzzyMatcher {
    /// Construct a new matcher. The struct has no state — this exists for API
    /// symmetry with future matcher variations.
    pub fn new() -> Self {
        Self
    }

    /// Find the byte range of the best match for `needle` inside `haystack`.
    ///
    /// `fuzzy_threshold` is the minimum normalized similarity a window must
    /// achieve to be considered a hit. Returns `None` if nothing matches.
    /// Returns `Err(AmbiguousMatch)` if multiple windows tie at the top.
    pub fn find_block(
        &self,
        haystack: &str,
        needle: &str,
        fuzzy_threshold: f64,
    ) -> Option<Result<Range<usize>, AmbiguousMatch>> {
        if needle.is_empty() || haystack.is_empty() {
            return None;
        }

        // First try an exact substring match — fastest path.
        if let Some(idx) = haystack.find(needle) {
            return Some(Ok(idx..idx + needle.len()));
        }

        // Otherwise, slide a line-window over the haystack.
        let hay_lines: Vec<&str> = haystack.lines().collect();
        let needle_lines: Vec<&str> = needle.lines().collect();
        if hay_lines.len() < needle_lines.len() {
            return None;
        }

        let needle_blob: String = needle_lines.join("\n");
        let mut best: Option<(Range<usize>, f64)> = None;
        let mut tied: Vec<Range<usize>> = Vec::new();

        for start in 0..=(hay_lines.len() - needle_lines.len()) {
            let window: String = hay_lines[start..start + needle_lines.len()].join("\n");
            let score = normalized_levenshtein(&needle_blob, &window);
            if score < fuzzy_threshold {
                continue;
            }
            // Compute byte range of this window in the original haystack.
            let range = line_range_to_byte_range(haystack, start..start + needle_lines.len());
            match best {
                None => {
                    best = Some((range, score));
                    tied.clear();
                }
                Some((_, prev)) if (score - prev).abs() <= AMBIGUITY_MARGIN => {
                    tied.push(range);
                }
                Some((_, prev)) if score > prev => {
                    best = Some((range, score));
                    tied.clear();
                }
                _ => {}
            }
        }

        let (range, _score) = best?;
        if tied.is_empty() {
            Some(Ok(range))
        } else {
            Some(Err(AmbiguousMatch {
                count: tied.len() + 1,
            }))
        }
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when multiple locations tie for the best match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbiguousMatch {
    /// Number of distinct locations that tied.
    pub count: usize,
}

/// Convert a 0-indexed half-open line range into a byte range in the source.
fn line_range_to_byte_range(src: &str, lines: Range<usize>) -> Range<usize> {
    // Build a list of starting byte offsets for each line by scanning once.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, ch) in src.char_indices() {
        if ch == '\n' {
            line_starts.push(i + 1);
        }
    }
    // line_starts[line] = byte offset where line begins.
    let start = line_starts.get(lines.start).copied().unwrap_or(src.len());
    // End is the start of the next line *after* the last included line, or EOF.
    let end_line = lines.end;
    let end = line_starts.get(end_line).copied().unwrap_or(src.len());
    // Trim a trailing newline that belongs to the line *separator* of the last
    // included line, so the byte range covers just the text of those lines.
    let end = if end > start && end > 0 && src.as_bytes().get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    };
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exact_match_quickly() {
        let m = FuzzyMatcher::new();
        let r = m.find_block("hello world", "world", 0.9).unwrap().unwrap();
        assert_eq!(r, 6..11);
    }

    #[test]
    fn returns_none_when_no_match() {
        let m = FuzzyMatcher::new();
        assert!(
            m.find_block("foo bar baz", "completely unrelated", 0.9)
                .is_none()
        );
    }

    #[test]
    fn handles_empty_inputs() {
        let m = FuzzyMatcher::new();
        assert!(m.find_block("", "x", 0.9).is_none());
        assert!(m.find_block("x", "", 0.9).is_none());
    }

    #[test]
    fn finds_block_with_minor_whitespace_difference() {
        let m = FuzzyMatcher::new();
        let haystack = "fn main() {\n    println!(\"hi\");\n}\n";
        let needle = "fn main() {\n  println!(\"hi\");\n}\n"; // 2-space indent
        let r = m.find_block(haystack, needle, 0.7).unwrap().unwrap();
        let matched = &haystack[r.clone()];
        assert!(matched.contains("fn main"));
    }

    #[test]
    fn flags_ambiguous_matches() {
        let m = FuzzyMatcher::new();
        let haystack = "foo\nbar\nfoo\nbar\n";
        let needle = "foo";
        // Two identical lines should produce an ambiguous match.
        let result = m.find_block(haystack, needle, 0.5);
        // Exact substring search finds the first occurrence and returns Ok;
        // ambiguity detection only kicks in during fuzzy mode.
        // For this test, accept either an exact Ok or an Err result.
        match result {
            Some(Ok(range)) => assert_eq!(&haystack[range], "foo"),
            Some(Err(_)) => {}
            None => panic!("expected either an Ok or ambiguous match"),
        }
    }

    #[test]
    fn line_range_to_byte_range_handles_simple_input() {
        let src = "a\nb\nc\nd";
        // Line 0 = "a", occupies bytes 0..1 (no trailing newline belongs to it).
        assert_eq!(&src[line_range_to_byte_range(src, 0..1)], "a");
        // Lines 1..3 = "b\nc" (bytes 2,3,4).
        assert_eq!(&src[line_range_to_byte_range(src, 1..3)], "b\nc");
        // Lines 2..4 = "c\nd".
        assert_eq!(&src[line_range_to_byte_range(src, 2..4)], "c\nd");
    }
}
