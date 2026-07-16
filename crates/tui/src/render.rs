//! Ratatui rendering.
//!
//! This module is intentionally a single free function — it has no business
//! logic and is NOT unit-tested. All state-machine behavior lives in
//! [`crate::app`] and is tested there. The renderer is just a projection.
//!
//! The layout is:
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ message log (scrolling)     │
//! │                             │
//! │                             │
//! ├─────────────────────────────┤
//! │ input box (3 lines)         │
//! ├─────────────────────────────┤
//! │ status bar (1 line)         │
//! └─────────────────────────────┘
//! ```
//!
//! When the session picker is active (`/sessions`), the input box is replaced
//! by the picker.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::modes::InputMode;

/// Draw one frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // message log
            Constraint::Length(3), // input box / picker
            Constraint::Length(1), // status bar
        ])
        .split(area);

    draw_message_log(frame, app, chunks[0]);
    draw_input_region(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);
}

fn draw_message_log(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default().borders(Borders::ALL).title("Messages");
    let lines: Vec<Line> = app
        .messages()
        .history()
        .map(|entry| {
            let style = if entry.pending_approval {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            let prefix = if entry.pending_approval {
                "[PLAN] "
            } else {
                ""
            };
            Line::styled(format!("{prefix}{}", entry.text), style)
        })
        .collect();
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_input_region(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // When in slash mode, show the picker overlay.
    if app.input_mode() == InputMode::SlashCommand {
        let items: Vec<ListItem> = app
            .slash_matches()
            .iter()
            .map(|s| ListItem::new(format!("/{s}")))
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Commands (filter)"),
        );
        frame.render_widget(list, area);
        return;
    }

    if app.input_mode() == InputMode::ModelsPicker {
        draw_models_picker(frame, app, area);
        return;
    }

    let title = match app.input_mode() {
        InputMode::Normal => "Input (press i to insert, / for commands)",
        InputMode::Insert => "Input (Esc to leave, Enter to submit, Tab to queue)",
        InputMode::SlashCommand => "Commands",
        InputMode::ModelsPicker => "Models (picker)",
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let prompt = app.config().prompt.as_str();
    let draft = app.messages().draft();
    let display = format!("{prompt}{draft}");
    let paragraph = Paragraph::new(display)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Accent + dim colors reused from `login_render`. Duplicated rather than
/// sharing because the values are tiny and pulling them as `pub(crate)` would
/// leak a styled-api surface for one render module.
const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

fn draw_models_picker(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let picker = match app.models_picker() {
        Some(p) => p,
        None => return,
    };

    // Empty-items state — show a hint instead of an empty list.
    if picker.items().is_empty() {
        let hint = "no credentials configured — run `mscode login add` to add one";
        let line = Line::from(vec![Span::styled(
            format!(" {hint}"),
            Style::default().fg(DIM),
        )]);
        let block = Block::default().borders(Borders::ALL).title(" Models ");
        frame.render_widget(Paragraph::new(line).block(block), area);
        return;
    }

    // Split the input region into a search row + list.
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    let query = picker.query();
    let search_line = if query.is_empty() {
        Line::from(vec![
            Span::styled("Search: ", Style::default().fg(DIM)),
            Span::styled("type to filter…", Style::default().fg(DIM)),
        ])
    } else {
        Line::from(vec![
            Span::styled("Search: ", Style::default().fg(ACCENT)),
            Span::raw(query),
        ])
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .title(" Models (filter) ");
    frame.render_widget(Paragraph::new(search_line).block(search_block), inner[0]);

    let items: Vec<ListItem> = picker
        .filtered_indices()
        .iter()
        .map(|&idx| {
            let item = &picker.items()[idx];
            let ctx = item
                .context_window
                .map(|c| format!("   [{c} ctx]"))
                .unwrap_or_default();
            let tools = if item.supports_tools { " 🔧" } else { "" };
            ListItem::new(format!("{}{ctx}{tools}", item.display_label))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = ratatui::widgets::ListState::default();
    if let Some(cursor) = picker.cursor() {
        state.select(Some(cursor));
    }
    frame.render_stateful_widget(list, inner[1], &mut state);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let plan_indicator = if app.config().show_plan_indicator {
        match app.plan_mode() {
            crate::modes::PlanMode::Planning => "[PLAN MODE]",
            crate::modes::PlanMode::Executing => "[EXEC]",
        }
    } else {
        ""
    };

    let mode_label = match app.input_mode() {
        InputMode::Normal => "NORMAL",
        InputMode::Insert => "INSERT",
        InputMode::SlashCommand => "SLASH",
        InputMode::ModelsPicker => "MODELS",
    };

    let spans = vec![
        Span::styled(
            format!(" {mode_label} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(" "),
        Span::styled(
            plan_indicator.to_string(),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(format!("cwd: {}", app.cwd())),
    ];
    let line = Line::from(spans);
    let paragraph = Paragraph::new(line).style(Style::default().fg(Color::White));
    frame.render_widget(paragraph, area);
}
