//! Model picker state machine for the in-TUI `/models` slash command.
//!
//! Mirrors [`crate::login_prompt::ProviderPicker`] but with typed
//! [`ModelItem`]s that carry catalog metadata (context window, tool support).
//! Like the login picker, all behavior lives in plain Rust and is unit-tested
//! without a real TTY; the renderer in [`crate::render`] is a thin projection.
//!
//! # Empty catalog
//!
//! An empty `items` slice is a valid state — it represents "no credentials
//! configured". The renderer surfaces a "run `mscode login add`" hint rather
//! than a blank list, and [`ModelPickerEffect::Pick`] is suppressed (the key
//! routing in [`crate::app`] falls back to `Cancel` so the user is never stuck
//! on a non-actionable screen).
//!
//! # Fuzzy match
//!
//! Reuses [`crate::login_prompt::fuzzy_match`] against a search target built
//! from `provider_id`, `model_id`, and `display_label`. So a query of
//! `"gpt` matches both `OpenAI / GPT-5` and `OpenAI / GPT-4` regardless of
//! display-name case.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::login_prompt::fuzzy_match;

/// One row in the model picker. Built by the binary from
/// `mscode_provider::ModelsCatalog` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelItem {
    /// Catalog provider id, e.g. `"openai"`.
    pub provider_id: String,
    /// Catalog model id, e.g. `"gpt-5-codex"`.
    pub model_id: String,
    /// Display label, e.g. `"OpenAI / GPT-5-Codex"`.
    pub display_label: String,
    /// Maximum context tokens, when known. Surfaced in the picker row.
    pub context_window: Option<u64>,
    /// `true` when the model accepts tool/function calls.
    pub supports_tools: bool,
}

impl ModelItem {
    /// Lowercased search target. Includes provider id, model id, and display
    /// label so a query for `gpt` matches `OpenAI / GPT-5` regardless of
    /// which field the user is thinking of.
    pub fn search_target(&self) -> String {
        format!(
            "{} {} {}",
            self.provider_id, self.model_id, self.display_label
        )
        .to_lowercase()
    }
}

/// What the event loop should do after the picker consumes a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPickerEffect {
    /// Continue rendering — query or cursor may have changed.
    Continue,
    /// Tear down without selecting (Esc / Ctrl-C, or empty list).
    Cancel,
    /// Tear down and emit a `Model { name: "{provider}/{model}" }` command.
    Pick,
}

/// Picker state. Tracks the source list, current fuzzy query, the cached
/// filtered view, and the cursor into that view.
#[derive(Debug, Clone)]
pub struct ModelPicker {
    items: Vec<ModelItem>,
    query: String,
    /// Cursor into `filtered`. `None` only when `filtered` is empty.
    cursor: Option<usize>,
    /// Cached indices into `items` matching `query`. Recomputed on each query
    /// change so render can stay pure.
    filtered: Vec<usize>,
}

impl ModelPicker {
    /// Construct from owned items. Empty `items` is valid (see module docs).
    pub fn new(items: Vec<ModelItem>) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        let cursor = if filtered.is_empty() { None } else { Some(0) };
        Self {
            items,
            query: String::new(),
            cursor,
            filtered,
        }
    }

    /// Read-only view of source items.
    pub fn items(&self) -> &[ModelItem] {
        &self.items
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Indices into [`items()`] that match the current query, in display order.
    pub fn filtered_indices(&self) -> &[usize] {
        &self.filtered
    }

    /// Cursor position into [`filtered_indices()`]. `None` when empty.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Currently-highlighted item, if any.
    pub fn selected(&self) -> Option<&ModelItem> {
        self.cursor
            .and_then(|i| self.filtered.get(i))
            .and_then(|&idx| self.items.get(idx))
    }

    /// Append a character to the query and recompute the filtered view.
    pub fn push_query_char(&mut self, c: char) {
        self.query.push(c);
        self.recompute_filtered();
    }

    /// Drop the last character of the query (no-op when empty).
    pub fn pop_query_char(&mut self) {
        if self.query.pop().is_some() {
            self.recompute_filtered();
        }
    }

    /// Replace the entire query (used by tests and Ctrl-U clear).
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.recompute_filtered();
    }

    fn recompute_filtered(&mut self) {
        let q = self.query.trim();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| fuzzy_match(&item.search_target(), q))
            .map(|(i, _)| i)
            .collect();
        self.cursor = if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    /// Move cursor up. No-op at top.
    pub fn move_up(&mut self) {
        if let Some(i) = self.cursor {
            if i > 0 {
                self.cursor = Some(i - 1);
            }
        }
    }

    /// Move cursor down. No-op at bottom.
    pub fn move_down(&mut self) {
        if let Some(i) = self.cursor {
            if i + 1 < self.filtered.len() {
                self.cursor = Some(i + 1);
            }
        }
    }

    /// Dispatch a crossterm key event. Returns the effect the app should
    /// observe (continue / cancel / pick).
    ///
    /// Ctrl-C and Esc both cancel. Enter picks the highlighted row, or
    /// cancels when the list is empty (so the user is never stuck).
    pub fn handle_key(&mut self, key: KeyEvent) -> ModelPickerEffect {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return ModelPickerEffect::Cancel;
        }
        match key.code {
            KeyCode::Esc => ModelPickerEffect::Cancel,
            KeyCode::Up => {
                self.move_up();
                ModelPickerEffect::Continue
            }
            KeyCode::Down => {
                self.move_down();
                ModelPickerEffect::Continue
            }
            KeyCode::Enter => {
                if self.selected().is_some() {
                    ModelPickerEffect::Pick
                } else {
                    // Empty list — treat as cancel so user is not stuck.
                    ModelPickerEffect::Cancel
                }
            }
            KeyCode::Backspace => {
                self.pop_query_char();
                ModelPickerEffect::Continue
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_query("");
                ModelPickerEffect::Continue
            }
            KeyCode::Char(c) => {
                self.push_query_char(c);
                ModelPickerEffect::Continue
            }
            _ => ModelPickerEffect::Continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_items() -> Vec<ModelItem> {
        vec![
            ModelItem {
                provider_id: "openai".into(),
                model_id: "gpt-5".into(),
                display_label: "OpenAI / GPT-5".into(),
                context_window: Some(400_000),
                supports_tools: true,
            },
            ModelItem {
                provider_id: "openai".into(),
                model_id: "gpt-5-codex".into(),
                display_label: "OpenAI / GPT-5-Codex".into(),
                context_window: Some(400_000),
                supports_tools: true,
            },
            ModelItem {
                provider_id: "anthropic".into(),
                model_id: "claude-sonnet-4-6".into(),
                display_label: "Anthropic / Claude Sonnet 4.6".into(),
                context_window: Some(1_000_000),
                supports_tools: true,
            },
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn starts_with_all_items_visible_and_cursor_at_top() {
        let p = ModelPicker::new(sample_items());
        assert_eq!(p.items().len(), 3);
        assert_eq!(p.filtered_indices().len(), 3);
        assert_eq!(p.cursor(), Some(0));
    }

    #[test]
    fn empty_items_is_valid_initial_state() {
        let p = ModelPicker::new(Vec::new());
        assert!(p.items().is_empty());
        assert!(p.filtered_indices().is_empty());
        assert!(p.cursor().is_none());
    }

    #[test]
    fn query_filters_by_subsequence_across_fields() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("gpt");
        let visible: Vec<&str> = p
            .filtered_indices()
            .iter()
            .map(|&i| p.items()[i].model_id.as_str())
            .collect();
        assert_eq!(visible, vec!["gpt-5", "gpt-5-codex"]);
    }

    #[test]
    fn query_matches_provider_id() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("anthropic");
        let visible: Vec<&str> = p
            .filtered_indices()
            .iter()
            .map(|&i| p.items()[i].provider_id.as_str())
            .collect();
        assert_eq!(visible, vec!["anthropic"]);
    }

    #[test]
    fn query_no_matches_clears_cursor() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("zzzz");
        assert!(p.filtered_indices().is_empty());
        assert!(p.cursor().is_none());
    }

    #[test]
    fn cursor_navigation_clamps_at_top_and_bottom() {
        let mut p = ModelPicker::new(sample_items());
        assert_eq!(p.cursor(), Some(0));
        p.move_up();
        assert_eq!(p.cursor(), Some(0));
        p.move_down();
        assert_eq!(p.cursor(), Some(1));
        p.move_down();
        assert_eq!(p.cursor(), Some(2));
        p.move_down();
        assert_eq!(p.cursor(), Some(2));
    }

    #[test]
    fn cursor_resets_to_top_after_query_change() {
        let mut p = ModelPicker::new(sample_items());
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), Some(2));
        p.set_query("gpt");
        assert_eq!(p.cursor(), Some(0));
    }

    #[test]
    fn backspace_pops_one_query_char() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("gpt");
        p.pop_query_char();
        assert_eq!(p.query(), "gp");
        p.pop_query_char();
        p.pop_query_char();
        assert_eq!(p.query(), "");
        // No-op on empty.
        p.pop_query_char();
        assert_eq!(p.query(), "");
    }

    #[test]
    fn ctrl_u_clears_query() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("gpt");
        let _ = p.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(p.query(), "");
    }

    #[test]
    fn enter_on_highlighted_row_returns_pick() {
        let mut p = ModelPicker::new(sample_items());
        let effect = p.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, ModelPickerEffect::Pick);
        // Cursor unchanged so caller can still read selected().
        assert_eq!(p.selected().map(|i| i.model_id.as_str()), Some("gpt-5"));
    }

    #[test]
    fn enter_after_navigation_picks_correct_row() {
        let mut p = ModelPicker::new(sample_items());
        p.move_down();
        p.move_down();
        let effect = p.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, ModelPickerEffect::Pick);
        assert_eq!(
            p.selected().map(|i| i.model_id.as_str()),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn esc_returns_cancel() {
        let mut p = ModelPicker::new(sample_items());
        assert_eq!(p.handle_key(key(KeyCode::Esc)), ModelPickerEffect::Cancel);
    }

    #[test]
    fn ctrl_c_returns_cancel() {
        let mut p = ModelPicker::new(sample_items());
        assert_eq!(
            p.handle_key(ctrl(KeyCode::Char('c'))),
            ModelPickerEffect::Cancel
        );
    }

    #[test]
    fn enter_on_empty_list_returns_cancel_not_pick() {
        let mut p = ModelPicker::new(Vec::new());
        assert_eq!(p.handle_key(key(KeyCode::Enter)), ModelPickerEffect::Cancel);
    }

    #[test]
    fn enter_on_filtered_out_list_returns_cancel() {
        let mut p = ModelPicker::new(sample_items());
        p.set_query("zzzz");
        assert!(p.cursor().is_none());
        assert_eq!(p.handle_key(key(KeyCode::Enter)), ModelPickerEffect::Cancel);
    }

    #[test]
    fn typing_accumulates_into_query() {
        let mut p = ModelPicker::new(sample_items());
        p.handle_key(key(KeyCode::Char('g')));
        p.handle_key(key(KeyCode::Char('p')));
        p.handle_key(key(KeyCode::Char('t')));
        assert_eq!(p.query(), "gpt");
        assert_eq!(p.filtered_indices().len(), 2);
    }

    #[test]
    fn search_target_is_lowercase_across_all_fields() {
        let item = ModelItem {
            provider_id: "OpenAI".into(),
            model_id: "GPT-5".into(),
            display_label: "OpenAI / GPT-5".into(),
            context_window: None,
            supports_tools: false,
        };
        assert!(item.search_target().contains("openai"));
        assert!(item.search_target().contains("gpt-5"));
    }
}
