use crossterm::event::{KeyCode, KeyEvent};
use flashtest::{FlashTestPhase, FlashTestStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};

use crate::{
    app::{Action, Resources, View},
    commands::common::{COLOR_ACTIVE_BORDER, COLOR_BASE_BLUE, COLOR_ROW_SELECTED},
    tui::Keybinding,
};

const KEYBINDINGS: &[Keybinding] = &[
    Keybinding { key: "Esc/q", description: "Quit" },
    Keybinding { key: "?", description: "Toggle help" },
    Keybinding { key: "Up/k", description: "Scroll up" },
    Keybinding { key: "Down/j", description: "Scroll down" },
    Keybinding { key: "Enter", description: "Show error details" },
];

#[derive(Debug)]
struct DashboardState {
    table_state: TableState,
    /// Whether to show the error detail panel.
    show_detail: bool,
}

impl DashboardState {
    fn new() -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self { table_state, show_detail: false }
    }
}

#[derive(Debug, Default)]
pub struct FlashTestView {
    dashboard: Option<DashboardState>,
}

impl FlashTestView {
    pub const fn new() -> Self {
        Self { dashboard: None }
    }
}

impl View for FlashTestView {
    fn keybindings(&self) -> &'static [Keybinding] {
        KEYBINDINGS
    }

    fn tick(&mut self, resources: &mut Resources) -> Action {
        if let Some(ref mut ft) = resources.flashtest {
            ft.poll();
            if self.dashboard.is_none() {
                self.dashboard = Some(DashboardState::new());
            }
        } else {
            self.dashboard = None;
        }
        Action::None
    }

    fn handle_key(&mut self, key: KeyEvent, resources: &mut Resources) -> Action {
        self.dashboard.as_mut().map_or(Action::None, |state| handle_key(key, state, resources))
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, resources: &Resources) {
        if let Some(state) = self.dashboard.as_mut() {
            render_dashboard(frame, area, resources, state);
        } else {
            render_idle(frame, area);
        }
    }
}

fn render_idle(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" Flash Test ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No flash test active",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Run: basectl flashtest --rpc-url <endpoint>",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(para, inner);
}

fn handle_key(key: KeyEvent, state: &mut DashboardState, resources: &Resources) -> Action {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(selected) = state.table_state.selected()
                && selected > 0
            {
                state.table_state.select(Some(selected - 1));
            }
            state.show_detail = false;
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(selected) = state.table_state.selected() {
                let max =
                    resources.flashtest.as_ref().map_or(0, |ft| ft.results.len().saturating_sub(1));
                if selected < max {
                    state.table_state.select(Some(selected + 1));
                }
            }
            state.show_detail = false;
            Action::None
        }
        KeyCode::PageUp => {
            if let Some(selected) = state.table_state.selected() {
                let new_pos = selected.saturating_sub(10);
                state.table_state.select(Some(new_pos));
            }
            state.show_detail = false;
            Action::None
        }
        KeyCode::PageDown => {
            if let Some(selected) = state.table_state.selected() {
                let max =
                    resources.flashtest.as_ref().map_or(0, |ft| ft.results.len().saturating_sub(1));
                let new_pos = (selected + 10).min(max);
                state.table_state.select(Some(new_pos));
            }
            state.show_detail = false;
            Action::None
        }
        KeyCode::Enter => {
            state.show_detail = !state.show_detail;
            Action::None
        }
        _ => Action::None,
    }
}

fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    resources: &Resources,
    state: &mut DashboardState,
) {
    let Some(ft) = &resources.flashtest else {
        render_idle(frame, area);
        return;
    };

    let is_complete = ft.phase == FlashTestPhase::Complete;

    // Layout: header + optional banner + body + optional detail
    let show_detail = state.show_detail
        && state.table_state.selected().is_some_and(|idx| {
            ft.results
                .get(idx)
                .is_some_and(|r| r.status == FlashTestStatus::Failed && r.error.is_some())
        });

    let mut constraints = vec![Constraint::Length(1)]; // header
    if is_complete {
        constraints.push(Constraint::Length(1)); // banner
    }
    if show_detail {
        constraints.push(Constraint::Min(8)); // table
        constraints.push(Constraint::Length(6)); // detail
    } else {
        constraints.push(Constraint::Min(0)); // table
    }

    let chunks =
        Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);

    let mut chunk_idx = 0;

    // Header
    render_header(frame, chunks[chunk_idx], ft);
    chunk_idx += 1;

    // Completion banner
    if is_complete {
        render_completion_banner(frame, chunks[chunk_idx], ft);
        chunk_idx += 1;
    }

    // Table
    render_table(frame, chunks[chunk_idx], ft, &mut state.table_state);
    chunk_idx += 1;

    // Detail panel
    if show_detail
        && let Some(idx) = state.table_state.selected()
        && let Some(entry) = ft.results.get(idx)
    {
        render_detail(frame, chunks[chunk_idx], entry);
    }
}

fn render_header(f: &mut Frame, area: Rect, ft: &flashtest::FlashTestState) {
    let progress = ft.passed + ft.failed + ft.skipped;
    let total_str =
        if ft.total > 0 { format!("{}/{}", progress, ft.total) } else { format!("{progress}") };

    let phase_color = match ft.phase {
        FlashTestPhase::Running => Color::Green,
        FlashTestPhase::Complete => Color::Cyan,
    };

    let spans = vec![
        Span::styled(
            " FLASH TEST ",
            Style::default().fg(Color::Black).bg(COLOR_BASE_BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(&ft.rpc_url, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(total_str, Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(format!("{} pass", ft.passed), Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(
            format!("{} fail", ft.failed),
            Style::default().fg(if ft.failed > 0 { Color::Red } else { Color::DarkGray }),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{} skip", ft.skipped),
            Style::default().fg(if ft.skipped > 0 { Color::Yellow } else { Color::DarkGray }),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", ft.phase),
            Style::default().fg(phase_color).add_modifier(Modifier::BOLD),
        ),
    ];

    let para = Paragraph::new(Line::from(spans));
    f.render_widget(para, area);
}

fn render_completion_banner(f: &mut Frame, area: Rect, ft: &flashtest::FlashTestState) {
    let (text, bg_color) = if ft.failed == 0 {
        (format!("ALL TESTS PASSED - {} passed, {} skipped", ft.passed, ft.skipped), Color::Green)
    } else {
        (
            format!(
                "{} FAILED - {} passed, {} failed, {} skipped",
                ft.failed, ft.passed, ft.failed, ft.skipped
            ),
            Color::Red,
        )
    };

    let para = Paragraph::new(text)
        .style(Style::default().fg(Color::White).bg(bg_color).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);

    f.render_widget(para, area);
}

fn render_table(
    f: &mut Frame,
    area: Rect,
    ft: &flashtest::FlashTestState,
    table_state: &mut TableState,
) {
    let block = Block::default()
        .title(" Results ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACTIVE_BORDER));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if ft.results.is_empty() {
        let para =
            Paragraph::new("Waiting for tests...").style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    }

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("Status").style(header_style),
        Cell::from("Category").style(header_style),
        Cell::from("Test").style(header_style),
        Cell::from("Duration").style(header_style),
    ]);

    let rows: Vec<Row> = ft
        .results
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_selected = table_state.selected() == Some(idx);

            let row_style = if is_selected {
                Style::default().bg(COLOR_ROW_SELECTED)
            } else {
                Style::default()
            };

            let (status_str, status_style) = match entry.status {
                FlashTestStatus::Running => {
                    ("RUN ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                }
                FlashTestStatus::Passed => ("PASS", Style::default().fg(Color::Green)),
                FlashTestStatus::Failed => {
                    ("FAIL", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                }
                FlashTestStatus::Skipped => ("SKIP", Style::default().fg(Color::Yellow)),
            };

            let duration_str = entry.duration.map_or_else(
                || {
                    if entry.status == FlashTestStatus::Running {
                        "...".to_string()
                    } else {
                        "-".to_string()
                    }
                },
                |d| format!("{}ms", d.as_millis()),
            );

            Row::new(vec![
                Cell::from(status_str).style(status_style),
                Cell::from(entry.category.as_str()).style(Style::default().fg(Color::Cyan)),
                Cell::from(entry.name.as_str()),
                Cell::from(duration_str).style(Style::default().fg(Color::DarkGray)),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(6),
        Constraint::Length(14),
        Constraint::Min(20),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_stateful_widget(table, inner, table_state);
}

fn render_detail(f: &mut Frame, area: Rect, entry: &flashtest::FlashTestResultEntry) {
    let block = Block::default()
        .title(format!(" Error: {} ", entry.name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let error_text = entry.error.as_deref().unwrap_or("Unknown error");

    let para = Paragraph::new(error_text)
        .style(Style::default().fg(Color::Red))
        .wrap(ratatui::widgets::Wrap { trim: true });

    f.render_widget(para, inner);
}
