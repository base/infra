use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent};
use gobrr::LoadTestPhase;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};

use crate::{
    app::{Action, Resources, View},
    commands::common::{
        COLOR_ACTIVE_BORDER, COLOR_BASE_BLUE, COLOR_ROW_HIGHLIGHTED, COLOR_ROW_SELECTED,
        build_gas_bar, format_gas, format_gwei, time_diff_color,
    },
    tui::Keybinding,
};

const GAS_BAR_CHARS: usize = 40;
const DEFAULT_ELASTICITY: u64 = 6;

const DASHBOARD_KEYBINDINGS: &[Keybinding] = &[
    Keybinding { key: "Esc/q", description: "Quit" },
    Keybinding { key: "?", description: "Toggle help" },
    Keybinding { key: "Space", description: "Pause/Resume flash" },
    Keybinding { key: "Up/k", description: "Scroll up" },
    Keybinding { key: "Down/j", description: "Scroll down" },
    Keybinding { key: "PgUp", description: "Page up" },
    Keybinding { key: "PgDn", description: "Page down" },
    Keybinding { key: "y", description: "Copy block number" },
];

#[derive(Debug)]
struct DashboardState {
    table_state: TableState,
    auto_scroll: bool,
}

impl DashboardState {
    fn new() -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self { table_state, auto_scroll: true }
    }
}

#[derive(Debug, Default)]
pub struct LoadTestView {
    dashboard: Option<DashboardState>,
}

impl LoadTestView {
    pub const fn new() -> Self {
        Self { dashboard: None }
    }
}

impl View for LoadTestView {
    fn keybindings(&self) -> &'static [Keybinding] {
        DASHBOARD_KEYBINDINGS
    }

    fn tick(&mut self, resources: &mut Resources) -> Action {
        if resources.loadtest.is_some() {
            if self.dashboard.is_none() {
                self.dashboard = Some(DashboardState::new());
            }
            if let Some(state) = self.dashboard.as_mut()
                && state.auto_scroll
                && !resources.flash.entries.is_empty()
            {
                state.table_state.select(Some(0));
            }
        } else {
            self.dashboard = None;
        }
        Action::None
    }

    fn handle_key(&mut self, key: KeyEvent, resources: &mut Resources) -> Action {
        if let Some(state) = self.dashboard.as_mut() {
            handle_dashboard_key(key, &mut state.table_state, &mut state.auto_scroll, resources)
        } else {
            Action::None
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, resources: &Resources) {
        if let Some(state) = self.dashboard.as_mut() {
            render_dashboard(frame, area, resources, &state.table_state);
        } else {
            render_idle(frame, area);
        }
    }
}

fn render_idle(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" Load Test ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No load test active",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Run: basectl loadtest --file <config.yaml>",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(para, inner);
}

fn handle_dashboard_key(
    key: KeyEvent,
    table_state: &mut TableState,
    auto_scroll: &mut bool,
    resources: &mut Resources,
) -> Action {
    match key.code {
        KeyCode::Char(' ') => {
            resources.flash.paused = !resources.flash.paused;
            Action::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(selected) = table_state.selected() {
                if selected > 0 {
                    table_state.select(Some(selected - 1));
                    *auto_scroll = false;
                } else {
                    *auto_scroll = true;
                }
            }
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(selected) = table_state.selected() {
                let max = resources.flash.entries.len().saturating_sub(1);
                if selected < max {
                    table_state.select(Some(selected + 1));
                    *auto_scroll = false;
                }
            }
            Action::None
        }
        KeyCode::PageUp => {
            if let Some(selected) = table_state.selected() {
                let new_pos = selected.saturating_sub(10);
                table_state.select(Some(new_pos));
                *auto_scroll = new_pos == 0;
            }
            Action::None
        }
        KeyCode::PageDown => {
            if let Some(selected) = table_state.selected() {
                let max = resources.flash.entries.len().saturating_sub(1);
                let new_pos = (selected + 10).min(max);
                table_state.select(Some(new_pos));
                *auto_scroll = false;
            }
            Action::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            table_state.select(Some(0));
            *auto_scroll = true;
            Action::None
        }
        KeyCode::Char('y') => {
            if let Some(idx) = table_state.selected()
                && let Some(entry) = resources.flash.entries.get(idx)
                && let Ok(mut clipboard) = Clipboard::new()
            {
                let _ = clipboard.set_text(entry.block_number.to_string());
            }
            Action::None
        }
        _ => Action::None,
    }
}

fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    resources: &Resources,
    table_state: &TableState,
) {
    let is_complete =
        resources.loadtest.as_ref().is_some_and(|lt| lt.phase == LoadTestPhase::Complete);

    // Main layout: header + optional banner + body
    let main_chunks = if is_complete {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header bar
                Constraint::Length(1), // completion banner
                Constraint::Min(0),    // body
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header bar
                Constraint::Min(0),    // body
            ])
            .split(area)
    };

    render_header(frame, main_chunks[0], resources);

    let body_area = if is_complete {
        // Render completion banner
        render_completion_banner(frame, main_chunks[1], resources);
        main_chunks[2]
    } else {
        main_chunks[1]
    };

    // Body: top (55%) + bottom (45%)
    let body_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(55), // metrics + txpool
            Constraint::Percentage(45), // flashblocks
        ])
        .split(body_area);

    // Top: metrics (50%) + txpool (50%)
    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // metrics
            Constraint::Percentage(50), // txpool
        ])
        .split(body_chunks[0]);

    render_metrics(frame, top_chunks[0], resources);
    render_txpool(frame, top_chunks[1], resources);
    render_flashblocks(frame, body_chunks[1], resources, table_state);
}

fn render_header(f: &mut Frame, area: Rect, resources: &Resources) {
    let Some(lt) = &resources.loadtest else {
        let para =
            Paragraph::new("No load test active").style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, area);
        return;
    };

    let elapsed = lt.stats.as_ref().map_or(0.0, |s| s.elapsed_secs);
    let elapsed_str = format_elapsed(elapsed);

    let duration_str =
        lt.duration.map_or_else(|| "indefinite".to_string(), |d| format!("{}s", d.as_secs()));

    let tps_str = lt.target_tps.map_or_else(|| "unlimited".to_string(), |tps| format!("{tps}"));

    let phase_color = match lt.phase {
        LoadTestPhase::Running => Color::Green,
        LoadTestPhase::Complete => Color::Cyan,
        LoadTestPhase::Starting | LoadTestPhase::Draining => Color::Yellow,
    };

    let spans = vec![
        Span::styled(
            " LOAD TEST ",
            Style::default().fg(Color::Black).bg(COLOR_BASE_BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(&lt.config_file, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(format!("{elapsed_str}/{duration_str}"), Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(format!("TPS: {tps_str}"), Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", lt.phase),
            Style::default().fg(phase_color).add_modifier(Modifier::BOLD),
        ),
    ];

    let para = Paragraph::new(Line::from(spans));
    f.render_widget(para, area);
}

fn render_completion_banner(f: &mut Frame, area: Rect, resources: &Resources) {
    let (sent, confirmed, failed) = resources
        .loadtest
        .as_ref()
        .and_then(|lt| lt.stats.as_ref())
        .map_or((0, 0, 0), |s| (s.sent, s.confirmed, s.failed));

    let text =
        format!("✓ LOAD TEST COMPLETE - {sent} sent, {confirmed} confirmed, {failed} failed");

    let para = Paragraph::new(text)
        .style(Style::default().fg(Color::White).bg(Color::Green).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);

    f.render_widget(para, area);
}

fn render_metrics(f: &mut Frame, area: Rect, resources: &Resources) {
    let block = Block::default()
        .title(" Metrics ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_ACTIVE_BORDER));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(stats) = resources.loadtest.as_ref().and_then(|lt| lt.stats.as_ref()) else {
        let para =
            Paragraph::new("Waiting for stats...").style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    };

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let section_style = Style::default().fg(COLOR_BASE_BLUE).add_modifier(Modifier::BOLD);
    let error_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);

    let mut lines = Vec::new();

    // Throughput
    lines.push(Line::from(Span::styled("THROUGHPUT", section_style)));
    lines.push(Line::from(vec![
        Span::styled("  Send TPS         ", label_style),
        Span::styled(format!("{:.0} tx/s", stats.tps()), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Confirmed TPS    ", label_style),
        Span::styled(format!("{:.0} tx/s", stats.confirmed_tps()), value_style),
    ]));
    lines.push(Line::from(""));

    // Transactions
    lines.push(Line::from(Span::styled("TRANSACTIONS", section_style)));
    lines.push(Line::from(vec![
        Span::styled("  Sent             ", label_style),
        Span::styled(format_number(stats.sent), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Confirmed        ", label_style),
        Span::styled(format_number(stats.confirmed), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Pending          ", label_style),
        Span::styled(
            format_number(stats.pending()),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Failed           ", label_style),
        Span::styled(
            format_number(stats.failed),
            if stats.failed > 0 { error_style } else { value_style },
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Timed Out        ", label_style),
        Span::styled(
            format_number(stats.timed_out),
            if stats.timed_out > 0 { error_style } else { value_style },
        ),
    ]));
    lines.push(Line::from(""));

    // Flashblock Inclusion Times
    lines.push(Line::from(Span::styled("FB INCLUSION", section_style)));
    if stats.fb_inclusion_count > 0 {
        lines.push(Line::from(vec![
            Span::styled("  P50  ", label_style),
            Span::styled(format_ms(stats.fb_percentile(50.0)), value_style),
            Span::styled("    P95  ", label_style),
            Span::styled(format_ms(stats.fb_percentile(95.0)), value_style),
            Span::styled("    P99  ", label_style),
            Span::styled(format_ms(stats.fb_percentile(99.0)), value_style),
        ]));
    } else {
        lines.push(Line::from(Span::styled("  N/A", Style::default().fg(Color::DarkGray))));
    }
    lines.push(Line::from(""));

    // Block Inclusion Times
    lines.push(Line::from(Span::styled("BLOCK INCLUSION", section_style)));
    if stats.block_inclusion_count > 0 {
        lines.push(Line::from(vec![
            Span::styled("  P50  ", label_style),
            Span::styled(format_ms(stats.block_percentile(50.0)), value_style),
            Span::styled("    P95  ", label_style),
            Span::styled(format_ms(stats.block_percentile(95.0)), value_style),
            Span::styled("    P99  ", label_style),
            Span::styled(format_ms(stats.block_percentile(99.0)), value_style),
        ]));
    } else {
        lines.push(Line::from(Span::styled("  N/A", Style::default().fg(Color::DarkGray))));
    }

    // Errors
    if !stats.failure_reasons.is_empty() {
        lines.push(Line::from(""));
        let total_errors: u64 = stats.failure_reasons.values().sum();
        lines.push(Line::from(vec![
            Span::styled("ERRORS", section_style),
            Span::raw("  "),
            Span::styled(format!("{total_errors} total"), error_style),
        ]));
        for (reason, count) in &stats.failure_reasons {
            let display_reason = if reason.len() > 20 { &reason[..20] } else { reason };
            lines.push(Line::from(vec![
                Span::styled(format!("  {display_reason:<20} "), label_style),
                Span::styled(count.to_string(), Style::default().fg(Color::Red)),
            ]));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_txpool(f: &mut Frame, area: Rect, resources: &Resources) {
    let block = Block::default()
        .title(" Txpool ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(lt) = &resources.loadtest else {
        return;
    };

    if lt.txpool_hosts.is_empty() {
        let para = Paragraph::new("No management hosts configured")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    }

    if lt.txpool_status.is_empty() {
        let para = Paragraph::new("Waiting for txpool data...")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    }

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("Host").style(header_style),
        Cell::from("Pending").style(header_style),
        Cell::from("Queued").style(header_style),
    ]);

    let rows: Vec<Row> = lt
        .txpool_status
        .iter()
        .map(|status| {
            let host_display = status
                .host
                .strip_prefix("http://")
                .or_else(|| status.host.strip_prefix("https://"))
                .unwrap_or(&status.host);

            let pending_style = if status.pending > 1000 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                Cell::from(host_display.to_string()).style(Style::default().fg(Color::Cyan)),
                Cell::from(format_number(status.pending)).style(pending_style),
                Cell::from(format_number(status.queued)).style(Style::default().fg(Color::White)),
            ])
        })
        .collect();

    let widths = [Constraint::Min(20), Constraint::Length(10), Constraint::Length(10)];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

fn render_flashblocks(f: &mut Frame, area: Rect, resources: &Resources, table_state: &TableState) {
    let flash = &resources.flash;

    let title = if flash.paused {
        format!(" Flashblocks [PAUSED] - {} msgs ", flash.message_count)
    } else {
        format!(" Flashblocks - {} msgs ", flash.message_count)
    };

    let border_color = if flash.paused { Color::Yellow } else { COLOR_ACTIVE_BORDER };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let highlighted_block =
        table_state.selected().and_then(|idx| flash.entries.get(idx)).map(|e| e.block_number);

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("Block").style(header_style),
        Cell::from("Idx").style(header_style),
        Cell::from("Txs").style(header_style),
        Cell::from("Gas").style(header_style),
        Cell::from("Base Fee").style(header_style),
        Cell::from("\u{0394}t").style(header_style),
        Cell::from("Fill").style(header_style),
    ]);

    let rows: Vec<Row> = flash
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_selected = table_state.selected() == Some(idx);
            let is_highlighted = highlighted_block == Some(entry.block_number);

            let row_style = if is_selected {
                Style::default().bg(COLOR_ROW_SELECTED)
            } else if is_highlighted {
                Style::default().bg(COLOR_ROW_HIGHLIGHTED)
            } else {
                Style::default()
            };

            let (base_fee_str, base_fee_style) = if entry.index == 0 {
                let fee_str = entry.base_fee.map_or_else(|| "-".to_string(), format_gwei);
                let style = match (entry.base_fee, entry.prev_base_fee) {
                    (Some(curr), Some(prev)) if curr > prev => Style::default().fg(Color::Red),
                    (Some(curr), Some(prev)) if curr < prev => Style::default().fg(Color::Green),
                    _ => Style::default().fg(Color::White),
                };
                (fee_str, style)
            } else {
                (String::new(), Style::default())
            };

            let gas_bar =
                build_gas_bar(entry.gas_used, entry.gas_limit, DEFAULT_ELASTICITY, GAS_BAR_CHARS);

            let (time_diff_str, time_style) = entry.time_diff_ms.map_or_else(
                || ("-".to_string(), Style::default().fg(Color::DarkGray)),
                |ms| (format!("+{ms}ms"), Style::default().fg(time_diff_color(ms))),
            );

            let first_fb_style = if entry.index == 0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                Cell::from(entry.block_number.to_string()).style(first_fb_style),
                Cell::from(entry.index.to_string()).style(first_fb_style),
                Cell::from(entry.tx_count.to_string()).style(first_fb_style),
                Cell::from(format_gas(entry.gas_used)),
                Cell::from(base_fee_str).style(base_fee_style),
                Cell::from(time_diff_str).style(time_style),
                Cell::from(gas_bar),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(7),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Min(GAS_BAR_CHARS as u16 + 2),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_stateful_widget(table, inner, &mut table_state.clone());
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        let s = n.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    }
}

fn format_ms(ms: u64) -> String {
    if ms >= 1000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{ms}ms") }
}

fn format_elapsed(secs: f64) -> String {
    let total = secs as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}
