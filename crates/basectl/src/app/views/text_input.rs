use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

use crate::commands::common::COLOR_ACTIVE_BORDER;

#[derive(Debug)]
pub(super) struct TextInput {
    pub(super) value: String,
    cursor: usize,
    pub(super) error: Option<String>,
}

impl TextInput {
    pub(super) const fn new() -> Self {
        Self { value: String::new(), cursor: 0, error: None }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.error = None;
        match key.code {
            KeyCode::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                false
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.value[..self.cursor].chars().last().map_or(0, |c| c.len_utf8());
                    self.cursor -= prev;
                    self.value.remove(self.cursor);
                }
                false
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
                false
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    let prev = self.value[..self.cursor].chars().last().map_or(0, |c| c.len_utf8());
                    self.cursor -= prev;
                }
                false
            }
            KeyCode::Right => {
                if self.cursor < self.value.len() {
                    let next = self.value[self.cursor..].chars().next().map_or(0, |c| c.len_utf8());
                    self.cursor += next;
                }
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = self.value.len();
                false
            }
            KeyCode::Enter => true,
            _ => {
                // ctrl+u: clear line
                if key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.value.clear();
                    self.cursor = 0;
                }
                false
            }
        }
    }

    pub(super) fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Config path ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_ACTIVE_BORDER));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(ref err) = self.error {
            // Show input on first line, error on second
            let lines = vec![
                Line::from(Span::styled(&self.value, Style::default().fg(Color::White))),
                Line::from(Span::styled(err.as_str(), Style::default().fg(Color::Red))),
            ];
            let para = Paragraph::new(lines);
            frame.render_widget(para, inner);
        } else {
            let display = if self.value.is_empty() {
                Span::styled(
                    "Enter path to gobrr config (.yaml / .yml)",
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                Span::styled(&self.value, Style::default().fg(Color::White))
            };
            let para = Paragraph::new(Line::from(display));
            frame.render_widget(para, inner);
        }

        // Show cursor
        let x = inner.x + self.cursor as u16;
        let y = inner.y;
        if x < inner.right() {
            frame.set_cursor_position((x, y));
        }
    }
}
