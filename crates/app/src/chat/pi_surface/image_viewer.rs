use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub struct ImageViewer {
    pub path: String,
}

impl ImageViewer {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(100, 100, 255)))
            .title(Span::styled(
                format!(" Image: {} ", self.path),
                Style::default().add_modifier(Modifier::BOLD),
            ));

        let inner_area = block.inner(area);
        f.render_widget(block, area);

        let placeholder = Paragraph::new(vec![
            Line::from(vec![Span::styled(
                "  [ Image Preview ]  ",
                Style::default().fg(Color::Yellow),
            )]),
            Line::from(vec![Span::raw("  ")]),
            Line::from(vec![Span::raw(format!("  Path: {}", self.path))]),
            Line::from(vec![Span::raw("  Protocol: Braille / Halfblocks")]),
        ])
        .style(Style::default().fg(Color::DarkGray));

        f.render_widget(placeholder, inner_area);
    }
}
