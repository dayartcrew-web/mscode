//! Multi-step login wizard state machines for `mscode login add`.
//!
//! Logic-first, just like [`crate::session_list`]: all interesting behavior
//! lives in plain Rust state structs that can be unit-tested without a real
//! TTY. The renderer in [`crate::login_render`] is a thin projection.
//!
//! # Wizard flow
//!
//! 1. **Provider** — fuzzy-searchable list of [`PickerItem`]s built from
//!    [`mscode_credentials::PROVIDER_CATALOG`]. A sentinel `Custom provider…`
//!    row lets the user enter a `custom:<name>` id.
//! 2. **Label** — freeform text input (ascii alphanumeric + `-_.`).
//! 3. **Secret** — masked text input (renders `*` per char).
//!
//! The wizard returns `(provider_id, label, secret)` on completion or `None`
//! if the user cancels (Esc twice / Ctrl-C). Callers fall back to the legacy
//! text-prompt path when stdout is not a TTY (see `mscode login add` wiring).
//!
//! # Fuzzy match
//!
//! [`fuzzy_match`] is a simple case-insensitive subsequence check. With ~95
//! catalog entries this is O(N) per keystroke and the user-visible latency is
//! dominated by terminal redraw, not scoring. We avoid pulling in a fuzzy
//! crate to keep the binary lean.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One row in the provider picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// Catalog id (`openai`, `anthropic`, …) or sentinel value for the
    /// custom-provider entry.
    pub id: String,
    /// Human-readable name shown in the list (`OpenAI`, `Anthropic`, …).
    pub display_name: String,
    /// Endpoint preview shown alongside the name. `None` for the custom row.
    pub endpoint: Option<String>,
    /// `true` for the `Custom provider…` sentinel. The wizard routes selection
    /// of this row to a freeform text input.
    pub is_custom: bool,
}

impl PickerItem {
    /// Build a catalog entry from `(id, display_name, endpoint)`.
    pub fn catalog(
        id: impl Into<String>,
        display_name: impl Into<String>,
        endpoint: Option<&str>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            endpoint: endpoint.map(str::to_string),
            is_custom: false,
        }
    }

    /// Build the `Custom provider…` sentinel row. Always sorts last.
    pub fn custom_sentinel() -> Self {
        Self {
            id: CUSTOM_SENTINEL_ID.to_string(),
            display_name: "Custom provider…".to_string(),
            endpoint: None,
            is_custom: true,
        }
    }

    /// Lowercased search target. Includes both display name and id so a query
    /// for `openai` matches the OpenAI row even if its display name is `OpenAI`.
    pub fn search_target(&self) -> String {
        format!("{} {}", self.display_name, self.id).to_lowercase()
    }
}

/// Sentinel id for the custom-provider row. Picked to never collide with
/// catalog ids (which are lowercase kebab-case without spaces).
pub const CUSTOM_SENTINEL_ID: &str = "__custom__";

/// Case-insensitive subsequence match.
///
/// Returns `true` when every character of `needle` appears in `haystack` in
/// the same order (not necessarily contiguously). Empty `needle` matches
/// everything. Unicode-aware case folding via `char::to_lowercase`, so the
/// query `"üb"` matches `"Über"`.
///
/// # Examples
///
/// ```
/// # use mscode_tui::login_prompt::fuzzy_match;
/// assert!(fuzzy_match("OpenAI", "oai"));
/// assert!(fuzzy_match("anthropic", "ap"));
/// assert!(fuzzy_match("Anything", ""));
/// assert!(!fuzzy_match("openai", "x"));
/// ```
pub fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    // Unicode-aware case folding (e.g. "Ü" folds to "ü"). This is O(N) per
    // keystroke; for ~95 catalog entries the cost is negligible.
    let haystack_lower: String = haystack.to_lowercase();
    let needle_lower: String = needle.to_lowercase();
    let mut needle_chars = needle_lower.chars().peekable();
    for h in haystack_lower.chars() {
        if needle_chars.peek() == Some(&h) {
            needle_chars.next();
        }
    }
    needle_chars.peek().is_none()
}

/// Provider picker state — fuzzy-filterable list with cursor navigation.
#[derive(Debug, Clone)]
pub struct ProviderPicker {
    items: Vec<PickerItem>,
    query: String,
    /// Cursor into the *filtered* list. `None` means nothing highlighted
    /// (only happens when the filtered list is empty).
    cursor: Option<usize>,
    /// Cached filtered view, recomputed on each query change.
    filtered: Vec<usize>,
}

impl ProviderPicker {
    /// Construct from a static catalog. The custom sentinel is appended
    /// automatically and always remains at the bottom of the filtered view.
    pub fn new(mut items: Vec<PickerItem>) -> Self {
        items.push(PickerItem::custom_sentinel());
        let filtered: Vec<usize> = (0..items.len()).collect();
        let cursor = if filtered.is_empty() { None } else { Some(0) };
        Self {
            items,
            query: String::new(),
            cursor,
            filtered,
        }
    }

    /// Read-only view of all items (catalog + custom).
    pub fn items(&self) -> &[PickerItem] {
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

    /// Cursor position into [`filtered_indices()`]. `None` when the filtered
    /// list is empty.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The currently-highlighted item, if any.
    pub fn selected(&self) -> Option<&PickerItem> {
        self.cursor
            .and_then(|i| self.filtered.get(i))
            .and_then(|&idx| self.items.get(idx))
    }

    /// Append a character to the query and recompute the filtered view.
    /// Cursor resets to the top of the new filtered list.
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
            .filter(|(_, item)| {
                if item.is_custom {
                    // Custom row always visible so the user can escape the
                    // catalog even with a typo'd query.
                    true
                } else {
                    fuzzy_match(&item.search_target(), q)
                }
            })
            .map(|(i, _)| i)
            .collect();
        self.cursor = if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    /// Move cursor up (toward index 0). No-op at top.
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
}

/// Freeform text input state. Backed by a `String` + byte cursor.
///
/// The cursor is kept on a char boundary at all times; methods that mutate it
/// panic on internal corruption (would indicate a bug in this module, not user
/// input). Masking is purely a render concern — the underlying value is
/// always stored unmasked.
#[derive(Debug, Clone)]
pub struct TextInput {
    value: String,
    /// Byte offset into `value`, always on a UTF-8 boundary.
    cursor: usize,
    /// When `true`, the renderer shows `*` per character. Used for the secret
    /// step so the terminal scrollback does not leak the key.
    masked: bool,
}

impl TextInput {
    /// Construct with the given masking mode and empty value.
    pub fn new(masked: bool) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            masked,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn masked(&self) -> bool {
        self.masked
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Insert `c` at the cursor and advance the cursor past it.
    pub fn insert(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the char before the cursor (Backspace). No-op at offset 0.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Walk back to the previous char boundary.
        let prev = self.value[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i);
        if let Some(prev_idx) = prev {
            self.value.replace_range(prev_idx..self.cursor, "");
            self.cursor = prev_idx;
        }
    }

    /// Clear the entire field (Ctrl-U).
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Move cursor one char left. No-op at offset 0.
    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.value[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i);
        if let Some(prev_idx) = prev {
            self.cursor = prev_idx;
        }
    }

    /// Move cursor one char right. No-op at end.
    pub fn move_right(&mut self) {
        // `char_indices` yields `(byte_offset, char)` relative to the slice.
        // The slice starts at the cursor, so `nth(1)` is the next char and its
        // byte offset equals the byte length of the char currently under the
        // cursor. Adding that advances one character without breaking UTF-8.
        if let Some((next_offset, _)) = self.value[self.cursor..].char_indices().nth(1) {
            self.cursor += next_offset;
        } else if !self.value.is_empty() {
            // We're on the last char; jump to end.
            self.cursor = self.value.len();
        }
    }
}

/// Current step of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Picking a provider from the catalog.
    Provider,
    /// Freeform text input for `custom:<name>` (only entered when the user
    /// selects the custom sentinel).
    CustomProvider,
    /// Naming the account (e.g. `work`, `personal`).
    Label,
    /// Typing the secret (masked).
    Secret,
    /// Wizard finished; `result()` will return `Some`.
    Done,
    /// User cancelled (Esc on the first step, or Ctrl-C anywhere).
    Cancelled,
}

/// What the event loop should do after handling a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardEffect {
    /// Continue rendering — the visible step may have changed.
    Continue,
    /// Tear down and return `None` to the caller.
    Cancel,
    /// Tear down and return `Some((provider, label, secret))` to the caller.
    Finish,
}

/// Top-level login wizard state.
pub struct LoginWizard {
    step: WizardStep,
    picker: ProviderPicker,
    custom_input: TextInput,
    label_input: TextInput,
    secret_input: TextInput,
    /// `true` once the user has routed through the custom-provider sentinel.
    /// Decoupled from `picker.selected()` so the wizard's resolution remains
    /// correct even after the cursor moves away from the sentinel (which can
    /// happen if we ever add rewind semantics) or in tests that drive `step`
    /// directly.
    chose_custom: bool,
}

impl LoginWizard {
    /// Construct a wizard over a static catalog of items. The custom sentinel
    /// is added automatically; callers should pass only catalog items.
    pub fn new(catalog: Vec<PickerItem>) -> Self {
        Self {
            step: WizardStep::Provider,
            picker: ProviderPicker::new(catalog),
            custom_input: TextInput::new(false),
            label_input: TextInput::new(false),
            secret_input: TextInput::new(true),
            chose_custom: false,
        }
    }

    pub fn step(&self) -> WizardStep {
        self.step
    }

    pub fn picker(&self) -> &ProviderPicker {
        &self.picker
    }

    pub fn custom_input(&self) -> &TextInput {
        &self.custom_input
    }

    pub fn label_input(&self) -> &TextInput {
        &self.label_input
    }

    pub fn secret_input(&self) -> &TextInput {
        &self.secret_input
    }

    /// The resolved provider id. Returns `None` until the provider step is
    /// past.
    pub fn provider_id(&self) -> Option<String> {
        match self.step {
            WizardStep::Provider | WizardStep::CustomProvider | WizardStep::Cancelled => None,
            WizardStep::Label | WizardStep::Secret | WizardStep::Done => {
                if self.chose_custom {
                    let raw = self.custom_input.value().trim().to_string();
                    if raw.is_empty() {
                        return None;
                    }
                    // Auto-prefix `custom:` if the user didn't.
                    if raw.starts_with("custom:") {
                        Some(raw)
                    } else {
                        Some(format!("custom:{raw}"))
                    }
                } else {
                    self.picker.selected().map(|item| item.id.clone())
                }
            }
        }
    }

    /// Final result tuple. Only `Some` after the wizard reaches `Done`.
    pub fn result(&self) -> Option<(String, String, String)> {
        if self.step != WizardStep::Done {
            return None;
        }
        let provider = self.provider_id()?;
        let label = self.label_input.value().trim().to_string();
        let secret = self.secret_input.value().to_string();
        Some((provider, label, secret))
    }

    /// Dispatch a crossterm key event. Mutates state and returns the effect.
    pub fn handle_key(&mut self, key: KeyEvent) -> WizardEffect {
        // Ctrl-C cancels from anywhere.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.step = WizardStep::Cancelled;
            return WizardEffect::Cancel;
        }
        match self.step {
            WizardStep::Provider => self.handle_provider_key(key),
            WizardStep::CustomProvider => self.handle_custom_provider_key(key),
            WizardStep::Label => self.handle_label_key(key),
            WizardStep::Secret => self.handle_secret_key(key),
            WizardStep::Done | WizardStep::Cancelled => WizardEffect::Continue,
        }
    }

    fn handle_provider_key(&mut self, key: KeyEvent) -> WizardEffect {
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::Cancelled;
                WizardEffect::Cancel
            }
            KeyCode::Up => {
                self.picker.move_up();
                WizardEffect::Continue
            }
            KeyCode::Down => {
                self.picker.move_down();
                WizardEffect::Continue
            }
            KeyCode::Enter => match self.picker.selected() {
                Some(item) if item.is_custom => {
                    self.chose_custom = true;
                    self.step = WizardStep::CustomProvider;
                    WizardEffect::Continue
                }
                Some(_) => {
                    self.chose_custom = false;
                    self.step = WizardStep::Label;
                    WizardEffect::Continue
                }
                None => WizardEffect::Continue,
            },
            KeyCode::Backspace => {
                self.picker.pop_query_char();
                WizardEffect::Continue
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.picker.set_query("");
                WizardEffect::Continue
            }
            KeyCode::Char(c) => {
                self.picker.push_query_char(c);
                WizardEffect::Continue
            }
            _ => WizardEffect::Continue,
        }
    }

    fn handle_custom_provider_key(&mut self, key: KeyEvent) -> WizardEffect {
        match key.code {
            KeyCode::Esc => {
                // Retreat to the picker — user changed their mind.
                self.step = WizardStep::Provider;
                WizardEffect::Continue
            }
            KeyCode::Enter => {
                if !self.custom_input.value().trim().is_empty() {
                    self.step = WizardStep::Label;
                }
                WizardEffect::Continue
            }
            KeyCode::Backspace => {
                self.custom_input.backspace();
                WizardEffect::Continue
            }
            KeyCode::Left => {
                self.custom_input.move_left();
                WizardEffect::Continue
            }
            KeyCode::Right => {
                self.custom_input.move_right();
                WizardEffect::Continue
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.custom_input.clear();
                WizardEffect::Continue
            }
            KeyCode::Char(c) => {
                self.custom_input.insert(c);
                WizardEffect::Continue
            }
            _ => WizardEffect::Continue,
        }
    }

    fn handle_label_key(&mut self, key: KeyEvent) -> WizardEffect {
        match key.code {
            KeyCode::Esc => {
                // Retreat.
                if self.chose_custom {
                    self.step = WizardStep::CustomProvider;
                } else {
                    self.step = WizardStep::Provider;
                }
                WizardEffect::Continue
            }
            KeyCode::Enter => {
                if !self.label_input.value().trim().is_empty() {
                    self.step = WizardStep::Secret;
                }
                WizardEffect::Continue
            }
            KeyCode::Backspace => {
                self.label_input.backspace();
                WizardEffect::Continue
            }
            KeyCode::Left => {
                self.label_input.move_left();
                WizardEffect::Continue
            }
            KeyCode::Right => {
                self.label_input.move_right();
                WizardEffect::Continue
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.label_input.clear();
                WizardEffect::Continue
            }
            KeyCode::Char(c) => {
                self.label_input.insert(c);
                WizardEffect::Continue
            }
            _ => WizardEffect::Continue,
        }
    }

    fn handle_secret_key(&mut self, key: KeyEvent) -> WizardEffect {
        match key.code {
            KeyCode::Esc => {
                self.step = WizardStep::Label;
                WizardEffect::Continue
            }
            KeyCode::Enter => {
                if !self.secret_input.value().is_empty() {
                    self.step = WizardStep::Done;
                    WizardEffect::Finish
                } else {
                    WizardEffect::Continue
                }
            }
            KeyCode::Backspace => {
                self.secret_input.backspace();
                WizardEffect::Continue
            }
            KeyCode::Left => {
                self.secret_input.move_left();
                WizardEffect::Continue
            }
            KeyCode::Right => {
                self.secret_input.move_right();
                WizardEffect::Continue
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.secret_input.clear();
                WizardEffect::Continue
            }
            KeyCode::Char(c) => {
                self.secret_input.insert(c);
                WizardEffect::Continue
            }
            _ => WizardEffect::Continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<PickerItem> {
        vec![
            PickerItem::catalog("openai", "OpenAI", Some("https://api.openai.com")),
            PickerItem::catalog("anthropic", "Anthropic", Some("https://api.anthropic.com")),
            PickerItem::catalog("ollama", "Ollama (local)", Some("http://localhost:11434")),
            PickerItem::catalog("zai", "Z.AI", Some("https://api.z.ai")),
        ]
    }

    // ----- fuzzy_match ---------------------------------------------------

    #[test]
    fn fuzzy_empty_needle_matches_anything() {
        assert!(fuzzy_match("openai", ""));
        assert!(fuzzy_match("", ""));
    }

    #[test]
    fn fuzzy_case_insensitive_subsequence() {
        assert!(fuzzy_match("OpenAI", "oai"));
        assert!(fuzzy_match("OpenAI", "OAI"));
        assert!(fuzzy_match("anthropic", "ap"));
        assert!(fuzzy_match("anthropic", "ATH"));
    }

    #[test]
    fn fuzzy_rejects_non_subsequence() {
        assert!(!fuzzy_match("openai", "x"));
        assert!(!fuzzy_match("openai", "aiopen"));
    }

    #[test]
    fn fuzzy_handles_unicode_as_ascii_no_fold() {
        // Non-ASCII characters match themselves but do not get case-folded.
        assert!(fuzzy_match("Über", "üb"));
    }

    // ----- ProviderPicker -----------------------------------------------

    #[test]
    fn picker_starts_with_all_items_plus_custom_sentinel() {
        let p = ProviderPicker::new(catalog());
        // 4 catalog + 1 sentinel.
        assert_eq!(p.items().len(), 5);
        assert_eq!(p.filtered_indices().len(), 5);
        assert!(p.items().last().unwrap().is_custom);
        assert_eq!(p.cursor(), Some(0));
    }

    #[test]
    fn picker_query_filters_catalog_but_keeps_custom() {
        let mut p = ProviderPicker::new(catalog());
        p.set_query("open");
        let visible: Vec<&str> = p
            .filtered_indices()
            .iter()
            .map(|&i| p.items()[i].id.as_str())
            .collect();
        assert_eq!(visible, vec!["openai", CUSTOM_SENTINEL_ID]);
    }

    #[test]
    fn picker_query_matches_id_or_display_name() {
        let mut p = ProviderPicker::new(catalog());
        // `zai` is the id; "Z.AI" is the display name. Both should match.
        p.set_query("zai");
        assert!(
            p.filtered_indices()
                .iter()
                .any(|&i| p.items()[i].id == "zai")
        );
        p.set_query("Z.AI");
        // Case-insensitive — should still find it.
        assert!(
            p.filtered_indices()
                .iter()
                .any(|&i| p.items()[i].id == "zai")
        );
    }

    #[test]
    fn picker_cursor_navigation_clamps() {
        let mut p = ProviderPicker::new(catalog());
        assert_eq!(p.cursor(), Some(0));
        p.move_up();
        assert_eq!(p.cursor(), Some(0));
        p.move_down();
        assert_eq!(p.cursor(), Some(1));
        p.move_down();
        p.move_down();
        p.move_down();
        // At bottom of filtered list (5 items: 0..4).
        assert_eq!(p.cursor(), Some(4));
        p.move_down();
        assert_eq!(p.cursor(), Some(4));
    }

    #[test]
    fn picker_cursor_resets_after_query_change() {
        let mut p = ProviderPicker::new(catalog());
        p.move_down();
        p.move_down();
        assert_eq!(p.cursor(), Some(2));
        p.set_query("o");
        // Two matches: openai + ollama + custom → 3 items, cursor resets to 0.
        assert_eq!(p.cursor(), Some(0));
    }

    #[test]
    fn picker_backspace_pops_query() {
        let mut p = ProviderPicker::new(catalog());
        p.set_query("open");
        assert_eq!(p.query(), "open");
        p.pop_query_char();
        assert_eq!(p.query(), "ope");
        p.pop_query_char();
        p.pop_query_char();
        p.pop_query_char();
        assert_eq!(p.query(), "");
        // No-op on empty.
        p.pop_query_char();
        assert_eq!(p.query(), "");
    }

    #[test]
    fn picker_empty_filtered_list_clears_cursor() {
        let mut p = ProviderPicker::new(catalog());
        p.set_query("zzzzz_nomatch");
        // Custom sentinel always visible, so filtered list is never empty.
        assert!(p.cursor().is_some());
    }

    // ----- TextInput ----------------------------------------------------

    #[test]
    fn text_input_insert_advances_cursor() {
        let mut t = TextInput::new(false);
        t.insert('a');
        t.insert('b');
        t.insert('c');
        assert_eq!(t.value(), "abc");
        assert_eq!(t.cursor(), 3);
    }

    #[test]
    fn text_input_backspace_deletes_before_cursor() {
        let mut t = TextInput::new(false);
        t.insert('a');
        t.insert('b');
        t.insert('c');
        t.backspace();
        assert_eq!(t.value(), "ab");
        assert_eq!(t.cursor(), 2);
    }

    #[test]
    fn text_input_backspace_at_zero_is_noop() {
        let mut t = TextInput::new(false);
        t.backspace();
        assert_eq!(t.value(), "");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn text_input_clear_empties_value() {
        let mut t = TextInput::new(false);
        t.insert('a');
        t.insert('b');
        t.clear();
        assert_eq!(t.value(), "");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn text_input_handles_unicode_correctly() {
        let mut t = TextInput::new(false);
        t.insert('Ü');
        t.insert('b');
        t.insert('e');
        t.insert('r');
        assert_eq!(t.value(), "Über");
        assert_eq!(t.cursor(), "Über".len());
        // Move left past the multibyte Ü.
        t.move_left();
        t.move_left();
        // Cursor should be before 'e', i.e. at byte offset 3.
        assert_eq!(t.cursor(), 3);
        t.insert('X');
        assert_eq!(t.value(), "ÜbXer");
    }

    #[test]
    fn text_input_move_left_right_clamp() {
        let mut t = TextInput::new(false);
        t.insert('a');
        t.insert('b');
        // At end.
        t.move_right();
        assert_eq!(t.cursor(), 2);
        t.move_left();
        t.move_left();
        // At start.
        assert_eq!(t.cursor(), 0);
        t.move_left();
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn text_input_masked_flag_preserved() {
        let t = TextInput::new(true);
        assert!(t.masked());
        let t2 = TextInput::new(false);
        assert!(!t2.masked());
    }

    // ----- LoginWizard --------------------------------------------------

    fn wizard() -> LoginWizard {
        LoginWizard::new(catalog())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn wizard_starts_on_provider_step() {
        let w = wizard();
        assert_eq!(w.step(), WizardStep::Provider);
        assert!(w.result().is_none());
    }

    #[test]
    fn wizard_ctrl_c_cancels_anywhere() {
        let mut w = wizard();
        let effect = w.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(effect, WizardEffect::Cancel);
        assert_eq!(w.step(), WizardStep::Cancelled);
    }

    #[test]
    fn wizard_esc_at_provider_cancels() {
        let mut w = wizard();
        let effect = w.handle_key(key(KeyCode::Esc));
        assert_eq!(effect, WizardEffect::Cancel);
        assert_eq!(w.step(), WizardStep::Cancelled);
    }

    #[test]
    fn wizard_typing_in_provider_filters() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Char('o')));
        w.handle_key(key(KeyCode::Char('p')));
        assert_eq!(w.picker().query(), "op");
    }

    #[test]
    fn wizard_enter_openai_advances_to_label() {
        let mut w = wizard();
        // Default cursor is on OpenAI (first row of unfiltered list).
        let effect = w.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, WizardEffect::Continue);
        assert_eq!(w.step(), WizardStep::Label);
        assert_eq!(w.provider_id().as_deref(), Some("openai"));
    }

    #[test]
    fn wizard_selecting_custom_routes_to_custom_step() {
        let mut w = wizard();
        // Filter down to just the custom sentinel.
        w.handle_key(key(KeyCode::Char('z')));
        w.handle_key(key(KeyCode::Char('z')));
        w.handle_key(key(KeyCode::Char('z')));
        // Custom should be the only visible row.
        assert_eq!(w.picker().filtered_indices().len(), 1);
        let effect = w.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, WizardEffect::Continue);
        assert_eq!(w.step(), WizardStep::CustomProvider);
    }

    #[test]
    fn wizard_custom_flow_auto_prefixes_custom_namespace() {
        let mut w = wizard();
        // Jump straight to CustomProvider. Tests that bypass `handle_key` must
        // set both `step` and `chose_custom` because provider resolution
        // depends on the latter, not on the picker cursor.
        w.step = WizardStep::CustomProvider;
        w.chose_custom = true;
        for c in "together".chars() {
            w.custom_input.insert(c);
        }
        w.handle_key(key(KeyCode::Enter));
        assert_eq!(w.step(), WizardStep::Label);
        assert_eq!(w.provider_id().as_deref(), Some("custom:together"));
    }

    #[test]
    fn wizard_custom_flow_keeps_existing_custom_prefix() {
        let mut w = wizard();
        w.step = WizardStep::CustomProvider;
        w.chose_custom = true;
        for c in "custom:foo".chars() {
            w.custom_input.insert(c);
        }
        w.handle_key(key(KeyCode::Enter));
        assert_eq!(w.provider_id().as_deref(), Some("custom:foo"));
    }

    #[test]
    fn wizard_label_enter_advances_to_secret() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // Provider -> Label
        for c in "work".chars() {
            w.handle_key(key(KeyCode::Char(c)));
        }
        let effect = w.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, WizardEffect::Continue);
        assert_eq!(w.step(), WizardStep::Secret);
    }

    #[test]
    fn wizard_label_enter_with_empty_does_not_advance() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // Provider -> Label
        let effect = w.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, WizardEffect::Continue);
        // Still on Label.
        assert_eq!(w.step(), WizardStep::Label);
    }

    #[test]
    fn wizard_secret_enter_finishes() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // Provider -> Label
        for c in "work".chars() {
            w.handle_key(key(KeyCode::Char(c)));
        }
        w.handle_key(key(KeyCode::Enter)); // Label -> Secret
        for c in "sk-test-12345".chars() {
            w.handle_key(key(KeyCode::Char(c)));
        }
        let effect = w.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, WizardEffect::Finish);
        assert_eq!(w.step(), WizardStep::Done);
        assert_eq!(
            w.result(),
            Some((
                "openai".to_string(),
                "work".to_string(),
                "sk-test-12345".to_string()
            ))
        );
    }

    #[test]
    fn wizard_esc_on_label_retreats_to_provider() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // Provider -> Label
        w.handle_key(key(KeyCode::Esc));
        assert_eq!(w.step(), WizardStep::Provider);
    }

    #[test]
    fn wizard_esc_on_secret_retreats_to_label() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // -> Label
        for c in "work".chars() {
            w.handle_key(key(KeyCode::Char(c)));
        }
        w.handle_key(key(KeyCode::Enter)); // -> Secret
        w.handle_key(key(KeyCode::Esc));
        assert_eq!(w.step(), WizardStep::Label);
    }

    #[test]
    fn wizard_ctrl_u_clears_active_input() {
        let mut w = wizard();
        w.handle_key(key(KeyCode::Enter)); // -> Label
        for c in "work".chars() {
            w.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(w.label_input().value(), "work");
        w.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(w.label_input().value(), "");
    }
}
