use super::utils::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render_markdown_to_lines(md: &str) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(md, options);
    let mut lines = Vec::new();
    let mut current_spans = Vec::new();

    let mut in_code_block = false;
    let mut in_quote = false;
    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut image_alt = String::new();
    let mut list_depth: usize = 0;

    let mut current_style = Style::default();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(l) => l.to_string(),
                    _ => "".to_string(),
                };
                in_code_block = true;
                // Pi style: ```lang in dim gray
                lines.push(Line::from(Span::styled(
                    format!("```{}", lang),
                    Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                )));
            }
            Event::End(TagEnd::CodeBlock) => {
                if !current_spans.is_empty() {
                    let content = std::mem::take(&mut current_spans)
                        .into_iter()
                        .map(|s| s.content.into_owned())
                        .collect::<String>();
                    for l in content.lines() {
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(l.to_string(), Style::default().fg(PI_GREEN)),
                        ]));
                    }
                }
                in_code_block = false;
                lines.push(Line::from(Span::styled(
                    "```",
                    Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                )));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                in_image = true;
                image_url = Some(dest_url.to_string());
                image_alt.clear();
            }
            Event::End(TagEnd::Image) => {
                let alt = if image_alt.trim().is_empty() {
                    "image".to_owned()
                } else {
                    image_alt.trim().to_owned()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        "[image] ",
                        Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(alt, Style::default().fg(PI_ACCENT)),
                ]));
                if let Some(url) = image_url.take() {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(url, Style::default().fg(PI_DIM_GRAY)),
                    ]));
                }
                lines.push(Line::from(""));
                in_image = false;
            }
            Event::Start(Tag::BlockQuote(_)) => in_quote = true,
            Event::End(TagEnd::BlockQuote(_)) => {
                if !current_spans.is_empty() {
                    let mut line_spans = vec![Span::styled("┃ ", Style::default().fg(PI_GRAY))];
                    line_spans.extend(std::mem::take(&mut current_spans));
                    lines.push(Line::from(line_spans));
                }
                in_quote = false;
            }
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => list_depth -= 1,
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                current_spans.push(Span::styled(
                    format!("{indent}• "),
                    Style::default().fg(PI_ACCENT),
                ));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let prefix = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    _ => "#### ",
                };
                current_spans.push(Span::styled(
                    prefix.to_string(),
                    Style::default().fg(PI_HEADING).add_modifier(Modifier::BOLD),
                ));
                current_style = current_style.add_modifier(Modifier::BOLD).fg(PI_HEADING);
            }
            Event::End(TagEnd::Heading(_)) => {
                current_style = current_style
                    .remove_modifier(Modifier::BOLD)
                    .fg(ratatui::style::Color::Reset);
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                lines.push(Line::from(""));
            }
            Event::Start(Tag::Strong) => current_style = current_style.add_modifier(Modifier::BOLD),
            Event::End(TagEnd::Strong) => {
                current_style = current_style.remove_modifier(Modifier::BOLD)
            }
            Event::Start(Tag::Emphasis) => {
                current_style = current_style.add_modifier(Modifier::ITALIC)
            }
            Event::End(TagEnd::Emphasis) => {
                current_style = current_style.remove_modifier(Modifier::ITALIC)
            }
            Event::Code(text) => {
                current_spans.push(Span::styled(
                    text.to_string(),
                    Style::default().fg(PI_ACCENT),
                ));
            }
            Event::Text(text) => {
                if in_image {
                    image_alt.push_str(text.as_ref());
                    continue;
                }
                if in_code_block {
                    for (i, line) in text.lines().enumerate() {
                        if i > 0 {
                            lines.push(Line::from(vec![
                                Span::raw("  "),
                                Span::styled(line.to_string(), Style::default().fg(PI_GREEN)),
                            ]));
                        } else {
                            current_spans.push(Span::styled(
                                line.to_string(),
                                Style::default().fg(PI_GREEN),
                            ));
                        }
                    }
                } else {
                    current_spans.push(Span::styled(text.to_string(), current_style));
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_quote {
                    let mut line_spans = vec![Span::styled("┃ ", Style::default().fg(PI_GRAY))];
                    line_spans.extend(std::mem::take(&mut current_spans));
                    lines.push(Line::from(line_spans));
                } else {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if in_quote {
                    let mut line_spans = vec![Span::styled("┃ ", Style::default().fg(PI_GRAY))];
                    line_spans.extend(std::mem::take(&mut current_spans));
                    lines.push(Line::from(line_spans));
                } else {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                lines.push(Line::from(""));
            }
            _ => {}
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::render_markdown_to_lines;

    #[test]
    fn renders_markdown_images_as_placeholder_lines() {
        let lines = render_markdown_to_lines("before\n\n![diagram](https://example.com/a.png)\n");
        let joined = lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("[image] diagram"));
        assert!(joined.contains("https://example.com/a.png"));
    }
}
