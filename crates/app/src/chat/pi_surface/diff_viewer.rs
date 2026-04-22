use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use similar::{ChangeTag, TextDiff};

pub fn render_diff_to_lines(diff: &str) -> Vec<Line<'static>> {
    let raw_lines = diff.lines().collect::<Vec<_>>();
    let mut rendered = Vec::new();
    let mut index = 0usize;

    while index < raw_lines.len() {
        let Some(current) = raw_lines.get(index).copied() else {
            break;
        };
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
