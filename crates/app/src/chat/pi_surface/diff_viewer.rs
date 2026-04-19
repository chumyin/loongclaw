use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};
use similar::{ChangeTag, TextDiff};

pub fn render_diff_to_lines(diff: &str) -> Vec<Line<'static>> {
    let raw_lines = diff.lines().collect::<Vec<_>>();
    let mut rendered = Vec::new();
    let mut index = 0usize;

    while index < raw_lines.len() {
        let current = raw_lines[index];
        if let Some(removed) = current.strip_prefix('-')
            && let Some(next) = raw_lines
                .get(index + 1)
                .and_then(|line| line.strip_prefix('+'))
        {
            let (removed_line, added_line) = render_intraline_pair(removed, next);
            rendered.push(prefixed_line("  ", "- ", removed_line));
            rendered.push(prefixed_line("  ", "+ ", added_line));
            index += 2;
            continue;
        }

        rendered.push(render_plain_diff_line(current));
        index += 1;
    }
    if rendered.is_empty() {
        rendered.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("(empty diff)", Style::default().fg(Color::DarkGray)),
        ]));
    }
    rendered
}

fn render_plain_diff_line(raw_line: &str) -> Line<'static> {
    let (style, prefix, text) = if let Some(rest) = raw_line.strip_prefix('+') {
        (
            Style::default().fg(Color::Rgb(100, 255, 100)),
            "+ ",
            rest.to_owned(),
        )
    } else if let Some(rest) = raw_line.strip_prefix('-') {
        (
            Style::default().fg(Color::Rgb(255, 100, 100)),
            "- ",
            rest.to_owned(),
        )
    } else {
        (
            Style::default().fg(Color::DarkGray),
            "  ",
            raw_line.to_owned(),
        )
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(prefix, style),
        Span::styled(text, style),
    ])
}

fn render_intraline_pair(removed: &str, added: &str) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let base_removed = Style::default().fg(Color::Rgb(255, 100, 100));
    let base_added = Style::default().fg(Color::Rgb(100, 255, 100));
    let highlight_removed = base_removed.add_modifier(Modifier::REVERSED);
    let highlight_added = base_added.add_modifier(Modifier::REVERSED);
    let diff = TextDiff::from_words(removed, added);
    let mut removed_spans = Vec::new();
    let mut added_spans = Vec::new();

    for change in diff.iter_all_changes() {
        let text = change.to_string();
        match change.tag() {
            ChangeTag::Delete => removed_spans.push(Span::styled(text, highlight_removed)),
            ChangeTag::Insert => added_spans.push(Span::styled(text, highlight_added)),
            ChangeTag::Equal => {
                removed_spans.push(Span::styled(text.clone(), base_removed));
                added_spans.push(Span::styled(text, base_added));
            }
        }
    }

    (removed_spans, added_spans)
}

fn prefixed_line(indent: &str, prefix: &str, spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line_spans = vec![Span::raw(indent.to_owned())];
    if let Some(first_style) = spans.first().map(|span| span.style) {
        line_spans.push(Span::styled(prefix.to_owned(), first_style));
    } else {
        line_spans.push(Span::raw(prefix.to_owned()));
    }
    line_spans.extend(spans);
    Line::from(line_spans)
}

pub struct DiffViewer {
    lines: Vec<Line<'static>>,
    state: ListState,
}

impl DiffViewer {
    pub fn new(original: &str, modified: &str) -> Self {
        let diff = TextDiff::from_lines(original, modified)
            .iter_all_changes()
            .map(|change| {
                let prefix = match change.tag() {
                    ChangeTag::Delete => "-",
                    ChangeTag::Insert => "+",
                    ChangeTag::Equal => " ",
                };
                format!("{prefix}{}", change.value())
            })
            .collect::<Vec<_>>()
            .join("");
        let lines = render_diff_to_lines(&diff);

        Self {
            lines,
            state: ListState::default(),
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .lines
            .iter()
            .map(|l| ListItem::new(l.clone()))
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE)) // Remove borders and title
            .highlight_style(Style::default().bg(Color::Rgb(50, 50, 80)));

        let scrollbar = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);

        let mut scrollbar_state =
            ScrollbarState::new(self.lines.len()).position(self.state.offset());

        f.render_stateful_widget(list, area, &mut self.state);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}
