//! Ratatui rendering for the login wizard.
//!
//! Mirrors the philosophy of [`crate::render`]: a single free function that
//! projects a [`LoginWizard`] onto a frame. No business logic, no unit tests —
//! all behavior is exercised through the state-machine tests in
//! [`crate::login_prompt`].
//!
//! # Layout
//!
//! ```text
//! ┌───────────────────────────────────────┐
//! │ mscode login — step 1 of 3: Provider  │
//! ├───────────────────────────────────────┤
//! │ Search: query                         │
//! │ ┌─────────────────────────────────┐   │
//! │ │ OpenAI      https://api.openai  │   │
//! │ │ Anthropic   https://api.anth... │   │
//! │ │ ...                             │   │
//! │ │ Custom provider…                │   │
//! │ └─────────────────────────────────┘   │
//! ├───────────────────────────────────────┤
//! │ ↑/↓ select • Enter confirm • Esc...   │
//! └───────────────────────────────────────┘
//! ```

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::login_prompt::{LoginWizard, TextInput, WizardStep};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

/// Draw one frame of the wizard.
pub fn draw(frame: &mut Frame, wizard: &LoginWizard) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(5),    // content
            Constraint::Length(1), // help line
        ])
        .split(area);

    draw_title(frame, wizard, chunks[0]);
    draw_content(frame, wizard, chunks[1]);
    draw_help_line(frame, wizard, chunks[2]);
}

fn draw_title(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let (step_num, label) = step_indicator(wizard.step());
    let title = format!(" mscode login — step {step_num} of 3: {label} ");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT));
    let paragraph = Paragraph::new(Line::from(vec![Span::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    )]))
    .block(block);
    frame.render_widget(paragraph, area);
}

fn step_indicator(step: WizardStep) -> (u8, &'static str) {
    match step {
        WizardStep::Provider | WizardStep::CustomProvider => (1, "Provider"),
        WizardStep::Label => (2, "Label"),
        WizardStep::Secret => (3, "Secret"),
        WizardStep::Done => (3, "Done"),
        WizardStep::Cancelled => (0, "Cancelled"),
    }
}

fn draw_content(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    match wizard.step() {
        WizardStep::Provider => draw_provider_picker(frame, wizard, area),
        WizardStep::CustomProvider => draw_custom_input(frame, wizard, area),
        WizardStep::Label => draw_label_input(frame, wizard, area),
        WizardStep::Secret => draw_secret_input(frame, wizard, area),
        WizardStep::Done | WizardStep::Cancelled => {
            // The loop tears down before rendering these; render a placeholder
            // so the frame is never blank if draw is called in a test harness.
            let p = Paragraph::new("").block(Block::default().borders(Borders::ALL));
            frame.render_widget(p, area);
        }
    }
}

fn draw_provider_picker(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let picker = wizard.picker();

    // Split the content area into a search box and a list box.
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    // Search row.
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
        .border_type(BorderType::Rounded)
        .title(" Select provider ");
    frame.render_widget(Paragraph::new(search_line).block(search_block), inner[0]);

    // List rows.
    let items: Vec<ListItem> = picker
        .filtered_indices()
        .iter()
        .map(|&idx| {
            let item = &picker.items()[idx];
            let display = if let Some(ep) = &item.endpoint {
                format!("{:<14} {}", item.display_name, ep)
            } else if item.is_custom {
                format!("{:<14} (enter your own id)", item.display_name)
            } else {
                item.display_name.clone()
            };
            if item.is_custom {
                ListItem::new(format!("✦ {display}")).style(Style::default().fg(Color::Magenta))
            } else {
                ListItem::new(format!("  {display}"))
            }
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .highlight_style(
            Style::default()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    if let Some(cursor) = picker.cursor() {
        state.select(Some(cursor));
    }
    frame.render_stateful_widget(list, inner[1], &mut state);
}

fn draw_custom_input(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let input = wizard.custom_input();
    draw_text_input_block(
        frame,
        area,
        " Custom provider id ",
        input,
        Some("custom:"),
        "example: together — the wizard will prefix `custom:` automatically",
    );
}

fn draw_label_input(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let input = wizard.label_input();
    draw_text_input_block(
        frame,
        area,
        " Account label ",
        input,
        None,
        "example: work, personal, ci",
    );
}

fn draw_secret_input(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let input = wizard.secret_input();
    draw_text_input_block(
        frame,
        area,
        " API key / secret ",
        input,
        None,
        "value is masked — stored via OS keyring at exit",
    );
}

fn draw_text_input_block(
    frame: &mut Frame,
    area: Rect,
    title: &'static str,
    input: &TextInput,
    prefix: Option<&str>,
    hint: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(1)])
        .split(area);

    let value_span = if input.masked() {
        Span::raw("*".repeat(input.value().chars().count()))
    } else {
        Span::raw(input.value().to_string())
    };

    let mut spans: Vec<Span> = Vec::new();
    if let Some(pfx) = prefix {
        spans.push(Span::styled(pfx, Style::default().fg(ACCENT)));
    }
    spans.push(value_span);
    spans.push(Span::styled(
        "▎",
        Style::default().add_modifier(Modifier::SLOW_BLINK),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title);
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), chunks[0]);

    let hint_line = Line::from(vec![Span::styled(
        format!("  {hint}"),
        Style::default().fg(DIM),
    )]);
    frame.render_widget(hint_line, chunks[1]);
}

fn draw_help_line(frame: &mut Frame, wizard: &LoginWizard, area: Rect) {
    let help = match wizard.step() {
        WizardStep::Provider => {
            "↑/↓ select  •  Enter confirm  •  Type to search  •  Backspace deletes  •  Esc cancel  •  Ctrl-C cancel"
        }
        WizardStep::CustomProvider | WizardStep::Label | WizardStep::Secret => {
            "Enter confirm  •  ←/→ move  •  Backspace deletes  •  Ctrl-U clear  •  Esc back  •  Ctrl-C cancel"
        }
        WizardStep::Done | WizardStep::Cancelled => "",
    };
    let line = Line::from(vec![Span::styled(
        help,
        Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
    )]);
    frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), area);
}
