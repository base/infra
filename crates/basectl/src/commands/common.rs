use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use alloy_primitives::B256;
use chrono::{DateTime, Local};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

use crate::rpc::BlobSubmission;

pub(super) const BLOB_SIZE: u64 = 128 * 1024;
pub(super) const MAX_HISTORY: usize = 1000;

const BLOCK_COLORS: [Color; 24] = [
    Color::Rgb(0, 82, 255),
    Color::Rgb(0, 140, 255),
    Color::Rgb(0, 180, 220),
    Color::Rgb(0, 190, 180),
    Color::Rgb(0, 180, 130),
    Color::Rgb(40, 180, 100),
    Color::Rgb(80, 180, 80),
    Color::Rgb(130, 180, 60),
    Color::Rgb(170, 170, 50),
    Color::Rgb(200, 160, 50),
    Color::Rgb(220, 140, 50),
    Color::Rgb(230, 110, 60),
    Color::Rgb(235, 90, 70),
    Color::Rgb(230, 70, 90),
    Color::Rgb(220, 60, 120),
    Color::Rgb(200, 60, 150),
    Color::Rgb(180, 70, 180),
    Color::Rgb(150, 80, 200),
    Color::Rgb(120, 90, 210),
    Color::Rgb(90, 100, 220),
    Color::Rgb(60, 110, 230),
    Color::Rgb(40, 130, 240),
    Color::Rgb(30, 160, 245),
    Color::Rgb(20, 180, 235),
];

const EIGHTH_BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

// =============================================================================
// Shared Data Types
// =============================================================================

#[derive(Clone)]
pub(super) struct FlashblockEntry {
    pub block_number: u64,
    pub index: u64,
    pub tx_count: usize,
    pub gas_used: u64,
    pub gas_limit: u64,
    pub base_fee: Option<u128>,
    pub prev_base_fee: Option<u128>,
    pub timestamp: DateTime<Local>,
    pub time_diff_ms: Option<i64>,
}

#[derive(Clone)]
pub(super) struct BlockContribution {
    pub block_number: u64,
    pub da_bytes: u64,
    pub timestamp: Instant,
}

#[derive(Clone)]
pub(super) struct BatchSubmission {
    pub da_bytes: u64,
    pub estimated_blobs: u64,
    pub actual_blobs: Option<u64>,
    pub l1_blob_bytes: Option<u64>,
    pub blocks_submitted: u64,
    pub timestamp: Instant,
    pub block_range: (u64, u64),
    pub l1_block_number: Option<u64>,
    pub l1_block_hash: Option<B256>,
}

impl BatchSubmission {
    pub(super) fn new(da_bytes: u64, blocks_submitted: u64, block_range: (u64, u64)) -> Self {
        Self {
            da_bytes,
            estimated_blobs: da_bytes.div_ceil(BLOB_SIZE),
            actual_blobs: None,
            l1_blob_bytes: None,
            blocks_submitted,
            timestamp: Instant::now(),
            block_range,
            l1_block_number: None,
            l1_block_hash: None,
        }
    }

    pub(super) fn record_l1_submission(&mut self, blob_sub: &BlobSubmission) {
        self.actual_blobs = Some(blob_sub.blob_count);
        self.l1_blob_bytes = Some(blob_sub.l1_blob_bytes);
        self.l1_block_number = Some(blob_sub.block_number);
        self.l1_block_hash = Some(blob_sub.block_hash);
    }

    pub(super) fn has_l1_data(&self) -> bool {
        self.actual_blobs.is_some()
    }

    pub(super) fn compression_ratio(&self) -> Option<f64> {
        self.l1_blob_bytes.filter(|&b| b > 0).map(|l1_bytes| self.da_bytes as f64 / l1_bytes as f64)
    }

    pub(super) fn blob_count_display(&self) -> String {
        match self.actual_blobs {
            Some(actual) => actual.to_string(),
            None => format!("~{}", self.estimated_blobs),
        }
    }

    pub(super) fn l1_block_display(&self) -> String {
        self.l1_block_number.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string())
    }

    pub(super) fn compression_display(&self) -> String {
        self.compression_ratio().map(|r| format!("{r:.2}x")).unwrap_or_else(|| "-".to_string())
    }
}

pub(super) struct RateTracker {
    samples: VecDeque<(Instant, u64)>,
}

impl RateTracker {
    pub(super) fn new() -> Self {
        Self { samples: VecDeque::with_capacity(300) }
    }

    pub(super) fn add_sample(&mut self, bytes: u64) {
        let now = Instant::now();
        self.samples.push_back((now, bytes));
        let cutoff = now - Duration::from_secs(300);
        while self.samples.front().is_some_and(|(t, _)| *t < cutoff) {
            self.samples.pop_front();
        }
    }

    pub(super) fn rate_over(&self, duration: Duration) -> Option<f64> {
        let now = Instant::now();
        let cutoff = now - duration;
        let samples_in_window: Vec<_> = self.samples.iter().filter(|(t, _)| *t >= cutoff).collect();

        if samples_in_window.len() < 2 {
            return None;
        }

        let total: u64 = samples_in_window.iter().map(|(_, b)| *b).sum();
        let elapsed = duration.as_secs_f64();
        Some(total as f64 / elapsed)
    }
}

pub(super) struct LoadingState {
    pub current_block: u64,
    pub total_blocks: u64,
}

// =============================================================================
// DA Tracker - Shared State Management for DA Monitoring
// =============================================================================

pub(super) struct DaTracker {
    pub safe_l2_block: u64,
    pub da_backlog_bytes: u64,
    pub block_contributions: VecDeque<BlockContribution>,
    pub batch_submissions: VecDeque<BatchSubmission>,
    pub growth_tracker: RateTracker,
    pub burn_tracker: RateTracker,
    pub last_blob_time: Option<Instant>,
}

impl DaTracker {
    pub(super) fn new() -> Self {
        Self {
            safe_l2_block: 0,
            da_backlog_bytes: 0,
            block_contributions: VecDeque::with_capacity(MAX_HISTORY),
            batch_submissions: VecDeque::with_capacity(MAX_HISTORY),
            growth_tracker: RateTracker::new(),
            burn_tracker: RateTracker::new(),
            last_blob_time: None,
        }
    }

    pub(super) fn set_initial_backlog(&mut self, safe_block: u64, da_bytes: u64) {
        self.safe_l2_block = safe_block;
        self.da_backlog_bytes = da_bytes;
    }

    pub(super) fn add_block(&mut self, block_number: u64, da_bytes: u64) {
        if block_number <= self.safe_l2_block {
            return;
        }

        self.da_backlog_bytes = self.da_backlog_bytes.saturating_add(da_bytes);
        self.growth_tracker.add_sample(da_bytes);

        let contribution = BlockContribution { block_number, da_bytes, timestamp: Instant::now() };
        self.block_contributions.push_front(contribution);
        if self.block_contributions.len() > MAX_HISTORY {
            self.block_contributions.pop_back();
        }
    }

    pub(super) fn update_block_da(&mut self, block_number: u64, accurate_da_bytes: u64) {
        for contrib in &mut self.block_contributions {
            if contrib.block_number == block_number {
                let diff = accurate_da_bytes as i64 - contrib.da_bytes as i64;
                contrib.da_bytes = accurate_da_bytes;

                if block_number > self.safe_l2_block {
                    if diff > 0 {
                        self.da_backlog_bytes = self.da_backlog_bytes.saturating_add(diff as u64);
                    } else {
                        self.da_backlog_bytes =
                            self.da_backlog_bytes.saturating_sub((-diff) as u64);
                    }
                }
                break;
            }
        }
    }

    pub(super) fn update_safe_head(&mut self, safe_block: u64) -> Option<BatchSubmission> {
        if safe_block <= self.safe_l2_block {
            return None;
        }

        let old_safe = self.safe_l2_block;
        self.safe_l2_block = safe_block;

        let mut submitted_bytes: u64 = 0;
        let mut blocks_submitted: u64 = 0;
        let mut min_block = u64::MAX;
        let mut max_block = 0u64;

        for contrib in &self.block_contributions {
            if contrib.block_number > old_safe && contrib.block_number <= safe_block {
                submitted_bytes = submitted_bytes.saturating_add(contrib.da_bytes);
                blocks_submitted += 1;
                min_block = min_block.min(contrib.block_number);
                max_block = max_block.max(contrib.block_number);
            }
        }

        if submitted_bytes > 0 {
            self.da_backlog_bytes = self.da_backlog_bytes.saturating_sub(submitted_bytes);
            self.burn_tracker.add_sample(submitted_bytes);
            self.last_blob_time = Some(Instant::now());

            let submission =
                BatchSubmission::new(submitted_bytes, blocks_submitted, (min_block, max_block));
            self.batch_submissions.push_front(submission.clone());
            if self.batch_submissions.len() > MAX_HISTORY {
                self.batch_submissions.pop_back();
            }
            Some(submission)
        } else {
            None
        }
    }

    pub(super) fn record_l1_blob_submission(&mut self, blob_sub: &BlobSubmission) {
        if let Some(submission) = self.batch_submissions.iter_mut().rev().find(|s| !s.has_l1_data())
        {
            submission.record_l1_submission(blob_sub);
        }
    }

    pub(super) fn find_batch_for_block(&self, block_number: u64) -> Option<usize> {
        self.batch_submissions
            .iter()
            .position(|b| block_number >= b.block_range.0 && block_number <= b.block_range.1)
    }
}

// =============================================================================
// Formatting Functions
// =============================================================================

pub(super) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1}G", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1}M", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0}K", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes}B")
    }
}

pub(super) fn format_gas(gas: u64) -> String {
    if gas >= 1_000_000 {
        format!("{:.1}M", gas as f64 / 1_000_000.0)
    } else if gas >= 1_000 {
        format!("{:.0}K", gas as f64 / 1_000.0)
    } else {
        gas.to_string()
    }
}

pub(super) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub(super) fn format_rate(rate: Option<f64>) -> String {
    match rate {
        Some(r) if r >= 1_000_000.0 => format!("{:.1}M/s", r / 1_000_000.0),
        Some(r) if r >= 1_000.0 => format!("{:.1}K/s", r / 1_000.0),
        Some(r) => format!("{r:.0}B/s"),
        None => "-".to_string(),
    }
}

pub(super) const fn backlog_size_color(bytes: u64) -> Color {
    if bytes < 5_000_000 {
        Color::Rgb(100, 200, 100)
    } else if bytes < 10_000_000 {
        Color::Rgb(150, 220, 100)
    } else if bytes < 20_000_000 {
        Color::Rgb(200, 220, 80)
    } else if bytes < 30_000_000 {
        Color::Rgb(240, 200, 60)
    } else if bytes < 45_000_000 {
        Color::Rgb(255, 160, 60)
    } else if bytes < 60_000_000 {
        Color::Rgb(255, 100, 80)
    } else {
        Color::Rgb(255, 80, 120)
    }
}

pub(super) const fn block_color(block_number: u64) -> Color {
    BLOCK_COLORS[(block_number as usize) % BLOCK_COLORS.len()]
}

pub(super) fn build_gas_bar(
    gas_used: u64,
    gas_limit: u64,
    elasticity: u64,
    bar_chars: usize,
) -> Line<'static> {
    if gas_limit == 0 {
        return Line::from("-".to_string());
    }

    let bar_units = bar_chars * 8;
    let gas_target = gas_limit / elasticity;
    let target_char = ((gas_target as f64 / gas_limit as f64) * bar_chars as f64).round() as usize;

    let filled_units = ((gas_used as f64 / gas_limit as f64) * bar_units as f64).round() as usize;
    let filled_units = filled_units.min(bar_units);

    let fill_color = Color::Rgb(100, 180, 255);
    let target_color = Color::Rgb(255, 200, 100);

    let mut spans = Vec::new();
    let mut current_units = 0;

    for char_idx in 0..bar_chars {
        let char_end_units = (char_idx + 1) * 8;
        let is_target_char = char_idx == target_char;

        if is_target_char {
            if current_units >= filled_units {
                spans.push(Span::styled("│", Style::default().fg(target_color)));
            } else {
                spans.push(Span::styled("│", Style::default().fg(target_color).bg(fill_color)));
            }
        } else if current_units >= filled_units {
            spans.push(Span::styled(" ", Style::default()));
        } else if char_end_units <= filled_units {
            spans.push(Span::styled("█", Style::default().fg(fill_color)));
        } else {
            let units_in_char = filled_units - current_units;
            spans.push(Span::styled(
                EIGHTH_BLOCKS[units_in_char - 1].to_string(),
                Style::default().fg(fill_color),
            ));
        }

        current_units = char_end_units;
    }

    Line::from(spans)
}

// =============================================================================
// Reusable Render Functions
// =============================================================================

pub(super) fn render_blocks_table(
    f: &mut Frame,
    area: Rect,
    contributions: &VecDeque<BlockContribution>,
    safe_block: u64,
    is_active: bool,
    selected_row: usize,
    highlighted_block: Option<u64>,
    title: &str,
) {
    let border_color = if is_active { Color::Rgb(100, 180, 255) } else { Color::DarkGray };

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let header_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let header = Row::new(vec![
        Cell::from("Block").style(header_style),
        Cell::from("DA").style(header_style),
        Cell::from("Age").style(header_style),
    ]);

    let rows: Vec<Row> = contributions
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, contrib)| {
            let is_selected = is_active && idx == selected_row;
            let is_highlighted = highlighted_block == Some(contrib.block_number);
            let is_safe = contrib.block_number <= safe_block;

            let style = if is_selected {
                Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 80))
            } else if is_highlighted {
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
            } else {
                Style::default().fg(Color::White)
            };

            let block_style = if is_safe {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(block_color(contrib.block_number))
            };

            Row::new(vec![
                Cell::from(contrib.block_number.to_string()).style(block_style),
                Cell::from(format_bytes(contrib.da_bytes)),
                Cell::from(format_duration(contrib.timestamp.elapsed())),
            ])
            .style(style)
        })
        .collect();

    let widths = [Constraint::Length(10), Constraint::Length(8), Constraint::Min(6)];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

pub(super) fn render_batches_table(
    f: &mut Frame,
    area: Rect,
    batches: &VecDeque<BatchSubmission>,
    is_active: bool,
    selected_row: usize,
    highlighted_batch_idx: Option<usize>,
    has_op_node: bool,
    title: &str,
) {
    let border_color = if is_active { Color::Rgb(100, 180, 255) } else { Color::DarkGray };

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if !has_op_node {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled("op_node_rpc required", Style::default().fg(Color::Yellow))),
            Line::from(Span::styled("Set in config file:", Style::default().fg(Color::DarkGray))),
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
        Cell::from("Blks").style(header_style),
        Cell::from("Age").style(header_style),
    ]);

    let rows: Vec<Row> = batches
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(idx, batch)| {
            let is_selected = is_active && idx == selected_row;
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
                Cell::from(batch.blocks_submitted.to_string()),
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
        Constraint::Length(5),
        Constraint::Min(5),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

pub(super) fn render_stats_line(
    growth_rate: Option<f64>,
    burn_rate: Option<f64>,
    time_since_blob: Option<Duration>,
) -> Line<'static> {
    let growth = format_rate(growth_rate);
    let burn = format_rate(burn_rate);
    let since = time_since_blob.map(format_duration).unwrap_or_else(|| "-".to_string());

    Line::from(vec![
        Span::styled("Growth: ", Style::default().fg(Color::DarkGray)),
        Span::styled(growth, Style::default().fg(Color::Rgb(255, 180, 100))),
        Span::raw("  "),
        Span::styled("Burn: ", Style::default().fg(Color::DarkGray)),
        Span::styled(burn, Style::default().fg(Color::Rgb(100, 200, 100))),
        Span::raw("  "),
        Span::styled("Last batch: ", Style::default().fg(Color::DarkGray)),
        Span::styled(since, Style::default().fg(Color::White)),
    ])
}
