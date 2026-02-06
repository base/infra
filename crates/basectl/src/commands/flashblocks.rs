use std::collections::VecDeque;
use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use arboard::Clipboard;
use base_flashtypes::Flashblock;
use chrono::Local;
use clap::{Args, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Row, Table, TableState},
};

use super::common::{build_gas_bar, FlashblockEntry};
use crate::config::ChainConfig;
use crate::rpc::{fetch_chain_params, ChainParams};
use crate::tui::{restore_terminal, setup_terminal, AppFrame, Keybinding};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;

const MAX_FLASHBLOCKS: usize = 10_000;
const GAS_BAR_CHARS: usize = 40;

const KEYBINDINGS: &[Keybinding] = &[
    Keybinding::new("q", "Quit"),
    Keybinding::new("?", "Toggle help"),
    Keybinding::new("Space", "Pause/Resume"),
    Keybinding::new("Up/k", "Scroll up"),
    Keybinding::new("Down/j", "Scroll down"),
    Keybinding::new("PgUp", "Page up"),
    Keybinding::new("PgDn", "Page down"),
    Keybinding::new("Home/g", "Top (auto-scroll)"),
    Keybinding::new("End/G", "Bottom"),
];

#[derive(Debug, Subcommand)]
pub enum FlashblocksCommand {
    /// Subscribe to flashblocks stream
    #[command(visible_alias = "s")]
    Subscribe(SubscribeArgs),
}

#[derive(Debug, Args)]
pub struct SubscribeArgs {
    /// WebSocket endpoint (overrides chain config)
    #[arg(short = 'w', long = "websocket")]
    websocket: Option<String>,

    /// Output JSON lines instead of TUI
    #[arg(long)]
    json: bool,
}

pub async fn run_flashblocks(command: FlashblocksCommand, config: &ChainConfig) -> Result<()> {
    match command {
        FlashblocksCommand::Subscribe(args) => run_subscribe(args, config).await,
    }
}

/// Run the default flashblocks subscribe with TUI mode (called from homescreen)
pub async fn default_subscribe(config: &ChainConfig) -> Result<()> {
    let params = fetch_chain_params(config).await?;
    run_tui_mode(config.flashblocks_ws.as_str(), &config.name, params).await
}



struct AppState {
    chain_name: String,
    elasticity_multiplier: u64,
    flashblocks: VecDeque<FlashblockEntry>,
    message_count: u64,
    current_gas_limit: u64,
    current_base_fee: Option<u128>,
    show_help: bool,
    table_state: TableState,
    auto_scroll: bool,
    paused: bool,
}

impl AppState {
    fn new(chain_name: String, params: ChainParams) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self {
            chain_name,
            elasticity_multiplier: params.elasticity,
            flashblocks: VecDeque::with_capacity(MAX_FLASHBLOCKS),
            message_count: 0,
            current_gas_limit: params.gas_limit,
            current_base_fee: None,
            show_help: false,
            table_state,
            auto_scroll: true,
            paused: false,
        }
    }

    fn add_flashblock(&mut self, fb: Flashblock) {
        let block_number = fb.metadata.block_number;
        let now = Local::now();

        let base_fee = fb.base.as_ref().map(|base| {
            base.base_fee_per_gas.try_into().unwrap_or(u128::MAX)
        });

        let prev_base_fee = self.current_base_fee;

        if let Some(ref base) = fb.base {
            self.current_gas_limit = base.gas_limit;
            self.current_base_fee = base_fee;
        }

        let time_diff_ms = self.flashblocks.front().map(|prev| {
            (now - prev.timestamp).num_milliseconds()
        });

        let entry = FlashblockEntry {
            block_number,
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
        self.message_count += 1;
        self.maintain_scroll_on_new_data();
    }

    fn scroll_up(&mut self) {
        if let Some(selected) = self.table_state.selected() {
            if selected > 0 {
                self.table_state.select(Some(selected - 1));
                self.auto_scroll = false;
            } else {
                // At top, enable auto-scroll
                self.auto_scroll = true;
            }
        }
    }

    fn scroll_down(&mut self) {
        if let Some(selected) = self.table_state.selected() {
            let max = self.flashblocks.len().saturating_sub(1);
            if selected < max {
                self.table_state.select(Some(selected + 1));
                self.auto_scroll = false;
            }
        }
    }

    fn page_up(&mut self, page_size: usize) {
        if let Some(selected) = self.table_state.selected() {
            let new_selected = selected.saturating_sub(page_size);
            self.table_state.select(Some(new_selected));
            self.auto_scroll = new_selected == 0;
        }
    }

    fn page_down(&mut self, page_size: usize) {
        if let Some(selected) = self.table_state.selected() {
            let max = self.flashblocks.len().saturating_sub(1);
            let new_selected = (selected + page_size).min(max);
            self.table_state.select(Some(new_selected));
            self.auto_scroll = false;
        }
    }

    fn scroll_to_top(&mut self) {
        self.table_state.select(Some(0));
        self.auto_scroll = true;
    }

    fn scroll_to_bottom(&mut self) {
        let max = self.flashblocks.len().saturating_sub(1);
        self.table_state.select(Some(max));
        self.auto_scroll = false;
    }

    fn maintain_scroll_on_new_data(&mut self) {
        if self.auto_scroll {
            self.table_state.select(Some(0));
        } else if let Some(selected) = self.table_state.selected() {
            let max = self.flashblocks.len().saturating_sub(1);
            self.table_state.select(Some((selected + 1).min(max)));
        }
    }

    fn selected_block_number(&self) -> Option<u64> {
        self.table_state
            .selected()
            .and_then(|idx| self.flashblocks.get(idx))
            .map(|fb| fb.block_number)
    }
}

async fn run_subscribe(args: SubscribeArgs, config: &ChainConfig) -> Result<()> {
    let ws_url = args
        .websocket
        .as_deref()
        .unwrap_or(config.flashblocks_ws.as_str());

    if args.json {
        run_json_mode(ws_url).await
    } else {
        let params = fetch_chain_params(config).await?;
        run_tui_mode(ws_url, &config.name, params).await
    }
}

async fn run_json_mode(url: &str) -> Result<()> {
    let (ws_stream, _) = connect_async(url).await?;
    let (_, mut read) = ws_stream.split();

    while let Some(msg) = read.next().await {
        match msg {
            Ok(msg) => {
                if msg.is_binary() || msg.is_text() {
                    let data = msg.into_data();
                    match Flashblock::try_decode_message(data) {
                        Ok(fb) => {
                            println!("{}", serde_json::to_string(&fb)?);
                        }
                        Err(e) => {
                            eprintln!("Failed to decode flashblock: {e}");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("WebSocket error: {e}");
                break;
            }
        }
    }

    Ok(())
}

async fn run_tui_mode(url: &str, chain_name: &str, params: ChainParams) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_tui_loop(&mut terminal, url, chain_name, params).await;
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

async fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    url: &str,
    chain_name: &str,
    params: ChainParams,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Flashblock>(100);

    let mut state = AppState::new(chain_name.to_string(), params);

    let ws_url = url.to_string();
    tokio::spawn(async move {
        if let Err(e) = run_ws_connection(ws_url, tx).await {
            eprintln!("WebSocket error: {e}");
        }
    });

    loop {
        let content_height = terminal.size()?.height.saturating_sub(5) as usize;

        terminal.draw(|f| draw_ui(f, &mut state))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press {
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('?') => state.show_help = !state.show_help,
                        KeyCode::Char(' ') => state.paused = !state.paused,
                        KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => state.scroll_down(),
                        KeyCode::PageUp => state.page_up(content_height),
                        KeyCode::PageDown => state.page_down(content_height),
                        KeyCode::Home | KeyCode::Char('g') => state.scroll_to_top(),
                        KeyCode::End | KeyCode::Char('G') => state.scroll_to_bottom(),
                        KeyCode::Char('y') => {
                            if let Some(block_num) = state.selected_block_number() {
                                if let Ok(mut clipboard) = Clipboard::new() {
                                    let _ = clipboard.set_text(block_num.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }

        while let Ok(fb) = rx.try_recv() {
            if !state.paused {
                state.add_flashblock(fb);
            }
        }
    }

    Ok(())
}

fn draw_ui(f: &mut Frame, state: &mut AppState) {
    let layout = AppFrame::split_layout(f.area(), state.show_help);
    draw_table(f, layout.content, state);

    AppFrame::render(
        f,
        &layout,
        &state.chain_name,
        KEYBINDINGS,
        None,
    );
}

fn draw_table(f: &mut Frame, area: Rect, state: &mut AppState) {
    let header_cells = ["Block", "FB#", "Txns", "Gas Used", "Base Fee", "Delta", "Gas Fill", "Time"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let selected_block = state.selected_block_number();
    let selected_idx = state.table_state.selected();

    let rows: Vec<Row> = state.flashblocks.iter().enumerate().map(|(idx, fb)| {
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

        let base_fee_cell = if fb.index == 0 {
            let base_fee_str = fb.base_fee
                .map(format_gwei)
                .unwrap_or_else(|| "-".to_string());

            let style = match (fb.base_fee, fb.prev_base_fee) {
                (Some(current), Some(prev)) if current > prev => {
                    Style::default().fg(Color::Green)
                }
                (Some(current), Some(prev)) if current < prev => {
                    Style::default().fg(Color::Red)
                }
                _ => Style::default(),
            };
            Cell::from(base_fee_str).style(style)
        } else {
            Cell::from(String::new())
        };

        let gas_bar = build_gas_bar(fb.gas_used, fb.gas_limit, state.elasticity_multiplier, GAS_BAR_CHARS);

        let is_selected = selected_idx == Some(idx);
        let is_same_block = selected_block == Some(fb.block_number) && !is_selected;

        let row_style = if is_selected {
            Style::default().bg(Color::Rgb(50, 50, 70))
        } else if is_same_block {
            Style::default().bg(Color::Rgb(35, 35, 50))
        } else if fb.index == 0 {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        Row::new([
            Cell::from(fb.block_number.to_string()),
            Cell::from(fb.index.to_string()),
            Cell::from(fb.tx_count.to_string()),
            Cell::from(format_gas(fb.gas_used)),
            base_fee_cell,
            delta_cell,
            Cell::from(gas_bar),
            Cell::from(fb.timestamp.format("%H:%M:%S%.3f").to_string()),
        ])
        .style(row_style)
    }).collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(GAS_BAR_CHARS as u16),
        Constraint::Min(14),
    ];

    let title = if state.paused {
        " Recent Flashblocks [PAUSED] ".to_string()
    } else if state.auto_scroll {
        " Recent Flashblocks [AUTO] ".to_string()
    } else {
        let selected = state.table_state.selected().unwrap_or(0) + 1;
        let total = state.flashblocks.len();
        format!(" Recent Flashblocks [{selected}/{total}] ")
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(title)
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(50, 50, 70)));

    f.render_stateful_widget(table, area, &mut state.table_state);
}

fn format_gas(gas: u64) -> String {
    if gas >= 1_000_000 {
        format!("{:.2}M", gas as f64 / 1_000_000.0)
    } else if gas >= 1_000 {
        format!("{:.1}K", gas as f64 / 1_000.0)
    } else {
        gas.to_string()
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
