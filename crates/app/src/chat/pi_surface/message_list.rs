use super::utils::*;
use crate::chat::pi_surface::diff_viewer::render_diff_to_lines;
use crate::chat::pi_surface::markdown;
use crate::conversation::is_compacted_summary_content;
use crate::tui_surface::TuiSectionSpec;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

pub enum MessageContent {
    RenderedLines(Vec<String>),
    Markdown(String),
    Diff {
        title: Option<String>,
        content: String,
    },
    Image {
        alt: String,
        url: String,
    },
    ToolCall {
        title: String,
        lines: Vec<String>,
        status: ToolStatus,
    },
    Compaction {
        turn_count: usize,
        summary: String,
        expanded: bool,
    },
    StartupHeader {
        version: String,
        tutorial: String,
        sections: Vec<(String, Vec<String>)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Success,
    Error,
}

pub struct Message {
    pub role: String,
    pub contents: Vec<MessageContent>,
}

pub struct MessageList {
    pub messages: Vec<Message>,
    pub scroll_offset: u16,
}

impl MessageList {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
        }
    }

    pub fn add_user_message(&mut self, msg: String) {
        self.messages.push(Message {
            role: "You".to_string(),
            contents: vec![MessageContent::Markdown(msg)],
        });
        self.scroll_offset = 0;
    }

    pub fn add_assistant_message(&mut self, msg: String) {
        let contents = build_assistant_contents(&msg);
        self.messages.push(Message {
            role: "Assistant".to_string(),
            contents,
        });
        self.scroll_offset = 0;
    }

    pub fn add_rendered_lines(&mut self, lines: Vec<String>) {
        self.messages.push(Message {
            role: "System".to_string(),
            contents: vec![MessageContent::RenderedLines(lines)],
        });
        self.scroll_offset = 0;
    }

    pub fn add_startup_header(
        &mut self,
        version: String,
        tutorial: String,
        sections: Vec<(String, Vec<String>)>,
    ) {
        self.messages.push(Message {
            role: "System".to_string(),
            contents: vec![MessageContent::StartupHeader {
                version,
                tutorial,
                sections,
            }],
        });
        self.scroll_offset = 0;
    }

    pub fn toggle_latest_compaction(&mut self) -> bool {
        for message in self.messages.iter_mut().rev() {
            for content in message.contents.iter_mut().rev() {
                if let MessageContent::Compaction { expanded, .. } = content {
                    *expanded = !*expanded;
                    return true;
                }
            }
        }
        false
    }

    pub fn get_rendered_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut text_lines = Vec::new();
        text_lines.push(Line::from(""));

        for msg in &self.messages {
            for content in &msg.contents {
                match content {
                    MessageContent::RenderedLines(lines) => {
                        for line in lines {
                            for wrapped in crate::presentation::render_wrapped_display_line(
                                line,
                                width as usize,
                            ) {
                                text_lines.push(Line::from(wrapped));
                            }
                        }
                    }
                    MessageContent::StartupHeader {
                        version,
                        tutorial,
                        sections,
                    } => {
                        text_lines.push(Line::from(vec![
                            Span::styled(
                                "loong ",
                                Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("v{}", version),
                                Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                            ),
                        ]));
                        text_lines.push(Line::from(vec![Span::styled(
                            tutorial.to_string(),
                            Style::default().fg(PI_DIM_GRAY),
                        )]));
                        text_lines.push(Line::from(""));

                        for (title, values) in sections {
                            text_lines.push(Line::from(vec![Span::styled(
                                format!("[{title}]"),
                                Style::default().fg(PI_HEADING),
                            )]));
                            for value in values {
                                text_lines.push(Line::from(vec![
                                    Span::raw("  "),
                                    Span::styled(
                                        value.clone(),
                                        Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                                    ),
                                ]));
                            }
                            text_lines.push(Line::from(""));
                        }
                    }
                    MessageContent::Markdown(md) => {
                        let is_user = msg.role == "You";
                        let md_lines = markdown::render_markdown_to_lines(md);

                        if is_user {
                            let mut padding =
                                Line::from(vec![Span::raw(" ".repeat(width as usize))]);
                            for span in &mut padding.spans {
                                span.style = span.style.bg(PI_USER_MSG_BG);
                            }
                            text_lines.push(padding);

                            for line in md_lines {
                                let mut full_line = line.clone();
                                full_line.spans.insert(0, Span::raw("  "));
                                let mut colored_spans =
                                    vec![Span::styled(" ", Style::default().bg(PI_USER_MSG_BG))];
                                for span in full_line.spans {
                                    let mut s = span.clone();
                                    s.style = s.style.bg(PI_USER_MSG_BG);
                                    colored_spans.push(s);
                                }
                                text_lines.push(Line::from(colored_spans));
                            }
                        } else {
                            let wrapped_lines = wrap_assistant_markdown_lines(md_lines, width);
                            text_lines.extend(wrapped_lines);
                            text_lines.push(Line::from(""));
                            text_lines.push(Line::from(""));
                        }
                    }
                    MessageContent::Diff { title, content } => {
                        text_lines.extend(render_diff_block_lines(
                            title.as_deref(),
                            content.as_str(),
                            width,
                        ));
                    }
                    MessageContent::Image { alt, url } => {
                        text_lines.extend(render_image_block_lines(alt, url, width));
                    }
                    MessageContent::ToolCall {
                        title,
                        lines,
                        status,
                    } => {
                        text_lines.extend(render_tool_block_lines(title, lines, *status, width));
                    }
                    MessageContent::Compaction {
                        turn_count,
                        summary,
                        expanded,
                    } => {
                        text_lines.extend(render_compaction_block_lines(
                            *turn_count,
                            summary.as_str(),
                            *expanded,
                            width,
                        ));
                    }
                }
            }
            text_lines.push(Line::from(""));
        }
        text_lines
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut text_lines = self.get_rendered_lines(area.width);

        for line in &mut text_lines {
            let is_user_bg = line
                .spans
                .iter()
                .any(|s| s.style.bg == Some(PI_USER_MSG_BG));
            let is_comp_bg = line
                .spans
                .iter()
                .any(|s| s.style.bg == Some(PI_COMPACTION_BG));
            let bg = if is_user_bg {
                Some(PI_USER_MSG_BG)
            } else if is_comp_bg {
                Some(PI_COMPACTION_BG)
            } else {
                None
            };
            if let Some(c) = bg {
                pad_and_bg(line, area.width, c);
            }
        }

        let total_lines = text_lines.len();
        let scroll_val = total_lines
            .saturating_sub(area.height as usize)
            .saturating_sub(self.scroll_offset as usize) as u16;

        let paragraph = Paragraph::new(Text::from(text_lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll_val, 0));

        f.render_widget(paragraph, area);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_add(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1)
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        match mouse.kind {
            crossterm::event::MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(3)
            }
            crossterm::event::MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1)
            }
            _ => {}
        }
    }
}

fn pad_and_bg(line: &mut Line, width: u16, bg: Color) {
    let line_len: usize = line.spans.iter().map(|s| s.width()).sum();
    let pad_len = (width as usize).saturating_sub(line_len);
    if pad_len > 0 {
        line.spans.push(Span::raw(" ".repeat(pad_len)));
    }
    for span in &mut line.spans {
        span.style = span.style.bg(bg);
    }
}

fn wrap_assistant_markdown_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2) as usize;
    let mut rendered = Vec::new();

    for line in lines {
        let plain = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        if plain.trim().is_empty() {
            rendered.push(Line::from(""));
            continue;
        }

        let style = assistant_line_style(&plain);
        for wrapped in
            crate::presentation::render_wrapped_display_line(plain.as_str(), content_width)
        {
            rendered.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(wrapped, style),
            ]));
        }
    }

    rendered
}

fn assistant_line_style(line: &str) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        Style::default().fg(PI_HEADING).add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("```") {
        Style::default().fg(PI_DIM_GRAY)
    } else if trimmed.starts_with("┃") || trimmed.starts_with('>') {
        Style::default().fg(PI_GRAY)
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("• ")
    {
        Style::default().fg(ratatui::style::Color::White)
    } else if trimmed.starts_with("[image]") {
        Style::default().fg(PI_ACCENT)
    } else {
        Style::default().fg(ratatui::style::Color::White)
    }
}

fn build_assistant_contents(text: &str) -> Vec<MessageContent> {
    if is_compacted_summary_content(text) {
        return vec![parse_compaction_content(text)];
    }

    let sections = super::super::parse_cli_chat_markdown_sections(text);
    let mut contents = Vec::new();

    for section in sections {
        match section {
            TuiSectionSpec::Preformatted {
                title,
                language: Some(language),
                lines,
            } if matches!(
                language.trim().to_ascii_lowercase().as_str(),
                "diff" | "patch"
            ) =>
            {
                contents.push(MessageContent::Diff {
                    title,
                    content: lines.join("\n"),
                });
            }
            TuiSectionSpec::Callout { title, lines, .. }
                if title
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("tool activity")) =>
            {
                contents.push(MessageContent::ToolCall {
                    title: title.unwrap_or_else(|| "tool activity".to_owned()),
                    status: infer_tool_status(&lines),
                    lines,
                });
            }
            other => {
                let markdown = render_section_markdown(&other);
                append_markdown_or_image_contents(&markdown, &mut contents);
            }
        }
    }

    if contents.is_empty() {
        append_markdown_or_image_contents(text, &mut contents);
    }

    if contents.is_empty() {
        contents.push(MessageContent::Markdown(text.to_owned()));
    }

    contents
}

fn parse_compaction_content(text: &str) -> MessageContent {
    let mut turn_count = 0usize;
    let mut summary_lines = Vec::new();

    for line in text.lines() {
        if let Some(value) = line
            .strip_prefix("Compacted ")
            .and_then(|rest| rest.strip_suffix(" earlier turns"))
            .and_then(|value| value.parse::<usize>().ok())
        {
            turn_count = value;
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            continue;
        }

        if line == "This compacted checkpoint is session-local recall only." {
            continue;
        }

        if !line.trim().is_empty() {
            summary_lines.push(line.to_owned());
        }
    }

    MessageContent::Compaction {
        turn_count,
        summary: summary_lines.join("\n"),
        expanded: false,
    }
}

fn infer_tool_status(lines: &[String]) -> ToolStatus {
    let lower = lines.join("\n").to_ascii_lowercase();
    if lower.contains("[failed]")
        || lower.contains(" interrupted")
        || lower.contains(" error")
        || lower.contains(" exit=") && !lower.contains("exit=0")
    {
        ToolStatus::Error
    } else if lower.contains("[running]") || lower.contains("[pending]") {
        ToolStatus::Pending
    } else {
        ToolStatus::Success
    }
}

fn append_markdown_or_image_contents(markdown: &str, contents: &mut Vec<MessageContent>) {
    let mut markdown_buffer = Vec::new();

    for line in markdown.lines() {
        if let Some((alt, url)) = parse_markdown_image_line(line) {
            if !markdown_buffer.is_empty() {
                let buffered = markdown_buffer.join("\n");
                if !buffered.trim().is_empty() {
                    contents.push(MessageContent::Markdown(buffered));
                }
                markdown_buffer.clear();
            }
            contents.push(MessageContent::Image { alt, url });
            continue;
        }

        markdown_buffer.push(line.to_owned());
    }

    if !markdown_buffer.is_empty() {
        let buffered = markdown_buffer.join("\n");
        if !buffered.trim().is_empty() {
            contents.push(MessageContent::Markdown(buffered));
        }
    }
}

fn parse_markdown_image_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("![")?;
    let (alt, remainder) = rest.split_once("](")?;
    let url = remainder.strip_suffix(')')?;
    Some((alt.trim().to_owned(), url.trim().to_owned()))
}

fn render_section_markdown(section: &TuiSectionSpec) -> String {
    match section {
        TuiSectionSpec::Narrative { title, lines } => {
            let mut parts = Vec::new();
            if let Some(title) = title {
                parts.push(format!("### {title}"));
            }
            parts.extend(lines.iter().cloned());
            parts.join("\n")
        }
        TuiSectionSpec::Callout { title, lines, .. } => {
            let mut rendered = Vec::new();
            if let Some(title) = title {
                rendered.push(format!("### {title}"));
            }
            rendered.extend(lines.iter().map(|line| format!("> {line}")));
            rendered.join("\n")
        }
        TuiSectionSpec::Preformatted {
            title,
            language,
            lines,
        } => {
            let mut parts = Vec::new();
            if let Some(title) = title {
                parts.push(format!("### {title}"));
            }
            let fence = language.as_deref().unwrap_or("");
            parts.push(format!("```{fence}"));
            parts.extend(lines.iter().cloned());
            parts.push("```".to_owned());
            parts.join("\n")
        }
        TuiSectionSpec::KeyValues { title, items } => {
            let mut parts = Vec::new();
            if let Some(title) = title {
                parts.push(format!("### {title}"));
            }
            for item in items {
                match item {
                    crate::tui_surface::TuiKeyValueSpec::Plain { key, value } => {
                        parts.push(format!("- {key}: {value}"));
                    }
                    crate::tui_surface::TuiKeyValueSpec::Csv { key, values } => {
                        parts.push(format!("- {key}: {}", values.join(", ")));
                    }
                }
            }
            parts.join("\n")
        }
        TuiSectionSpec::ActionGroup { title, items, .. } => {
            let mut parts = Vec::new();
            if let Some(title) = title {
                parts.push(format!("### {title}"));
            }
            for item in items {
                parts.push(format!("- {}: `{}`", item.label, item.command));
            }
            parts.join("\n")
        }
        TuiSectionSpec::Checklist { title, items } => {
            let mut parts = Vec::new();
            if let Some(title) = title {
                parts.push(format!("### {title}"));
            }
            for item in items {
                parts.push(format!("- {} — {}", item.label, item.detail));
            }
            parts.join("\n")
        }
    }
}

fn render_tool_block_lines(
    title: &str,
    lines: &[String],
    status: ToolStatus,
    width: u16,
) -> Vec<Line<'static>> {
    let status_text = match status {
        ToolStatus::Pending => ("pending", PI_YELLOW),
        ToolStatus::Success => ("success", PI_GREEN),
        ToolStatus::Error => ("error", PI_RED),
    };
    let mut rendered = Vec::new();
    rendered.push(background_line(width, PI_TOOL_BG));
    rendered.push(styled_background_line(
        vec![
            Span::raw(" "),
            Span::styled(
                format!("[{}]", title),
                Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(status_text.0, Style::default().fg(status_text.1)),
        ],
        width,
        PI_TOOL_BG,
    ));
    for line in lines {
        let (prefix_style, body_style) = if line.starts_with("stdout:") {
            (
                Style::default().fg(PI_GREEN).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            )
        } else if line.starts_with("stderr:") {
            (
                Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            )
        } else if line.starts_with("file:") {
            (
                Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            )
        } else if line.starts_with("metrics:") {
            (
                Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            )
        } else {
            (
                Style::default().fg(PI_GRAY),
                Style::default().fg(PI_DARK_GRAY),
            )
        };
        let (prefix, body) = line
            .split_once(':')
            .map(|(prefix, body)| (format!("{prefix}: "), body.trim_start().to_owned()))
            .unwrap_or_else(|| ("".to_owned(), line.clone()));
        rendered.push(styled_background_line(
            vec![
                Span::raw("  "),
                Span::styled(prefix, prefix_style),
                Span::styled(body, body_style),
            ],
            width,
            PI_TOOL_BG,
        ));
    }
    rendered.push(background_line(width, PI_TOOL_BG));
    rendered
}

fn render_compaction_block_lines(
    turn_count: usize,
    summary: &str,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    rendered.push(background_line(width, PI_COMPACTION_BG));
    rendered.push(styled_background_line(
        vec![
            Span::raw(" "),
            Span::styled(
                "[compaction]",
                Style::default()
                    .fg(LOONG_COMPACTION_TAG)
                    .add_modifier(Modifier::BOLD),
            ),
        ],
        width,
        PI_COMPACTION_BG,
    ));
    if expanded {
        rendered.push(styled_background_line(
            vec![
                Span::raw("  "),
                Span::styled(
                    format!("Compacted from {turn_count} earlier turns"),
                    Style::default().fg(PI_GRAY),
                ),
            ],
            width,
            PI_COMPACTION_BG,
        ));
        for line in summary.lines() {
            rendered.push(styled_background_line(
                vec![
                    Span::raw("  "),
                    Span::styled(line.to_owned(), Style::default().fg(PI_GRAY)),
                ],
                width,
                PI_COMPACTION_BG,
            ));
        }
    } else {
        rendered.push(styled_background_line(
            vec![
                Span::raw("  "),
                Span::styled(
                    format!("Compacted from {turn_count} earlier turns (Ctrl+O to expand)"),
                    Style::default().fg(PI_GRAY),
                ),
            ],
            width,
            PI_COMPACTION_BG,
        ));
    }
    rendered.push(background_line(width, PI_COMPACTION_BG));
    rendered
}

fn render_diff_block_lines(title: Option<&str>, diff: &str, width: u16) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    rendered.push(background_line(width, PI_TOOL_BG));
    rendered.push(styled_background_line(
        vec![
            Span::raw(" "),
            Span::styled(
                format!("[{}]", title.unwrap_or("diff")),
                Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
            ),
        ],
        width,
        PI_TOOL_BG,
    ));
    for line in render_diff_to_lines(diff) {
        let mut line = line;
        pad_and_bg(&mut line, width, PI_TOOL_BG);
        rendered.push(line);
    }
    rendered.push(background_line(width, PI_TOOL_BG));
    rendered
}

fn render_image_block_lines(alt: &str, url: &str, width: u16) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let alt_text = if alt.trim().is_empty() {
        "image".to_owned()
    } else {
        alt.to_owned()
    };
    rendered.push(Line::from(vec![
        Span::styled(
            "[image] ",
            Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(alt_text, Style::default().fg(PI_ACCENT)),
    ]));
    rendered.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(url.to_owned(), Style::default().fg(PI_DIM_GRAY)),
    ]));
    rendered.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "inline preview unavailable in this terminal surface",
            Style::default().fg(PI_GRAY),
        ),
    ]));
    for line in &mut rendered {
        let current_width: usize = line.spans.iter().map(|span| span.width()).sum();
        if current_width < width as usize {
            line.spans
                .push(Span::raw(" ".repeat(width as usize - current_width)));
        }
    }
    rendered
}

fn background_line(width: u16, bg: Color) -> Line<'static> {
    let mut line = Line::from(vec![Span::raw(" ".repeat(width as usize))]);
    for span in &mut line.spans {
        span.style = span.style.bg(bg);
    }
    line
}

fn styled_background_line(spans: Vec<Span<'static>>, width: u16, bg: Color) -> Line<'static> {
    let mut line = Line::from(spans);
    pad_and_bg(&mut line, width, bg);
    line
}

#[cfg(test)]
mod tests {
    use super::{MessageContent, MessageList, ToolStatus, build_assistant_contents};

    #[test]
    fn assistant_reply_promotes_diff_fences_to_diff_content() {
        let contents = build_assistant_contents("### Diff\n```diff\n- old\n+ new\n```");

        assert!(matches!(
            contents.first(),
            Some(MessageContent::Diff { title, content })
                if title.as_deref() == Some("Diff") && content.contains("- old") && content.contains("+ new")
        ));
    }

    #[test]
    fn assistant_reply_promotes_tool_activity_callout_to_tool_block() {
        let contents = build_assistant_contents(
            "### Tool activity\n> [completed] read_file (id=call-1)\n> stdout: ok",
        );

        assert!(matches!(
            contents.first(),
            Some(MessageContent::ToolCall { title, status, lines })
                if title.eq_ignore_ascii_case("tool activity")
                    && *status == ToolStatus::Success
                    && !lines.is_empty()
        ));
    }

    #[test]
    fn compacted_summary_promotes_to_compaction_block() {
        let text = "[session_local_recall_compacted_window]\nThis compacted checkpoint is session-local recall only.\nCompacted 4 earlier turns\nUser context:\n- earlier ask";
        let contents = build_assistant_contents(text);

        assert!(matches!(
            contents.first(),
            Some(MessageContent::Compaction {
                turn_count,
                summary,
                expanded
            })
                if *turn_count == 4 && summary.contains("User context") && !expanded
        ));
    }

    #[test]
    fn toggle_latest_compaction_flips_expanded_state() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "[session_local_recall_compacted_window]\nThis compacted checkpoint is session-local recall only.\nCompacted 2 earlier turns\nUser context:\n- ask"
                .to_owned(),
        );

        assert!(list.toggle_latest_compaction());

        let Some(message) = list.messages.last() else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            message.contents.first(),
            Some(MessageContent::Compaction { expanded, .. }) if *expanded
        ));
    }

    #[test]
    fn assistant_reply_promotes_markdown_images_to_image_block() {
        let contents = build_assistant_contents(
            "Here is the diagram\n\n![plan](https://example.com/plan.png)",
        );

        assert!(matches!(
            contents.get(1),
            Some(MessageContent::Image { alt, url })
                if alt == "plan" && url == "https://example.com/plan.png"
        ));
    }
}
