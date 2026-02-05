use ratatui::{
    layout::Rect,
    prelude::*,
    widgets::Paragraph,
};

/// Status bar component that displays app info at the bottom of the screen.
#[derive(Debug)]
pub struct StatusBar;

impl StatusBar {
    /// Height of the status bar in lines.
    pub fn height() -> u16 {
        1
    }

    /// Renders the status bar.
    ///
    /// Left side: `basectl [config_name]`
    /// Right side: `? (help)`
    pub fn render(f: &mut Frame, area: Rect, config_name: &str) {
        let left = format!("basectl [{}]", config_name);
        let right = "? (help)";

        // Calculate padding to right-align the help text
        let total_width = area.width as usize;
        let left_len = left.len();
        let right_len = right.len();

        let padding = if total_width > left_len + right_len {
            total_width - left_len - right_len
        } else {
            1
        };

        let line = Line::from(vec![
            Span::styled(left, Style::default().fg(Color::DarkGray)),
            Span::raw(" ".repeat(padding)),
            Span::styled(right, Style::default().fg(Color::DarkGray)),
        ]);

        let paragraph = Paragraph::new(line);
        f.render_widget(paragraph, area);
    }
}
