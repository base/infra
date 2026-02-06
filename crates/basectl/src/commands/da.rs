use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use base_flashtypes::Flashblock;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use tokio::sync::mpsc;

use super::common::{
    backlog_size_color, block_color, format_bytes, format_duration, format_rate,
    render_batches_table, BlockContribution, DaTracker, LoadingState,
};
use crate::config::ChainConfig;
use crate::rpc::{
    fetch_initial_backlog_with_progress, fetch_sync_status, run_block_fetcher,
    run_l1_batcher_watcher, BacklogFetchResult, BlobSubmission, BlockDaInfo,
};
use crate::tui::{restore_terminal, setup_terminal, AppFrame, Keybinding};
use futures_util::StreamExt;
use tokio_tungstenite::connect_async;

const KEYBINDINGS: &[Keybinding] = &[
    Keybinding::new("q", "Quit"),
    Keybinding::new("?", "Toggle help"),
    Keybinding::new("↑/k ↓/j", "Navigate"),
    Keybinding::new("←/h →/l", "Switch panel"),
    Keybinding::new("Tab", "Next panel"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Blocks,
    Batches,
}

struct DaState {
    chain_name: String,
    show_help: bool,
    has_op_node: bool,

    da: DaTracker,
    current_block: Option<u64>,

    total_l2_da_bytes: u64,
    total_l1_blob_bytes: u64,
    total_actual_blobs: u64,

    selected_panel: Panel,
    selected_row: usize,
    highlighted_block: Option<u64>,
}

impl DaState {
    fn new(chain_name: String, has_op_node: bool) -> Self {
        Self {
            chain_name,
            show_help: false,
            has_op_node,
            da: DaTracker::new(),
            current_block: None,
            total_l2_da_bytes: 0,
            total_l1_blob_bytes: 0,
            total_actual_blobs: 0,
            selected_panel: Panel::Blocks,
            selected_row: 0,
            highlighted_block: None,
        }
    }

    fn update_highlighted_block(&mut self) {
        self.highlighted_block = match self.selected_panel {
            Panel::Blocks => self
                .da
                .block_contributions
                .get(self.selected_row)
                .map(|c| c.block_number),
            Panel::Batches => None,
        };
    }

    fn panel_len(&self, panel: Panel) -> usize {
        match panel {
            Panel::Blocks => self.da.block_contributions.len(),
            Panel::Batches => self.da.batch_submissions.len(),
        }
    }

    fn scroll_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
            self.update_highlighted_block();
        }
    }

    fn scroll_down(&mut self) {
        let max = self.panel_len(self.selected_panel).saturating_sub(1);
        if self.selected_row < max {
            self.selected_row += 1;
            self.update_highlighted_block();
        }
    }

    fn next_panel(&mut self) {
        self.selected_panel = match self.selected_panel {
            Panel::Blocks => Panel::Batches,
            Panel::Batches => Panel::Blocks,
        };
        self.selected_row = 0;
        self.update_highlighted_block();
    }

    fn get_copyable_value(&self) -> Option<String> {
        match self.selected_panel {
            Panel::Blocks => self
                .da
                .block_contributions
                .get(self.selected_row)
                .map(|c| c.block_number.to_string()),
            Panel::Batches => self
                .da
                .batch_submissions
                .get(self.selected_row)
                .and_then(|b| b.l1_block_number)
                .map(|n| n.to_string()),
        }
    }

    fn process_flashblock(&mut self, fb: &Flashblock) -> Option<u64> {
        let block_number = fb.metadata.block_number;
        let da_bytes: u64 = fb.diff.transactions.iter().map(|tx| tx.len() as u64).sum();

        if fb.index == 0 {
            let prev = self.current_block.filter(|&prev| prev < block_number);
            self.current_block = Some(block_number);
            self.da.add_block(block_number, da_bytes);
            prev
        } else if let Some(contrib) = self
            .da
            .block_contributions
            .iter_mut()
            .find(|c| c.block_number == block_number)
        {
            contrib.da_bytes = contrib.da_bytes.saturating_add(da_bytes);
            if block_number > self.da.safe_l2_block {
                self.da.da_backlog_bytes = self.da.da_backlog_bytes.saturating_add(da_bytes);
            }
            self.da.growth_tracker.add_sample(da_bytes);
            None
        } else {
            self.da.add_block(block_number, da_bytes);
            None
        }
    }

    fn update_safe_head(&mut self, safe_block: u64) {
        if let Some(submission) = self.da.update_safe_head(safe_block) {
            self.total_l2_da_bytes = self.total_l2_da_bytes.saturating_add(submission.da_bytes);
        }
    }

    fn record_l1_blob_submission(&mut self, blob_sub: &BlobSubmission) {
        self.total_actual_blobs = self.total_actual_blobs.saturating_add(blob_sub.blob_count);
        self.total_l1_blob_bytes = self.total_l1_blob_bytes.saturating_add(blob_sub.l1_blob_bytes);
        self.da.record_l1_blob_submission(blob_sub);
    }

    fn compression_ratio(&self) -> Option<f64> {
        if self.total_l1_blob_bytes > 0 {
            Some(self.total_l2_da_bytes as f64 / self.total_l1_blob_bytes as f64)
        } else {
            None
        }
    }

    fn backlog_blocks(&self) -> impl Iterator<Item = &BlockContribution> {
        self.da
            .block_contributions
            .iter()
            .filter(|c| c.block_number > self.da.safe_l2_block)
    }

    fn time_since_last_blob(&self) -> Option<Duration> {
        self.da.last_blob_time.map(|t| t.elapsed())
    }
}

pub async fn run_da_view(config: &ChainConfig) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_da_loop(&mut terminal, config).await;
    restore_terminal(&mut terminal)?;
    result
}

async fn run_ws_connection(url: String, tx: mpsc::Sender<Flashblock>) -> Result<()> {
    let (ws_stream, _) = connect_async(&url).await?;
    let (_, mut read) = ws_stream.split();

    while let Some(msg) = read.next().await {
        let msg = msg?;
        if !msg.is_binary() && !msg.is_text() {
            continue;
        }
        let fb = Flashblock::try_decode_message(msg.into_data())?;
        if tx.send(fb).await.is_err() {
            break;
        }
    }
    Ok(())
}

async fn run_da_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: &ChainConfig,
) -> Result<()> {
    let (fb_tx, mut fb_rx) = mpsc::channel::<Flashblock>(100);
    let (sync_tx, mut sync_rx) = mpsc::channel::<u64>(10);
    let (backlog_tx, mut backlog_rx) = mpsc::channel::<BacklogFetchResult>(100);
    let (block_req_tx, block_req_rx) = mpsc::channel::<u64>(100);
    let (block_res_tx, mut block_res_rx) = mpsc::channel::<BlockDaInfo>(100);
    let (blob_tx, mut blob_rx) = mpsc::channel::<BlobSubmission>(100);

    let has_op_node = config.op_node_rpc.is_some();
    let mut state = DaState::new(config.name.clone(), has_op_node);
    let mut backlog_loaded = !has_op_node;
    let mut buffered_flashblocks: Vec<Flashblock> = Vec::new();
    let mut loading_state: Option<LoadingState> = None;

    let ws_url = config.flashblocks_ws.to_string();
    tokio::spawn(async move {
        let _ = run_ws_connection(ws_url, fb_tx).await;
    });

    let rpc_url = config.rpc.to_string();
    tokio::spawn(async move {
        run_block_fetcher(rpc_url, block_req_rx, block_res_tx).await;
    });

    if let Some(batcher_addr) = config.batcher_address {
        let l1_rpc = config.l1_rpc.to_string();
        tokio::spawn(async move {
            run_l1_batcher_watcher(l1_rpc, batcher_addr, blob_tx).await;
        });
    }

    if let Some(ref rpc_url) = config.op_node_rpc {
        let l2_rpc = config.rpc.to_string();
        let rpc_url = rpc_url.to_string();
        let backlog_tx = backlog_tx;
        tokio::spawn(async move {
            fetch_initial_backlog_with_progress(l2_rpc, rpc_url, backlog_tx).await;
        });
    }

    if let Some(ref rpc_url) = config.op_node_rpc {
        let rpc_url = rpc_url.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                if let Ok(status) = fetch_sync_status(&rpc_url).await
                    && sync_tx.send(status.safe_l2.number).await.is_err()
                {
                    break;
                }
            }
        });
    }

    loop {
        if backlog_loaded {
            terminal.draw(|f| draw_da_ui(f, &state))?;
        } else {
            terminal.draw(|f| draw_loading_screen(f, &config.name, loading_state.as_ref()))?;
        }

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('?') => state.show_help = !state.show_help,
                KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                KeyCode::Down | KeyCode::Char('j') => state.scroll_down(),
                KeyCode::Tab
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
                    state.next_panel();
                }
                KeyCode::Char('y') => {
                    if let Some(value) = state.get_copyable_value() {
                        if let Ok(mut clipboard) = Clipboard::new() {
                            let _ = clipboard.set_text(value);
                        }
                    }
                }
                _ => {}
            }
        }

        while let Ok(result) = backlog_rx.try_recv() {
            match result {
                BacklogFetchResult::Progress(progress) => {
                    loading_state = Some(LoadingState {
                        current_block: progress.current_block,
                        total_blocks: progress.total_blocks,
                    });
                }
                BacklogFetchResult::Complete(initial) => {
                    state.da.set_initial_backlog(initial.safe_block, initial.da_bytes);
                    for fb in std::mem::take(&mut buffered_flashblocks) {
                        if let Some(prev_block) = state.process_flashblock(&fb) {
                            let _ = block_req_tx.try_send(prev_block);
                        }
                    }
                    backlog_loaded = true;
                }
                BacklogFetchResult::Error(_) => {
                    for fb in std::mem::take(&mut buffered_flashblocks) {
                        let _ = state.process_flashblock(&fb);
                    }
                    backlog_loaded = true;
                }
            }
        }

        while let Ok(fb) = fb_rx.try_recv() {
            if backlog_loaded {
                if let Some(prev_block) = state.process_flashblock(&fb) {
                    let _ = block_req_tx.try_send(prev_block);
                }
            } else {
                buffered_flashblocks.push(fb);
            }
        }

        while let Ok(block_info) = block_res_rx.try_recv() {
            state.da.update_block_da(block_info.block_number, block_info.da_bytes);
        }

        while let Ok(safe_block) = sync_rx.try_recv() {
            state.update_safe_head(safe_block);
        }

        while let Ok(blob_sub) = blob_rx.try_recv() {
            state.record_l1_blob_submission(&blob_sub);
        }
    }

    Ok(())
}

fn draw_loading_screen(f: &mut Frame, chain_name: &str, loading: Option<&LoadingState>) {
    let area = f.area();

    let block = Block::default()
        .title(format!(" {chain_name} - DA Monitor "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let center_y = inner.height / 2;
    let text_area = Rect::new(inner.x, inner.y + center_y.saturating_sub(2), inner.width, 4);

    let (line1, line2) = match loading {
        Some(state) if state.total_blocks > 0 => {
            let pct = (state.current_block as f64 / state.total_blocks as f64 * 100.0) as u64;
            let bar_width = 30usize;
            let filled = (pct as usize * bar_width / 100).min(bar_width);
            let bar = format!(
                "[{}{}] {}%",
                "█".repeat(filled),
                "░".repeat(bar_width - filled),
                pct
            );
            (
                Line::from(vec![
                    Span::styled("Loading DA backlog: ", Style::default().fg(Color::White)),
                    Span::styled(
                        format!("{}/{} blocks", state.current_block, state.total_blocks),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(Span::styled(bar, Style::default().fg(Color::Green))),
            )
        }
        Some(_) => (
            Line::from(Span::styled(
                "Fetching sync status...",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
        ),
        None => (
            Line::from(Span::styled(
                "Connecting...",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
        ),
    };

    let text = Paragraph::new(vec![line1, Line::from(""), line2]).alignment(Alignment::Center);
    f.render_widget(text, text_area);
}

fn draw_da_ui(f: &mut Frame, state: &DaState) {
    let layout = AppFrame::split_layout(f.area(), state.show_help);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Min(10),
        ])
        .split(layout.content);

    draw_backlog_bar(f, chunks[0], state);
    draw_stats_panel(f, chunks[1], state);
    draw_columns(f, chunks[2], state);

    AppFrame::render(f, &layout, &state.chain_name, KEYBINDINGS, None);
}

fn draw_backlog_bar(f: &mut Frame, area: Rect, state: &DaState) {
    let block = Block::default()
        .title(" DA Backlog ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width < 10 || inner.height < 1 {
        return;
    }

    let bar_width = inner.width.saturating_sub(12) as usize;
    let backlog_blocks: Vec<_> = state.backlog_blocks().collect();

    if backlog_blocks.is_empty() || state.da.da_backlog_bytes == 0 {
        let empty_bar = "░".repeat(bar_width);
        let text = format!("{empty_bar} {:>8}", format_bytes(0));
        let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    }

    let total_backlog = state.da.da_backlog_bytes;
    let mut spans: Vec<Span> = Vec::new();
    let mut chars_used = 0usize;

    for contrib in backlog_blocks.iter().rev() {
        let color = block_color(contrib.block_number);
        let is_highlighted = state.highlighted_block == Some(contrib.block_number);

        let proportion = contrib.da_bytes as f64 / total_backlog as f64;
        let char_count = ((proportion * bar_width as f64).round() as usize).max(1);
        let char_count = char_count.min(bar_width - chars_used);

        if char_count > 0 {
            let (glyph, style) = if is_highlighted {
                ("▓", Style::default().fg(Color::White).bg(color))
            } else {
                ("█", Style::default().fg(color))
            };
            spans.push(Span::styled(glyph.repeat(char_count), style));
            chars_used += char_count;
        }

        if chars_used >= bar_width {
            break;
        }
    }

    if chars_used < bar_width {
        spans.push(Span::styled(
            "░".repeat(bar_width - chars_used),
            Style::default().fg(Color::DarkGray),
        ));
    }

    let backlog_color = backlog_size_color(total_backlog);
    spans.push(Span::styled(
        format!(" {:>8}", format_bytes(total_backlog)),
        Style::default().fg(backlog_color).add_modifier(Modifier::BOLD),
    ));

    let line = Line::from(spans);
    let para = Paragraph::new(line);
    f.render_widget(para, inner);
}

fn draw_stats_panel(f: &mut Frame, area: Rect, state: &DaState) {
    let block = Block::default()
        .title(" Statistics ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
        ])
        .split(inner);

    let growth_30s = state.da.growth_tracker.rate_over(Duration::from_secs(30));
    let growth_2m = state.da.growth_tracker.rate_over(Duration::from_secs(120));
    let growth_5m = state.da.growth_tracker.rate_over(Duration::from_secs(300));

    let growth_text = vec![
        Line::from(Span::styled(
            "Growth",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("30s: {}", format_rate(growth_30s))),
        Line::from(format!(" 2m: {}", format_rate(growth_2m))),
        Line::from(format!(" 5m: {}", format_rate(growth_5m))),
    ];
    f.render_widget(Paragraph::new(growth_text), cols[0]);

    let burn_30s = state.da.burn_tracker.rate_over(Duration::from_secs(30));
    let burn_2m = state.da.burn_tracker.rate_over(Duration::from_secs(120));
    let burn_5m = state.da.burn_tracker.rate_over(Duration::from_secs(300));

    let burn_text = vec![
        Line::from(Span::styled(
            "Burn",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("30s: {}", format_rate(burn_30s))),
        Line::from(format!(" 2m: {}", format_rate(burn_2m))),
        Line::from(format!(" 5m: {}", format_rate(burn_5m))),
    ];
    f.render_widget(Paragraph::new(burn_text), cols[1]);

    let net_30s = compute_net_rate(growth_30s, burn_30s);
    let net_2m = compute_net_rate(growth_2m, burn_2m);
    let net_5m = compute_net_rate(growth_5m, burn_5m);

    let net_text = vec![
        Line::from(Span::styled(
            "Net",
            Style::default()
                .fg(Color::Rgb(0, 82, 255))
                .add_modifier(Modifier::BOLD),
        )),
        format_net_rate(net_30s, "30s"),
        format_net_rate(net_2m, " 2m"),
        format_net_rate(net_5m, " 5m"),
    ];
    f.render_widget(Paragraph::new(net_text), cols[2]);

    let time_since = state
        .time_since_last_blob()
        .map(format_duration)
        .unwrap_or_else(|| "-".to_string());

    let compression = state
        .compression_ratio()
        .map(|r| format!("{r:.2}x"))
        .unwrap_or_else(|| "-".to_string());

    let blob_text = vec![
        Line::from(Span::styled(
            "Compression",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Ratio: {compression}")),
        Line::from(format!("Blobs: {}", state.total_actual_blobs)),
        Line::from(format!("Since: {time_since}")),
    ];
    f.render_widget(Paragraph::new(blob_text), cols[3]);
}

fn compute_net_rate(growth: Option<f64>, burn: Option<f64>) -> Option<f64> {
    match (growth, burn) {
        (Some(g), Some(b)) => Some(g - b),
        (Some(g), None) => Some(g),
        (None, Some(b)) => Some(-b),
        (None, None) => None,
    }
}

fn format_net_rate(rate: Option<f64>, label: &str) -> Line<'static> {
    match rate {
        Some(r) => {
            let (sign, abs_r, color) = if r >= 0.0 {
                ("+", r, Color::Green)
            } else {
                ("-", -r, Color::Red)
            };
            let formatted = if abs_r >= 1_000_000.0 {
                format!("{sign}{:.1}M/s", abs_r / 1_000_000.0)
            } else if abs_r >= 1_000.0 {
                format!("{sign}{:.1}K/s", abs_r / 1_000.0)
            } else {
                format!("{sign}{abs_r:.0}B/s")
            };
            Line::from(vec![
                Span::raw(format!("{label}: ")),
                Span::styled(formatted, Style::default().fg(color)),
            ])
        }
        None => Line::from(format!("{label}: -")),
    }
}

fn draw_columns(f: &mut Frame, area: Rect, state: &DaState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(area);

    draw_blocks_column(f, cols[0], state);

    let highlighted_batch_idx = state
        .highlighted_block
        .and_then(|b| state.da.find_batch_for_block(b));

    render_batches_table(
        f,
        cols[1],
        &state.da.batch_submissions,
        state.selected_panel == Panel::Batches,
        state.selected_row,
        highlighted_batch_idx,
        state.has_op_node,
        "Batches Posted (DA ↓)",
    );
}

fn draw_blocks_column(f: &mut Frame, area: Rect, state: &DaState) {
    let is_active = state.selected_panel == Panel::Blocks;
    let border_color = if is_active {
        Color::Rgb(100, 255, 100)
    } else {
        Color::Green
    };

    let block = Block::default()
        .title(" Blocks Produced (DA ↑) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let header = ratatui::widgets::Row::new(vec![
        ratatui::widgets::Cell::from("Block").style(header_style),
        ratatui::widgets::Cell::from("DA Bytes").style(header_style),
        ratatui::widgets::Cell::from("Age").style(header_style),
    ]);

    let rows: Vec<ratatui::widgets::Row> = state
        .da
        .block_contributions
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, contrib)| {
            let color = block_color(contrib.block_number);
            let in_backlog = contrib.block_number > state.da.safe_l2_block;
            let is_selected = is_active && idx == state.selected_row;
            let is_highlighted = state.highlighted_block == Some(contrib.block_number);

            let style = if is_selected {
                Style::default().fg(color).bg(Color::Rgb(60, 60, 80))
            } else if is_highlighted {
                Style::default().fg(color).bg(Color::Rgb(40, 40, 60))
            } else if in_backlog {
                Style::default().fg(color)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            ratatui::widgets::Row::new(vec![
                ratatui::widgets::Cell::from(contrib.block_number.to_string()),
                ratatui::widgets::Cell::from(format_bytes(contrib.da_bytes)),
                ratatui::widgets::Cell::from(format_duration(contrib.timestamp.elapsed())),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Min(8),
    ];

    let table = ratatui::widgets::Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}
