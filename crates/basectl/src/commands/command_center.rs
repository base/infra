use std::collections::VecDeque;
use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use base_flashtypes::Flashblock;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;

use super::common::{
    backlog_size_color, block_color, build_gas_bar, format_bytes, format_duration, format_gas,
    format_rate, BlockContribution, DaTracker, FlashblockEntry, LoadingState,
};
use crate::config::ChainConfig;
use crate::l1_client::{fetch_full_system_config, FullSystemConfig};
use crate::rpc::{
    fetch_initial_backlog_with_progress, fetch_sync_status, run_block_fetcher,
    run_l1_batcher_watcher, BacklogFetchResult, BlobSubmission, BlockDaInfo,
};
use crate::tui::{restore_terminal, setup_terminal};

const MAX_FLASHBLOCKS: usize = 100;
const GAS_BAR_CHARS: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Flashblocks,
    Blocks,
    Batches,
}

struct CommandCenterState {
    chain_name: String,
    has_op_node: bool,

    flashblocks: VecDeque<FlashblockEntry>,
    current_block: Option<u64>,
    current_gas_limit: u64,
    current_base_fee: Option<u128>,

    da_tracker: DaTracker,

    system_config: Option<FullSystemConfig>,
    backlog_loaded: bool,

    selected_panel: Panel,
    selected_row: usize,
    highlighted_block: Option<u64>,
}

impl CommandCenterState {
    fn new(chain_name: String, has_op_node: bool) -> Self {
        Self {
            chain_name,
            has_op_node,
            flashblocks: VecDeque::with_capacity(MAX_FLASHBLOCKS),
            current_block: None,
            current_gas_limit: 0,
            current_base_fee: None,
            da_tracker: DaTracker::new(),
            system_config: None,
            backlog_loaded: !has_op_node,
            selected_panel: Panel::Blocks,
            selected_row: 0,
            highlighted_block: None,
        }
    }

    fn update_highlighted_block(&mut self) {
        self.highlighted_block = match self.selected_panel {
            Panel::Flashblocks => self
                .flashblocks
                .get(self.selected_row)
                .map(|fb| fb.block_number),
            Panel::Blocks => self
                .da_tracker
                .block_contributions
                .get(self.selected_row)
                .map(|c| c.block_number),
            Panel::Batches => None,
        };
    }

    fn panel_len(&self, panel: Panel) -> usize {
        match panel {
            Panel::Flashblocks => self.flashblocks.len(),
            Panel::Blocks => self.da_tracker.block_contributions.len(),
            Panel::Batches => self.da_tracker.batch_submissions.len(),
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
            Panel::Flashblocks => Panel::Blocks,
            Panel::Blocks => Panel::Batches,
            Panel::Batches => Panel::Flashblocks,
        };
        self.selected_row = 0;
        self.update_highlighted_block();
    }

    fn prev_panel(&mut self) {
        self.selected_panel = match self.selected_panel {
            Panel::Flashblocks => Panel::Batches,
            Panel::Blocks => Panel::Flashblocks,
            Panel::Batches => Panel::Blocks,
        };
        self.selected_row = 0;
        self.update_highlighted_block();
    }

    fn get_copyable_value(&self) -> Option<String> {
        match self.selected_panel {
            Panel::Flashblocks => self
                .flashblocks
                .get(self.selected_row)
                .map(|fb| fb.block_number.to_string()),
            Panel::Blocks => self
                .da_tracker
                .block_contributions
                .get(self.selected_row)
                .map(|c| c.block_number.to_string()),
            Panel::Batches => self
                .da_tracker
                .batch_submissions
                .get(self.selected_row)
                .and_then(|b| b.l1_block_number)
                .map(|n| n.to_string()),
        }
    }

    fn set_initial_backlog(&mut self, safe_block: u64, da_bytes: u64) {
        self.da_tracker.set_initial_backlog(safe_block, da_bytes);
    }

    fn add_flashblock_display(&mut self, fb: &Flashblock) {
        let now = Local::now();

        let base_fee = fb
            .base
            .as_ref()
            .map(|base| base.base_fee_per_gas.try_into().unwrap_or(u128::MAX));

        let prev_base_fee = self.current_base_fee;

        if let Some(ref base) = fb.base {
            self.current_gas_limit = base.gas_limit;
            self.current_base_fee = base_fee;
        }

        let time_diff_ms = self
            .flashblocks
            .front()
            .map(|prev| (now - prev.timestamp).num_milliseconds());

        let entry = FlashblockEntry {
            block_number: fb.metadata.block_number,
            index: fb.index,
            tx_count: fb.diff.transactions.len(),
            gas_used: fb.diff.gas_used,
            gas_limit: self.current_gas_limit,
            base_fee,
            prev_base_fee,
            timestamp: now,
            time_diff_ms,
        };
        self.flashblocks.push_front(entry);
        if self.flashblocks.len() > MAX_FLASHBLOCKS {
            self.flashblocks.pop_back();
        }
    }

    fn process_flashblock_da(&mut self, fb: &Flashblock) -> Option<u64> {
        let block_number = fb.metadata.block_number;
        let da_bytes: u64 = fb.diff.transactions.iter().map(|tx| tx.len() as u64).sum();

        if fb.index == 0 {
            let prev = self.current_block.filter(|&prev| prev < block_number);
            self.current_block = Some(block_number);
            self.da_tracker.add_block(block_number, da_bytes);
            prev
        } else {
            if let Some(contrib) = self
                .da_tracker
                .block_contributions
                .iter_mut()
                .find(|c| c.block_number == block_number)
            {
                contrib.da_bytes = contrib.da_bytes.saturating_add(da_bytes);
                if block_number > self.da_tracker.safe_l2_block {
                    self.da_tracker.da_backlog_bytes = self.da_tracker.da_backlog_bytes.saturating_add(da_bytes);
                }
                self.da_tracker.growth_tracker.add_sample(da_bytes);
            }
            None
        }
    }



    fn update_safe_head(&mut self, safe_block: u64) {
        self.da_tracker.update_safe_head(safe_block);
    }

    fn record_l1_blob_submission(&mut self, blob_sub: &BlobSubmission) {
        self.da_tracker.record_l1_blob_submission(blob_sub);
    }

    fn find_batch_for_block(&self, block_number: u64) -> Option<usize> {
        self.da_tracker.find_batch_for_block(block_number)
    }

    fn update_block_da(&mut self, block_number: u64, accurate_da_bytes: u64) {
        self.da_tracker.update_block_da(block_number, accurate_da_bytes);
    }

    fn backlog_blocks(&self) -> impl Iterator<Item = &BlockContribution> {
        self.da_tracker
            .block_contributions
            .iter()
            .filter(|c| c.block_number > self.da_tracker.safe_l2_block)
    }

    fn time_since_last_blob(&self) -> Option<Duration> {
        self.da_tracker.last_blob_time.map(|t| t.elapsed())
    }
}

pub async fn run_command_center(config: &ChainConfig) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_command_center_loop(&mut terminal, config).await;
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

async fn run_command_center_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: &ChainConfig,
) -> Result<()> {
    let (fb_tx, mut fb_rx) = mpsc::channel::<Flashblock>(100);
    let (sync_tx, mut sync_rx) = mpsc::channel::<u64>(10);
    let (backlog_tx, mut backlog_rx) = mpsc::channel::<BacklogFetchResult>(100);
    let (block_req_tx, block_req_rx) = mpsc::channel::<u64>(100);
    let (block_res_tx, mut block_res_rx) = mpsc::channel::<BlockDaInfo>(100);
    let (config_tx, mut config_rx) = mpsc::channel::<FullSystemConfig>(1);

    let mut state = CommandCenterState::new(config.name.clone(), config.op_node_rpc.is_some());
    let mut loading_state: Option<LoadingState> = None;
    let mut buffered_flashblocks: Vec<Flashblock> = Vec::new();

    let ws_url = config.flashblocks_ws.to_string();
    tokio::spawn(async move {
        let _ = run_ws_connection(ws_url, fb_tx).await;
    });

    {
        let l2_rpc = config.rpc.to_string();
        tokio::spawn(async move {
            run_block_fetcher(l2_rpc, block_req_rx, block_res_tx).await;
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

    {
        let l1_rpc = config.l1_rpc.to_string();
        let system_config_addr = config.system_config;
        tokio::spawn(async move {
            if let Ok(cfg) = fetch_full_system_config(&l1_rpc, system_config_addr).await {
                let _ = config_tx.send(cfg).await;
            }
        });
    }

    let (blob_tx, mut blob_rx) = mpsc::channel::<BlobSubmission>(100);
    if let Some(batcher_addr) = config.batcher_address {
        let l1_rpc = config.l1_rpc.to_string();
        tokio::spawn(async move {
            run_l1_batcher_watcher(l1_rpc, batcher_addr, blob_tx).await;
        });
    }

    loop {
        terminal.draw(|f| draw_command_center(f, &state, loading_state.as_ref()))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                KeyCode::Down | KeyCode::Char('j') => state.scroll_down(),
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => state.next_panel(),
                KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => state.prev_panel(),
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

        if let Ok(cfg) = config_rx.try_recv() {
            state.system_config = Some(cfg);
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
                    state.set_initial_backlog(initial.safe_block, initial.da_bytes);
                    for fb in std::mem::take(&mut buffered_flashblocks) {
                        if let Some(prev_block) = state.process_flashblock_da(&fb) {
                            let _ = block_req_tx.try_send(prev_block);
                        }
                    }
                    state.backlog_loaded = true;
                }
                BacklogFetchResult::Error(_) => {
                    for fb in std::mem::take(&mut buffered_flashblocks) {
                        let _ = state.process_flashblock_da(&fb);
                    }
                    state.backlog_loaded = true;
                }
            }
        }

        while let Ok(fb) = fb_rx.try_recv() {
            state.add_flashblock_display(&fb);
            if state.backlog_loaded {
                if let Some(prev_block) = state.process_flashblock_da(&fb) {
                    let _ = block_req_tx.try_send(prev_block);
                }
            } else {
                buffered_flashblocks.push(fb);
            }
        }

        while let Ok(block_info) = block_res_rx.try_recv() {
            state.update_block_da(block_info.block_number, block_info.da_bytes);
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

fn draw_command_center(f: &mut Frame, state: &CommandCenterState, loading: Option<&LoadingState>) {
    let area = f.area();

    let block = Block::default()
        .title(format!(" {} - Command Center ", state.chain_name))
        .title_bottom(Line::from(" [q] Quit ").alignment(Alignment::Center))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0, 82, 255)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(10),
        ])
        .split(inner);

    draw_da_backlog_bar(f, rows[0], state, loading);

    let info_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    draw_config_panel(f, info_cols[0], state);
    draw_stats_panel(f, info_cols[1], state);

    let main_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(rows[2]);

    draw_flashblocks_panel(f, main_cols[0], state);
    draw_blocks_panel(f, main_cols[1], state);
    draw_batches_panel(f, main_cols[2], state);
}

fn draw_da_backlog_bar(f: &mut Frame, area: Rect, state: &CommandCenterState, loading: Option<&LoadingState>) {
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

    if !state.backlog_loaded {
        let (line1, line2) = match loading {
            Some(ls) if ls.total_blocks > 0 => {
                let pct = (ls.current_block as f64 / ls.total_blocks as f64 * 100.0) as u64;
                let filled = (pct as usize * bar_width / 100).min(bar_width);
                let bar = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(bar_width - filled),
                );
                (
                    Line::from(Span::styled(bar, Style::default().fg(Color::Cyan))),
                    Line::from(Span::styled(
                        format!(" Loading {}/{}", ls.current_block, ls.total_blocks),
                        Style::default().fg(Color::Cyan),
                    )),
                )
            }
            _ => (
                Line::from(Span::styled(
                    "░".repeat(bar_width),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(" Loading...", Style::default().fg(Color::Yellow))),
            ),
        };
        let para = Paragraph::new(vec![line1, line2]);
        f.render_widget(para, inner);
        return;
    }

    let backlog_blocks: Vec<_> = state.backlog_blocks().collect();

    if backlog_blocks.is_empty() || state.da_tracker.da_backlog_bytes == 0 {
        let empty_bar = "░".repeat(bar_width);
        let text = format!("{empty_bar} {:>8}", format_bytes(0));
        let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
        f.render_widget(para, inner);
        return;
    }

    let total_backlog = state.da_tracker.da_backlog_bytes;
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

fn draw_stats_panel(f: &mut Frame, area: Rect, state: &CommandCenterState) {
    let block = Block::default()
        .title(" DA Stats ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let growth_2m = state.da_tracker.growth_tracker.rate_over(Duration::from_secs(120));
    let burn_2m = state.da_tracker.burn_tracker.rate_over(Duration::from_secs(120));
    let net_2m = compute_net_rate(growth_2m, burn_2m);

    let time_since = state
        .time_since_last_blob()
        .map(format_duration)
        .unwrap_or_else(|| "-".to_string());

    let label_style = Style::default().fg(Color::Yellow);
    let lines = vec![
        Line::from(vec![
            Span::styled("Growth: ", label_style),
            Span::styled(format_rate(growth_2m), Style::default().fg(Color::Green)),
            Span::raw("   "),
            Span::styled("Burn: ", label_style),
            Span::styled(format_rate(burn_2m), Style::default().fg(Color::Red)),
        ]),
        Line::from(vec![
            Span::styled("Net: ", label_style),
            format_net_rate_span(net_2m),
            Span::raw("   "),
            Span::styled("Last blob: ", label_style),
            Span::raw(format!("{time_since} ago")),
        ]),
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn draw_flashblocks_panel(f: &mut Frame, area: Rect, state: &CommandCenterState) {
    let is_active = state.selected_panel == Panel::Flashblocks;
    let border_color = if is_active {
        Color::Rgb(100, 180, 255)
    } else {
        Color::Rgb(0, 82, 255)
    };

    let block = Block::default()
        .title(" Flashblocks ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("Block").style(header_style),
        Cell::from("FB#").style(header_style),
        Cell::from("Txs").style(header_style),
        Cell::from("Gas").style(header_style),
        Cell::from("Base Fee").style(header_style),
        Cell::from("Δt").style(header_style),
        Cell::from("Fill").style(header_style),
        Cell::from("Time").style(header_style),
    ]);

    let elasticity = state
        .system_config
        .as_ref()
        .and_then(|c| c.eip1559_elasticity)
        .unwrap_or(10) as u64;

    let rows: Vec<Row> = state
        .flashblocks
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, fb)| {
            let is_selected = is_active && idx == state.selected_row;
            let is_highlighted = state.highlighted_block == Some(fb.block_number);

            let style = if is_selected {
                Style::default().bg(Color::Rgb(60, 60, 80))
            } else if is_highlighted {
                Style::default().bg(Color::Rgb(40, 40, 60))
            } else if fb.index == 0 {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let gas_bar = build_gas_bar(fb.gas_used, fb.gas_limit, elasticity, GAS_BAR_CHARS);

            let base_fee_cell = if fb.index == 0 {
                let base_fee_str = fb
                    .base_fee
                    .map(format_gwei)
                    .unwrap_or_else(|| "-".to_string());

                let fee_style = match (fb.base_fee, fb.prev_base_fee) {
                    (Some(current), Some(prev)) if current > prev => {
                        Style::default().fg(Color::Green)
                    }
                    (Some(current), Some(prev)) if current < prev => {
                        Style::default().fg(Color::Red)
                    }
                    _ => Style::default(),
                };
                Cell::from(base_fee_str).style(fee_style)
            } else {
                Cell::from(String::new())
            };

            let delta_cell = fb.time_diff_ms.map_or_else(
                || Cell::from("-".to_string()),
                |ms| {
                    let color = if (150..=250).contains(&ms) {
                        Color::Green
                    } else if (100..150).contains(&ms) || (250..300).contains(&ms) {
                        Color::Yellow
                    } else {
                        Color::Red
                    };
                    Cell::from(format!("+{ms}ms")).style(Style::default().fg(color))
                },
            );

            Row::new(vec![
                Cell::from(fb.block_number.to_string()),
                Cell::from(fb.index.to_string()),
                Cell::from(fb.tx_count.to_string()),
                Cell::from(format_gas(fb.gas_used)),
                base_fee_cell,
                delta_cell,
                Cell::from(gas_bar),
                Cell::from(fb.timestamp.format("%H:%M:%S").to_string()),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(7),
        Constraint::Length(11),
        Constraint::Length(7),
        Constraint::Length(GAS_BAR_CHARS as u16),
        Constraint::Min(8),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

fn draw_blocks_panel(f: &mut Frame, area: Rect, state: &CommandCenterState) {
    let is_active = state.selected_panel == Panel::Blocks;
    let border_color = if is_active {
        Color::Rgb(100, 255, 100)
    } else {
        Color::Green
    };

    let block = Block::default()
        .title(" Blocks (DA ↑) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header = Row::new(vec![
        Cell::from("Block").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Cell::from("DA").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Cell::from("Age").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ]);

    let rows: Vec<Row> = state
        .da_tracker
        .block_contributions
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, contrib)| {
            let color = block_color(contrib.block_number);
            let in_backlog = contrib.block_number > state.da_tracker.safe_l2_block;
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

            Row::new(vec![
                Cell::from(contrib.block_number.to_string()),
                Cell::from(format_bytes(contrib.da_bytes)),
                Cell::from(format_duration(contrib.timestamp.elapsed())),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Min(6),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

fn draw_batches_panel(f: &mut Frame, area: Rect, state: &CommandCenterState) {
    let is_active = state.selected_panel == Panel::Batches;
    let border_color = if is_active {
        Color::Rgb(255, 100, 100)
    } else {
        Color::Red
    };

    let block = Block::default()
        .title(" Batches (DA ↓) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if !state.has_op_node {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "op_node_rpc not configured",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Set in config file:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "~/.base/config/<name>.yaml",
                Style::default().fg(Color::Yellow),
            )),
        ];
        let para = Paragraph::new(lines).alignment(Alignment::Center);
        f.render_widget(para, inner);
        return;
    }

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("L1 Blk").style(header_style),
        Cell::from("DA").style(header_style),
        Cell::from("Blobs").style(header_style),
        Cell::from("Ratio").style(header_style),
        Cell::from("Age").style(header_style),
    ]);

    let highlighted_batch_idx = state
        .highlighted_block
        .and_then(|b| state.find_batch_for_block(b));

    let rows: Vec<Row> = state
        .da_tracker
        .batch_submissions
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, batch)| {
            let is_selected = is_active && idx == state.selected_row;
            let is_highlighted = highlighted_batch_idx == Some(idx);

            let style = if is_selected {
                Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 80))
            } else if is_highlighted {
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                Cell::from(batch.l1_block_display()),
                Cell::from(format_bytes(batch.da_bytes)),
                Cell::from(batch.blob_count_display()),
                Cell::from(batch.compression_display()),
                Cell::from(format_duration(batch.timestamp.elapsed())),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Min(5),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

fn draw_config_panel(f: &mut Frame, area: Rect, state: &CommandCenterState) {
    let block = Block::default()
        .title(" L1 Config ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let label_style = Style::default().fg(Color::Yellow);
    let value_style = Style::default().fg(Color::White);
    let loading_style = Style::default().fg(Color::DarkGray);

    let lines: Vec<Line> = match &state.system_config {
        Some(cfg) => {
            vec![
                Line::from(vec![
                    Span::styled("Gas Limit: ", label_style),
                    Span::styled(
                        cfg.gas_limit.map_or_else(|| "-".to_string(), format_gas),
                        value_style,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Elasticity: ", label_style),
                    Span::styled(
                        cfg.eip1559_elasticity.map_or_else(|| "-".to_string(), |e| format!("{e}x")),
                        value_style,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Denominator: ", label_style),
                    Span::styled(
                        cfg.eip1559_denominator.map_or_else(|| "-".to_string(), |d| d.to_string()),
                        value_style,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Base Scalar: ", label_style),
                    Span::styled(
                        cfg.basefee_scalar.map_or_else(|| "-".to_string(), |s| s.to_string()),
                        value_style,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Blob Scalar: ", label_style),
                    Span::styled(
                        cfg.blobbasefee_scalar.map_or_else(|| "-".to_string(), |s| s.to_string()),
                        value_style,
                    ),
                ]),
            ]
        }
        None => {
            vec![Line::from(Span::styled("Loading...", loading_style))]
        }
    };

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn compute_net_rate(growth: Option<f64>, burn: Option<f64>) -> Option<f64> {
    match (growth, burn) {
        (Some(g), Some(b)) => Some(g - b),
        (Some(g), None) => Some(g),
        (None, Some(b)) => Some(-b),
        (None, None) => None,
    }
}

fn format_net_rate_span(rate: Option<f64>) -> Span<'static> {
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
            Span::styled(formatted, Style::default().fg(color))
        }
        None => Span::raw("-"),
    }
}

fn format_gwei(wei: u128) -> String {
    let gwei = wei as f64 / 1_000_000_000.0;
    if gwei >= 1.0 {
        format!("{gwei:.2} gwei")
    } else {
        format!("{gwei:.4} gwei")
    }
}
