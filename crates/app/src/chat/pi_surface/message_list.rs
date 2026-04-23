use super::utils::*;
use crate::chat::pi_surface::diff_viewer::render_diff_to_lines;
use crate::chat::pi_surface::markdown;
use crate::conversation::is_compacted_summary_content;
use crate::tui_surface::TuiSectionSpec;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use serde_json::Value;
use std::sync::LazyLock;

const PROVIDER_ERROR_REPLY_PREFIX: &str = "[provider_error] ";
static EMPTY_RENDER_LINES: LazyLock<Vec<Line<'static>>> = LazyLock::new(Vec::new);

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
    Error {
        title: String,
        summary: String,
        details: Vec<String>,
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
    page_step: u16,
    mouse_step: u16,
    last_render_height: u16,
    last_scroll_start: usize,
    follow_tail: bool,
    snap_scroll_on_next_render: bool,
    render_revision: u64,
    render_cache: Option<RenderCache>,
}

struct RenderCache {
    width: u16,
    revision: u64,
    lines: Vec<Line<'static>>,
}

impl MessageList {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            page_step: 12,
            mouse_step: 3,
            last_render_height: 0,
            last_scroll_start: 0,
            follow_tail: true,
            snap_scroll_on_next_render: true,
            render_revision: 0,
            render_cache: None,
        }
    }

    pub fn add_user_message(&mut self, msg: String) {
        self.messages.push(Message {
            role: "You".to_string(),
            contents: vec![MessageContent::Markdown(msg)],
        });
        self.scroll_offset = 0;
        self.invalidate_render_cache();
    }

    pub fn add_assistant_message(&mut self, msg: String) {
        let contents = build_assistant_contents(&msg);
        self.messages.push(Message {
            role: "Assistant".to_string(),
            contents,
        });
        self.scroll_offset = 0;
        self.invalidate_render_cache();
    }

    pub fn add_rendered_lines(&mut self, lines: Vec<String>) {
        self.messages.push(Message {
            role: "System".to_string(),
            contents: vec![MessageContent::RenderedLines(lines)],
        });
        self.scroll_offset = 0;
        self.invalidate_render_cache();
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
        self.invalidate_render_cache();
    }

    pub fn toggle_latest_compaction(&mut self) -> bool {
        for message in self.messages.iter_mut().rev() {
            for content in message.contents.iter_mut().rev() {
                if let MessageContent::Compaction { expanded, .. } = content {
                    *expanded = !*expanded;
                    self.invalidate_render_cache();
                    return true;
                }
            }
        }
        false
    }

    pub fn get_rendered_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        self.ensure_render_cache(width).clone()
    }

    pub fn rendered_line_count(&mut self, width: u16) -> usize {
        self.ensure_render_cache(width).len()
    }

    fn ensure_render_cache(&mut self, width: u16) -> &Vec<Line<'static>> {
        let needs_rebuild = self
            .render_cache
            .as_ref()
            .is_none_or(|cache| cache.width != width || cache.revision != self.render_revision);
        if needs_rebuild {
            let lines = self.compute_rendered_lines(width);
            self.render_cache = Some(RenderCache {
                width,
                revision: self.render_revision,
                lines,
            });
        }
        self.render_cache
            .as_ref()
            .map(|cache| &cache.lines)
            .unwrap_or(&EMPTY_RENDER_LINES)
    }

    fn compute_rendered_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut text_lines = Vec::new();
        let mut previous_colored_block = false;

        for msg in &self.messages {
            for content in &msg.contents {
                let current_colored_block =
                    content_renders_colored_block(msg.role.as_str(), content);
                if previous_colored_block
                    && current_colored_block
                    && text_lines.last().is_none_or(|line| {
                        !(is_visual_blank_line(line) && dominant_block_bg(line).is_none())
                    })
                {
                    text_lines.push(Line::from(""));
                }
                match content {
                    MessageContent::RenderedLines(lines) => {
                        for line in lines {
                            if let Some(normalized) = normalize_rendered_system_line(line) {
                                if normalized.trim().is_empty() {
                                    text_lines.push(Line::from(""));
                                    continue;
                                }
                                text_lines.extend(render_rendered_system_line(&normalized, width));
                            }
                        }
                    }
                    MessageContent::StartupHeader {
                        version,
                        tutorial,
                        sections,
                    } => {
                        text_lines.extend(render_startup_version_lines(version, width));
                        for wrapped in crate::presentation::render_wrapped_display_line(
                            tutorial,
                            width as usize,
                        ) {
                            text_lines.push(Line::from(vec![Span::styled(
                                wrapped,
                                Style::default().fg(PI_DIM_GRAY),
                            )]));
                        }
                        text_lines.push(Line::from(""));

                        for (title, values) in sections {
                            text_lines.push(Line::from(vec![Span::styled(
                                format!("[{title}]"),
                                Style::default().fg(PI_HEADING),
                            )]));
                            for value in values {
                                for wrapped in crate::presentation::render_wrapped_display_line(
                                    format!("  {value}").as_str(),
                                    width as usize,
                                ) {
                                    text_lines.push(Line::from(vec![Span::styled(
                                        wrapped,
                                        Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                                    )]));
                                }
                            }
                            text_lines.push(Line::from(""));
                        }
                    }
                    MessageContent::Markdown(md) => {
                        let is_user = msg.role == "You";
                        let markdown_width = width.saturating_sub(2) as usize;
                        let md_lines =
                            markdown::render_markdown_to_lines_with_width(md, Some(markdown_width));

                        if is_user {
                            let mut padding =
                                Line::from(vec![Span::raw(" ".repeat(width as usize))]);
                            for span in &mut padding.spans {
                                span.style = span.style.bg(PI_USER_MSG_BG);
                            }
                            text_lines.push(padding);

                            for line in render_user_markdown_lines(md_lines, width) {
                                text_lines.push(user_block_line(line));
                            }
                            let mut padding =
                                Line::from(vec![Span::raw(" ".repeat(width as usize))]);
                            for span in &mut padding.spans {
                                span.style = span.style.bg(PI_USER_MSG_BG);
                            }
                            text_lines.push(padding);
                        } else {
                            let wrapped_lines = wrap_assistant_markdown_lines(md_lines, width);
                            text_lines.push(Line::from(""));
                            text_lines.extend(wrapped_lines);
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
                    MessageContent::Error {
                        title,
                        summary,
                        details,
                    } => {
                        text_lines.extend(render_error_block_lines(title, summary, details, width));
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
                previous_colored_block = current_colored_block;
            }
        }

        for line in &mut text_lines {
            let is_user_bg = line
                .spans
                .iter()
                .any(|span| span.style.bg == Some(PI_USER_MSG_BG));
            let is_compaction_bg = line
                .spans
                .iter()
                .any(|span| span.style.bg == Some(PI_COMPACTION_BG));
            let background = if is_user_bg {
                Some(PI_USER_MSG_BG)
            } else if is_compaction_bg {
                Some(PI_COMPACTION_BG)
            } else {
                None
            };
            if let Some(background) = background {
                pad_and_bg(line, width, background);
            } else {
                pad_plain(line, width);
            }
        }

        text_lines
    }

    fn invalidate_render_cache(&mut self) {
        self.render_revision = self.render_revision.saturating_add(1);
        self.render_cache = None;
        if self.follow_tail {
            self.snap_scroll_on_next_render = true;
        }
    }

    pub fn trailing_colored_block(&mut self, width: u16) -> bool {
        self.ensure_render_cache(width)
            .iter()
            .rev()
            .find(|line| !is_visual_blank_line(line) || dominant_block_bg(line).is_some())
            .and_then(dominant_block_bg)
            .is_some()
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        self.page_step = page_step_for_height(area.height);
        self.mouse_step = mouse_step_for_height(area.height);
        let text_lines = self.get_rendered_lines(area.width);

        let total_lines = text_lines.len();
        if total_lines == 0 {
            self.last_render_height = area.height;
            self.last_scroll_start = 0;
            self.scroll_offset = 0;
            self.follow_tail = true;
            self.snap_scroll_on_next_render = false;
            f.render_widget(Paragraph::new(Text::from(text_lines)), area);
            return;
        }
        let max_scroll_start = total_lines.saturating_sub(area.height as usize);
        let raw_scroll_val = max_scroll_start.saturating_sub(self.scroll_offset as usize);
        let mut scroll_start = if self.follow_tail {
            raw_scroll_val
        } else if !self.snap_scroll_on_next_render {
            self.last_scroll_start.min(max_scroll_start)
        } else {
            adjust_scroll_start_for_message_boundary(&text_lines, raw_scroll_val)
        };
        scroll_start = scroll_start.min(max_scroll_start);
        self.last_scroll_start = scroll_start;
        self.last_render_height = area.height;
        self.follow_tail = scroll_start == max_scroll_start;
        self.scroll_offset = max_scroll_start.saturating_sub(scroll_start) as u16;
        self.snap_scroll_on_next_render = false;

        let paragraph = Paragraph::new(Text::from(text_lines)).scroll((scroll_start as u16, 0));

        f.render_widget(paragraph, area);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.snap_scroll_on_next_render = true;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_add(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1)
            }
            KeyCode::PageUp | KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll_offset = self.scroll_offset.saturating_add(self.page_step)
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.page_step)
            }
            KeyCode::Home => self.scroll_offset = u16::MAX,
            KeyCode::End => self.scroll_offset = 0,
            KeyCode::Backspace
            | KeyCode::Enter
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::PageUp
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Delete
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Char(_)
            | KeyCode::Null
            | KeyCode::Esc
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => {}
        }
        self.follow_tail = self.scroll_offset == 0;
    }

    pub fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        self.snap_scroll_on_next_render = true;
        match mouse.kind {
            crossterm::event::MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(self.mouse_step)
            }
            crossterm::event::MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.mouse_step)
            }
            crossterm::event::MouseEventKind::Down(_)
            | crossterm::event::MouseEventKind::Up(_)
            | crossterm::event::MouseEventKind::Drag(_)
            | crossterm::event::MouseEventKind::Moved
            | crossterm::event::MouseEventKind::ScrollLeft
            | crossterm::event::MouseEventKind::ScrollRight => {}
        }
        self.follow_tail = self.scroll_offset == 0;
    }

    pub fn is_following_tail(&self) -> bool {
        self.follow_tail
    }
}

fn page_step_for_height(height: u16) -> u16 {
    height.saturating_sub(2).max(1)
}

fn mouse_step_for_height(height: u16) -> u16 {
    page_step_for_height(height).saturating_add(3) / 4
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

fn pad_plain(line: &mut Line, width: u16) {
    let line_len: usize = line.spans.iter().map(|s| s.width()).sum();
    let pad_len = (width as usize).saturating_sub(line_len);
    if pad_len > 0 {
        line.spans.push(Span::raw(" ".repeat(pad_len)));
    }
}

fn adjust_scroll_start_for_message_boundary(lines: &[Line<'static>], start: usize) -> usize {
    let adjusted_for_block = adjust_scroll_start_for_block_boundary(lines, start);
    if adjusted_for_block == start && lines.get(start).and_then(dominant_block_bg).is_none() {
        return start;
    }
    let start = adjusted_for_block;
    if start == 0 || start >= lines.len() || lines.get(start).is_some_and(is_visual_blank_line) {
        return start;
    }

    let lookback = start.saturating_sub(4);
    for index in (lookback..start).rev() {
        if lines.get(index).is_some_and(is_visual_blank_line) {
            return index + 1;
        }
    }
    start
}

fn adjust_scroll_start_for_block_boundary(lines: &[Line<'static>], start: usize) -> usize {
    if start == 0 || start >= lines.len() {
        return start;
    }
    if lines.get(start).is_some_and(|line| line.spans.is_empty()) {
        return start;
    }
    let Some(bg) = lines.get(start).and_then(dominant_block_bg) else {
        return start;
    };
    let mut adjusted = start;
    while adjusted > 0 && lines.get(adjusted - 1).and_then(dominant_block_bg) == Some(bg) {
        adjusted -= 1;
    }
    if lines.get(adjusted).is_some_and(is_visual_blank_line) {
        let mut candidate = adjusted + 1;
        while candidate < lines.len()
            && lines.get(candidate).and_then(dominant_block_bg) == Some(bg)
        {
            if lines
                .get(candidate)
                .is_some_and(|line| !is_visual_blank_line(line))
            {
                return candidate;
            }
            candidate += 1;
        }
    }
    adjusted
}

fn is_visual_blank_line(line: &Line<'static>) -> bool {
    line.spans
        .iter()
        .all(|span| span.content.as_ref().trim().is_empty())
}

fn dominant_block_bg(line: &Line<'static>) -> Option<Color> {
    if line
        .spans
        .iter()
        .any(|span| span.style.bg == Some(PI_USER_MSG_BG))
    {
        return Some(PI_USER_MSG_BG);
    }
    if line
        .spans
        .iter()
        .any(|span| span.style.bg == Some(PI_TOOL_BG))
    {
        return Some(PI_TOOL_BG);
    }
    if line
        .spans
        .iter()
        .any(|span| span.style.bg == Some(PI_COMPACTION_BG))
    {
        return Some(PI_COMPACTION_BG);
    }
    None
}

fn normalize_rendered_system_line(line: &str) -> Option<String> {
    let trimmed = line.trim_end();

    if trimmed.starts_with("╭─ ") {
        return Some(trimmed.trim_start_matches("╭─ ").to_owned());
    }
    if trimmed == "╰─" {
        return None;
    }
    if trimmed == "│" {
        return Some(String::new());
    }
    if let Some(rest) = trimmed.strip_prefix("│ ") {
        return Some(rest.to_owned());
    }
    if let Some(rest) = trimmed.strip_prefix("│") {
        return Some(rest.trim_start().to_owned());
    }

    Some(trimmed.to_owned())
}

fn render_rendered_system_line(line: &str, width: u16) -> Vec<Line<'static>> {
    let content_width = width as usize;

    if let Some(rendered) = render_system_activity_headline(line, content_width) {
        return rendered;
    }

    if let Some(rendered) = render_system_activity_child(line, content_width) {
        return rendered;
    }

    let style = if line.trim_start().starts_with("… +") {
        Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(PI_DARK_GRAY)
    };

    crate::presentation::render_wrapped_display_line(line, content_width)
        .into_iter()
        .map(|wrapped| Line::from(vec![Span::styled(wrapped, style)]))
        .collect()
}

fn render_system_activity_headline(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("• ")?;

    let (label, body, label_style) = if let Some(body) = rest.strip_prefix("Ran ") {
        (
            "Ran",
            body,
            Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = rest.strip_prefix("Explored ") {
        (
            "Explored",
            body,
            Style::default()
                .fg(ratatui::style::Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = rest.strip_prefix("Called ") {
        (
            "Called",
            body,
            Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = rest.strip_prefix("Closed ") {
        (
            "Closed",
            body,
            Style::default().fg(PI_GRAY).add_modifier(Modifier::BOLD),
        )
    } else {
        return None;
    };

    let body_width = content_width
        .saturating_sub(2 + crate::presentation::display_width(label) + 1)
        .max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(body, body_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::styled("• ", Style::default().fg(PI_GREEN)),
                        Span::styled(format!("{label} "), label_style),
                        Span::styled(
                            wrapped_line,
                            Style::default().fg(ratatui::style::Color::White),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::raw(" ".repeat(crate::presentation::display_width(label) + 1)),
                        Span::styled(
                            wrapped_line,
                            Style::default().fg(ratatui::style::Color::White),
                        ),
                    ])
                }
            })
            .collect(),
    )
}

fn render_system_activity_child(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("└ ")?;

    let (label, body) = if let Some(body) = rest.strip_prefix("Read ") {
        ("Read", body)
    } else if let Some(body) = rest.strip_prefix("List ") {
        ("List", body)
    } else if let Some(body) = rest.strip_prefix("Search ") {
        ("Search", body)
    } else if let Some(body) = rest.strip_prefix("Inspect ") {
        ("Inspect", body)
    } else {
        return None;
    };

    let body_width = content_width
        .saturating_sub(2 + 2 + crate::presentation::display_width(label) + 1)
        .max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(body, body_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            "└ ",
                            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                        ),
                        Span::styled(format!("{label} "), Style::default().fg(PI_ACCENT)),
                        Span::styled(
                            wrapped_line,
                            Style::default().fg(ratatui::style::Color::White),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("    "),
                        Span::raw(" ".repeat(crate::presentation::display_width(label) + 1)),
                        Span::styled(
                            wrapped_line,
                            Style::default().fg(ratatui::style::Color::White),
                        ),
                    ])
                }
            })
            .collect(),
    )
}

fn content_renders_colored_block(role: &str, content: &MessageContent) -> bool {
    match content {
        MessageContent::Markdown(_) => role == "You",
        MessageContent::Diff { .. }
        | MessageContent::ToolCall { .. }
        | MessageContent::Compaction { .. } => true,
        MessageContent::Error { .. }
        | MessageContent::RenderedLines(_)
        | MessageContent::Image { .. }
        | MessageContent::StartupHeader { .. } => false,
    }
}

fn render_startup_version_lines(version: &str, width: u16) -> Vec<Line<'static>> {
    crate::presentation::render_wrapped_text_line("", &format!("loong {version}"), width as usize)
        .into_iter()
        .map(|wrapped| {
            if let Some(rest) = wrapped.strip_prefix("loong ") {
                Line::from(vec![
                    Span::styled(
                        "loong",
                        Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        rest.to_owned(),
                        Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                    ),
                ])
            } else {
                Line::from(vec![Span::styled(
                    wrapped,
                    Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
                )])
            }
        })
        .collect()
}

fn wrap_assistant_markdown_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let lines = normalize_blank_lines(lines);
    let content_width = width.saturating_sub(2) as usize;
    let mut rendered = Vec::new();
    let mut paragraph_buffer = String::new();
    let mut in_code_block = false;

    let flush_paragraph =
        |rendered: &mut Vec<Line<'static>>, paragraph_buffer: &mut String, content_width: usize| {
            if paragraph_buffer.trim().is_empty() {
                paragraph_buffer.clear();
                return;
            }
            rendered.extend(render_assistant_plain_line(
                paragraph_buffer.as_str(),
                content_width,
                assistant_line_style(paragraph_buffer),
            ));
            paragraph_buffer.clear();
        };

    for line in lines {
        let plain = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let trimmed = plain.trim_start();
        if plain.trim().is_empty() {
            flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);
            rendered.push(Line::from(""));
            continue;
        }

        if trimmed.starts_with("```") {
            flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);
            rendered.extend(render_assistant_plain_line(
                plain.as_str(),
                content_width,
                assistant_line_style(&plain),
            ));
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);
            rendered.extend(render_assistant_code_line(plain.as_str(), content_width));
            continue;
        }

        if let Some(split_bullets) = split_inline_bullet_runs(&plain) {
            flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);
            for bullet_line in split_bullets {
                rendered.extend(render_assistant_plain_line(
                    bullet_line.as_str(),
                    content_width,
                    assistant_line_style(&bullet_line),
                ));
            }
            continue;
        }

        if is_reflowable_assistant_line(&plain) {
            if !paragraph_buffer.is_empty() {
                paragraph_buffer.push_str(paragraph_joiner(&paragraph_buffer, &plain));
            }
            paragraph_buffer.push_str(plain.trim());
            continue;
        }

        flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);
        rendered.extend(render_assistant_plain_line(
            plain.as_str(),
            content_width,
            assistant_line_style(&plain),
        ));
    }

    flush_paragraph(&mut rendered, &mut paragraph_buffer, content_width);

    rendered
}

fn render_assistant_plain_line(
    line: &str,
    content_width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    crate::presentation::render_wrapped_display_line(line, content_width)
        .into_iter()
        .map(|wrapped| Line::from(vec![Span::raw("  "), Span::styled(wrapped, style)]))
        .collect()
}

fn render_assistant_code_line(line: &str, content_width: usize) -> Vec<Line<'static>> {
    let code_style = Style::default().fg(PI_GREEN);
    let (gutter, code) = line
        .strip_prefix("  ")
        .map_or(("", line), |rest| ("  ", rest));
    let code_width = content_width
        .saturating_sub(crate::presentation::display_width(gutter))
        .max(1);

    crate::presentation::render_wrapped_display_line(code, code_width)
        .into_iter()
        .map(|wrapped| {
            let mut spans = vec![Span::raw("  ")];
            if !gutter.is_empty() {
                spans.push(Span::styled(gutter.to_owned(), code_style));
            }
            spans.push(Span::styled(wrapped, code_style));
            Line::from(spans)
        })
        .collect()
}

fn split_inline_bullet_runs(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if trimmed.matches("• ").count() < 2 {
        return None;
    }

    let items = trimmed
        .split("• ")
        .filter_map(|segment| {
            let segment = segment.trim();
            (!segment.is_empty()).then(|| format!("• {segment}"))
        })
        .collect::<Vec<_>>();

    (items.len() >= 2).then_some(items)
}

fn normalize_blank_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    while lines.first().is_some_and(is_visual_blank_line) {
        lines.remove(0);
    }
    while lines.last().is_some_and(is_visual_blank_line) {
        lines.pop();
    }

    let mut normalized = Vec::new();
    let mut last_was_blank = false;
    for line in lines {
        let is_blank = is_visual_blank_line(&line);
        if is_blank && last_was_blank {
            continue;
        }
        last_was_blank = is_blank;
        normalized.push(line);
    }
    normalized
}

fn render_user_markdown_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let mut plain_lines = lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    while plain_lines
        .first()
        .is_some_and(|line| line.trim().is_empty())
    {
        plain_lines.remove(0);
    }
    while plain_lines
        .last()
        .is_some_and(|line| line.trim().is_empty())
    {
        plain_lines.pop();
    }

    let content_width = width.saturating_sub(2) as usize;
    let mut rendered = Vec::new();
    for line in plain_lines {
        if line.trim().is_empty() {
            rendered.push(Line::from(vec![Span::raw("")]));
            continue;
        }
        for wrapped in
            crate::presentation::render_wrapped_display_line(line.as_str(), content_width)
        {
            rendered.push(Line::from(vec![Span::styled(
                wrapped,
                Style::default().fg(ratatui::style::Color::White),
            )]));
        }
    }
    rendered
}

fn is_reflowable_assistant_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with('#')
        && !trimmed.starts_with("```")
        && !trimmed.starts_with('┌')
        && !trimmed.starts_with('├')
        && !trimmed.starts_with('└')
        && !trimmed.starts_with('│')
        && !trimmed.starts_with("┃")
        && !trimmed.starts_with('>')
        && !trimmed.starts_with("- ")
        && !trimmed.starts_with("* ")
        && !trimmed.starts_with("• ")
        && !trimmed.starts_with("[image]")
}

fn paragraph_joiner(current: &str, next: &str) -> &'static str {
    if contains_cjk(current) || contains_cjk(next) {
        ""
    } else {
        " "
    }
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        ('\u{4E00}'..='\u{9FFF}').contains(&ch)
            || ('\u{3040}'..='\u{30FF}').contains(&ch)
            || ('\u{AC00}'..='\u{D7AF}').contains(&ch)
    })
}

fn assistant_line_style(line: &str) -> Style {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        Style::default().fg(PI_HEADING).add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("```") {
        Style::default().fg(PI_DIM_GRAY)
    } else if trimmed.starts_with('┌')
        || trimmed.starts_with('├')
        || trimmed.starts_with('└')
        || trimmed.starts_with('│')
    {
        Style::default().fg(PI_GRAY)
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

fn user_block_line(mut line: Line<'static>) -> Line<'static> {
    line.spans.insert(0, Span::raw("  "));
    if line.spans.is_empty() {
        line.spans
            .push(Span::styled("", Style::default().bg(PI_USER_MSG_BG)));
        return line;
    }

    for span in &mut line.spans {
        span.style = span.style.bg(PI_USER_MSG_BG);
    }
    line
}

fn build_assistant_contents(text: &str) -> Vec<MessageContent> {
    if let Some(body) = text.trim().strip_prefix(PROVIDER_ERROR_REPLY_PREFIX) {
        return vec![parse_provider_error_content(body)];
    }

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
            other @ (TuiSectionSpec::Narrative { .. }
            | TuiSectionSpec::KeyValues { .. }
            | TuiSectionSpec::ActionGroup { .. }
            | TuiSectionSpec::Checklist { .. }
            | TuiSectionSpec::Callout { .. }
            | TuiSectionSpec::Preformatted { .. }) => {
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

fn parse_provider_error_content(body: &str) -> MessageContent {
    let segments = body
        .split(" | ")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let mut summary = segments
        .first()
        .copied()
        .unwrap_or("provider request failed")
        .to_owned();
    let mut details = Vec::new();

    if let Some((trimmed_summary, inline_details)) = extract_summary_details(&summary) {
        summary = trimmed_summary;
        details.extend(inline_details);
    }

    for segment in segments.iter().skip(1) {
        append_provider_error_segment(segment, &mut details);
    }
    summary = compact_provider_error_summary(&summary);
    details = compact_provider_error_details(details);

    MessageContent::Error {
        title: "provider error".to_owned(),
        summary,
        details,
    }
}

fn compact_provider_error_summary(summary: &str) -> String {
    let Some(rest) = summary.strip_prefix("provider returned status ") else {
        return summary.to_owned();
    };
    let Some((status, rest)) = rest.split_once(" for model `") else {
        return summary.to_owned();
    };
    let Some((model, rest)) = rest.split_once("` on attempt ") else {
        return summary.to_owned();
    };
    let attempt = rest.trim();
    if attempt.is_empty() {
        return summary.to_owned();
    }

    format!("{status} · {model} · {attempt}")
}

fn extract_summary_details(summary: &str) -> Option<(String, Vec<String>)> {
    let mut trimmed_summary = summary.trim().to_owned();
    let mut details = Vec::new();

    if let Some(start) = trimmed_summary.find(" (last_reason=")
        && trimmed_summary.ends_with(')')
    {
        let reason =
            trimmed_summary[start + " (last_reason=".len()..trimmed_summary.len() - 1].trim();
        if !reason.is_empty() {
            details.push(format!("last_reason: {reason}"));
        }
        trimmed_summary = trimmed_summary[..start].trim().to_owned();
    }

    if let Some((prefix, json_value, suffix, separator)) =
        extract_inline_json_payload(&trimmed_summary)
    {
        let key = if separator == '=' {
            Some("response")
        } else {
            None
        };
        append_json_detail_lines(key, &json_value, &mut details);
        if !suffix.trim().is_empty() {
            details.push(suffix.trim().to_owned());
        }
        trimmed_summary = prefix.trim().trim_end_matches(':').trim().to_owned();
    }

    if details.is_empty() {
        None
    } else {
        Some((trimmed_summary, details))
    }
}

fn append_provider_error_segment(segment: &str, details: &mut Vec<String>) {
    if let Some((key, value)) = segment.split_once('=') {
        if let Ok(json_value) = serde_json::from_str::<Value>(value) {
            append_json_value_lines(key.trim(), &json_value, details);
        } else {
            details.push(format!("{}: {}", key.trim(), value.trim()));
        }
    } else if let Some((prefix, json_value, suffix, separator)) =
        extract_inline_json_payload(segment)
    {
        let key = if separator == '=' {
            prefix.trim().trim_end_matches('=').trim()
        } else {
            "response"
        };
        append_json_detail_lines(Some(key), &json_value, details);
        if !suffix.trim().is_empty() {
            details.push(suffix.trim().to_owned());
        }
    } else {
        details.push(segment.to_owned());
    }
}

fn extract_inline_json_payload(segment: &str) -> Option<(String, String, String, char)> {
    let bytes = segment.as_bytes();
    let mut start = None;
    let mut separator = ':';

    for (idx, ch) in segment.char_indices() {
        if (ch == '{' || ch == '[') && idx > 0 {
            let mut separator_index = idx.saturating_sub(1);
            while separator_index > 0 && bytes.get(separator_index).copied() == Some(b' ') {
                separator_index -= 1;
            }
            let sep = bytes.get(separator_index).copied().map(char::from);
            if matches!(sep, Some(':') | Some('=')) {
                start = Some(idx);
                separator = sep.unwrap_or(':');
                break;
            }
        }
    }

    let start = start?;
    let opening = segment[start..].chars().next()?;
    let closing = match opening {
        '{' => '}',
        '[' => ']',
        _ => return None,
    };

    let mut depth = 0usize;
    let mut end = None;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in segment[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            continue;
        }
        if ch == opening {
            depth += 1;
        } else if ch == closing {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                end = Some(start + offset + ch.len_utf8());
                break;
            }
        }
    }

    let end = end?;
    Some((
        segment[..start].to_owned(),
        segment[start..end].to_owned(),
        segment[end..].to_owned(),
        separator,
    ))
}

fn append_json_detail_lines(key: Option<&str>, json_text: &str, details: &mut Vec<String>) {
    if let Ok(value) = serde_json::from_str::<Value>(json_text) {
        if let (None, Value::Object(map)) = (key, &value) {
            for (entry_key, entry_value) in map {
                append_json_value_lines(entry_key, entry_value, details);
            }
        } else {
            append_json_value_lines(key.unwrap_or("response"), &value, details);
        }
    } else {
        let label = key.unwrap_or("response");
        details.push(format!("{label}: {json_text}"));
    }
}

fn append_json_value_lines(prefix: &str, value: &Value, details: &mut Vec<String>) {
    if prefix == "provider_failover"
        && let Some(object) = value.as_object()
    {
        if let (Some(attempt), Some(max_attempts)) = (
            object.get("attempt").and_then(Value::as_u64),
            object.get("max_attempts").and_then(Value::as_u64),
        ) {
            details.push(format!(
                "provider_failover.attempt: {attempt}/{max_attempts}"
            ));
        }
        if let Some(reason) = object.get("reason").and_then(Value::as_str) {
            details.push(format!("provider_failover.reason: {reason}"));
        }
        if let Some(stage) = object.get("stage").and_then(Value::as_str) {
            details.push(format!("provider_failover.stage: {stage}"));
        }
        if let Some(model) = object.get("model").and_then(Value::as_str) {
            details.push(format!("provider_failover.model: {model}"));
        }
        if let Some(status_code) = object.get("status_code").and_then(Value::as_u64) {
            details.push(format!("provider_failover.status_code: {status_code}"));
        }
        for (key, value) in object {
            if matches!(
                key.as_str(),
                "attempt" | "max_attempts" | "reason" | "stage" | "model" | "status_code"
            ) {
                continue;
            }
            append_json_value_lines(&format!("{prefix}.{key}"), value, details);
        }
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, value) in map {
                append_json_value_lines(&format!("{prefix}.{key}"), value, details);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                append_json_value_lines(&format!("{prefix}[{index}]"), item, details);
            }
        }
        Value::String(text) => details.push(format!("{prefix}: {text}")),
        Value::Null | Value::Bool(_) | Value::Number(_) => {
            details.push(format!("{prefix}: {value}"))
        }
    }
}

fn compact_provider_error_details(details: Vec<String>) -> Vec<String> {
    let mut code = None;
    let mut message = None;
    let mut last_reason = None;
    let mut failover_reason = None;
    let mut failover_stage = None;
    let mut failover_attempt = None;
    let mut failover_status = None;
    let mut passthrough = Vec::new();

    for detail in details {
        if let Some(value) = detail.strip_prefix("code: ") {
            code = Some(value.to_owned());
        } else if let Some(value) = detail.strip_prefix("message: ") {
            message = Some(value.to_owned());
        } else if let Some(value) = detail.strip_prefix("last_reason: ") {
            last_reason = Some(value.to_owned());
        } else if let Some(value) = detail.strip_prefix("provider_failover.reason: ") {
            failover_reason = Some(value.to_owned());
        } else if let Some(value) = detail.strip_prefix("provider_failover.stage: ") {
            failover_stage = Some(value.to_owned());
        } else if let Some(value) = detail.strip_prefix("provider_failover.attempt: ") {
            failover_attempt = Some(value.to_owned());
        } else if detail.starts_with("provider_failover.model: ") {
        } else if let Some(value) = detail.strip_prefix("provider_failover.status_code: ") {
            failover_status = Some(value.to_owned());
        } else {
            passthrough.push(detail);
        }
    }

    let mut compacted = Vec::new();

    let mut response_parts = Vec::new();
    if let Some(code) = code {
        response_parts.push(code);
    }
    if let Some(message) = message {
        response_parts.push(message);
    }
    if !response_parts.is_empty() {
        compacted.push(response_parts.join(" · "));
    }

    let mut failover_parts = Vec::new();
    if let Some(reason) = failover_reason.or(last_reason) {
        failover_parts.push(reason);
    }
    if let Some(stage) = failover_stage {
        failover_parts.push(stage);
    }
    if let Some(attempt) = failover_attempt {
        failover_parts.push(attempt);
    }
    if let Some(status) = failover_status {
        failover_parts.push(status);
    }
    if !failover_parts.is_empty() {
        compacted.push(failover_parts.join(" · "));
    }

    compacted.extend(passthrough);
    compacted
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
    _title: &str,
    lines: &[String],
    _status: ToolStatus,
    width: u16,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let lines = dedupe_tool_activity_detail_lines(lines);
    rendered.push(Line::from(""));
    for line in &lines {
        rendered.extend(render_tool_detail_lines(line, width));
    }
    rendered.push(Line::from(""));
    rendered
}

fn dedupe_tool_activity_detail_lines(lines: &[String]) -> Vec<String> {
    let mut deduped = Vec::with_capacity(lines.len());
    let mut seen_structured_previews = std::collections::BTreeSet::new();
    let mut last_dedupe_key: Option<String> = None;
    for line in lines {
        let dedupe_key = tool_activity_dedupe_key(line);
        if last_dedupe_key.as_deref() == Some(dedupe_key.as_str()) {
            continue;
        }

        if tool_activity_line_starts_new_group(line) {
            seen_structured_previews.clear();
        }

        if let Some(preview) =
            compact_tool_request_preview(line).or_else(|| compact_tool_args_preview(line))
            && !seen_structured_previews.insert(preview)
        {
            continue;
        }

        deduped.push(line.clone());
        last_dedupe_key = Some(dedupe_key);
    }

    deduped
}

fn tool_activity_line_starts_new_group(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[')
        || trimmed.starts_with("• Called ")
        || trimmed.starts_with("• Closed ")
        || trimmed.starts_with("Called ")
        || trimmed.starts_with("Closed ")
        || trimmed.starts_with("Approval ")
        || trimmed.starts_with("Denied ")
}

fn tool_activity_dedupe_key(line: &str) -> String {
    if let Some(preview) = compact_tool_request_preview(line) {
        return format!("request:{preview}");
    }
    if let Some(preview) = compact_tool_args_preview(line) {
        return format!("args:{preview}");
    }
    if let Some(preview) = compact_tool_child_preview(line, "stdout") {
        return format!("stdout:{preview}");
    }
    if let Some(preview) = compact_tool_child_preview(line, "stderr") {
        return format!("stderr:{preview}");
    }
    if let Some(preview) = compact_tool_child_preview(line, "file") {
        return format!("file:{preview}");
    }
    if let Some(preview) = compact_tool_child_preview(line, "metrics") {
        return format!("metrics:{preview}");
    }
    if let Some((label, body)) = normalized_activity_headline(line) {
        return format!("status:{label}:{body}");
    }

    line.trim().to_owned()
}

fn compact_tool_child_preview(line: &str, label: &str) -> Option<String> {
    let trimmed = line.trim();
    let body = trimmed
        .strip_prefix(&format!("{label}:"))
        .or_else(|| trimmed.strip_prefix(&format!("{label} ")))
        .or_else(|| trimmed.strip_prefix(&format!("↳ {label} ")))?
        .trim_start();
    Some(body.to_owned())
}

fn normalized_activity_headline(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim().strip_prefix("• ").unwrap_or(line.trim());

    let (label, rest) = if let Some(status) = trimmed.strip_prefix('[') {
        let (status, rest) = status.split_once("] ")?;
        let label = match status {
            "running" | "pending" => "Called",
            "completed" | "failed" | "interrupted" => "Closed",
            "needs_approval" => "Approval",
            "denied" => "Denied",
            _ => return None,
        };
        (label, rest)
    } else if let Some(rest) = trimmed.strip_prefix("Called ") {
        ("Called", rest)
    } else if let Some(rest) = trimmed.strip_prefix("Closed ") {
        ("Closed", rest)
    } else if let Some(rest) = trimmed.strip_prefix("Approval ") {
        ("Approval", rest)
    } else if let Some(rest) = trimmed.strip_prefix("Denied ") {
        ("Denied", rest)
    } else {
        return None;
    };

    let (body, detail) = normalize_activity_target_and_detail(rest);
    let body = if let Some(detail) = detail {
        format!("{body} · {detail}")
    } else {
        body
    };

    Some((label.to_owned(), body))
}

fn compact_tool_request_preview(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let body = trimmed
        .strip_prefix("request:")
        .or_else(|| trimmed.strip_prefix("request "))?
        .trim_start();
    compact_structured_preview(body, 3)
}

fn compact_tool_args_preview(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let body = trimmed
        .strip_prefix("args:")
        .or_else(|| trimmed.strip_prefix("args "))
        .or_else(|| trimmed.strip_prefix("↳ args "))?
        .trim_start();
    compact_structured_preview(body, 3)
}

fn render_tool_detail_lines(line: &str, width: u16) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2).max(1) as usize;
    let trimmed = line.trim_start();

    if let Some(rendered) = render_status_activity_line(line, content_width) {
        return rendered;
    }

    if let Some(rendered) = render_named_activity_line(line, content_width) {
        return rendered;
    }

    if let Some(body) = trimmed.strip_prefix("↳ ") {
        let prefix = "↳ ";
        let body = if let Some(args) = body.strip_prefix("args ") {
            let compacted = compact_structured_preview(args, 3).unwrap_or_else(|| args.to_owned());
            format!("args {compacted}")
        } else if let Some(request) = body.strip_prefix("request ") {
            let compacted =
                compact_structured_preview(request, 3).unwrap_or_else(|| request.to_owned());
            format!("request {compacted}")
        } else {
            body.to_owned()
        };
        let body_width = content_width
            .saturating_sub(crate::presentation::display_width(prefix))
            .max(1);
        let wrapped = crate::presentation::render_wrapped_display_line(&body, body_width);
        return wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(prefix, Style::default().fg(PI_ACCENT)),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            " ".repeat(crate::presentation::display_width(prefix)),
                            Style::default().fg(PI_ACCENT),
                        ),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                }
            })
            .collect();
    }

    if let Some(request) = trimmed.strip_prefix("request:") {
        return render_tool_detail_lines(&format!("↳ request {}", request.trim_start()), width);
    }

    if let Some(request) = trimmed.strip_prefix("request ") {
        return render_tool_detail_lines(&format!("↳ request {}", request.trim_start()), width);
    }

    if let Some(args) = trimmed.strip_prefix("args:") {
        return render_tool_detail_lines(&format!("↳ args {}", args.trim_start()), width);
    }

    if let Some(args) = trimmed.strip_prefix("args ") {
        return render_tool_detail_lines(&format!("↳ args {}", args.trim_start()), width);
    }

    if let Some(stdout) = trimmed.strip_prefix("stdout:") {
        return render_tool_detail_lines(&format!("↳ stdout {}", stdout.trim_start()), width);
    }

    if let Some(stderr) = trimmed.strip_prefix("stderr:") {
        return render_tool_detail_lines(&format!("↳ stderr {}", stderr.trim_start()), width);
    }

    if let Some(file) = trimmed.strip_prefix("file:") {
        return render_tool_detail_lines(&format!("↳ file {}", file.trim_start()), width);
    }

    if let Some(metrics) = trimmed.strip_prefix("metrics:") {
        return render_tool_detail_lines(&format!("↳ metrics {}", metrics.trim_start()), width);
    }

    if let Some((prefix, body)) = line.split_once(':') {
        let prefix = format!("{prefix}: ");
        let (prefix_style, body_style) = match prefix.trim_end() {
            "stdout:" => (
                Style::default().fg(PI_GREEN).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            ),
            "stderr:" => (
                Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
                Style::default().fg(PI_DARK_GRAY),
            ),
            _ => (
                Style::default().fg(PI_GRAY),
                Style::default().fg(PI_DARK_GRAY),
            ),
        };
        let body_width = content_width
            .saturating_sub(crate::presentation::display_width(&prefix))
            .max(1);
        let wrapped =
            crate::presentation::render_wrapped_display_line(body.trim_start(), body_width);
        let continuation_prefix = " ".repeat(crate::presentation::display_width(&prefix));
        return wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                let display_prefix = if index == 0 {
                    prefix.clone()
                } else {
                    continuation_prefix.clone()
                };
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(display_prefix, prefix_style),
                    Span::styled(wrapped_line, body_style),
                ])
            })
            .collect();
    }

    crate::presentation::render_wrapped_display_line(line, content_width)
        .into_iter()
        .map(|wrapped_line| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
            ])
        })
        .collect()
}

fn render_named_activity_line(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim().strip_prefix("• ").unwrap_or(line.trim());

    let (headline_label, headline_style, rest) = if let Some(rest) = trimmed.strip_prefix("Called ")
    {
        (
            "Called",
            Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
            rest,
        )
    } else if let Some(rest) = trimmed.strip_prefix("Closed ") {
        (
            "Closed",
            Style::default().fg(PI_GRAY).add_modifier(Modifier::BOLD),
            rest,
        )
    } else if let Some(rest) = trimmed.strip_prefix("Approval ") {
        (
            "Approval",
            Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
            rest,
        )
    } else if let Some(rest) = trimmed.strip_prefix("Denied ") {
        (
            "Denied",
            Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
            rest,
        )
    } else {
        return None;
    };

    let display_body = if rest.contains(" (id=") || rest.contains(" - ") {
        let (headline_body, detail_suffix) = normalize_activity_target_and_detail(rest);
        if let Some(detail_suffix) = detail_suffix {
            format!("{headline_body} · {detail_suffix}")
        } else {
            headline_body
        }
    } else {
        rest.to_owned()
    };

    let body_width = content_width
        .saturating_sub(crate::presentation::display_width(headline_label) + 3)
        .max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(&display_body, body_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::styled("• ", Style::default().fg(PI_GRAY)),
                        Span::styled(format!("{headline_label} "), headline_style),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            " ".repeat(crate::presentation::display_width(headline_label) + 1),
                            headline_style,
                        ),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                }
            })
            .collect(),
    )
}

fn render_status_activity_line(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim();
    let status = trimmed.strip_prefix('[')?;
    let (status, rest) = status.split_once("] ")?;

    let (headline_label, headline_style) = match status {
        "running" | "pending" => (
            "Called",
            Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
        ),
        "completed" | "failed" | "interrupted" => (
            "Closed",
            Style::default().fg(PI_GRAY).add_modifier(Modifier::BOLD),
        ),
        "needs_approval" => (
            "Approval",
            Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
        ),
        "denied" => (
            "Denied",
            Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
        ),
        _ => return None,
    };

    let (headline_body, detail_suffix) = normalize_activity_target_and_detail(rest);
    let mut display_body = headline_body;
    if let Some(detail_suffix) = detail_suffix {
        display_body.push_str(" · ");
        display_body.push_str(detail_suffix.as_str());
    }

    let body_width = content_width
        .saturating_sub(crate::presentation::display_width(headline_label) + 3)
        .max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(&display_body, body_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    Line::from(vec![
                        Span::styled("• ", Style::default().fg(PI_GRAY)),
                        Span::styled(format!("{headline_label} "), headline_style),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            " ".repeat(crate::presentation::display_width(headline_label) + 1),
                            headline_style,
                        ),
                        Span::styled(wrapped_line, Style::default().fg(PI_DARK_GRAY)),
                    ])
                }
            })
            .collect(),
    )
}

fn normalize_activity_target_and_detail(rest: &str) -> (String, Option<String>) {
    let (target_with_id, detail_suffix) = rest
        .split_once(" - ")
        .map(|(target, detail)| (target.trim(), Some(detail.trim().to_owned())))
        .unwrap_or((rest.trim(), None));

    let target = if let Some(id_index) = target_with_id.find(" (id=") {
        target_with_id[..id_index].trim().to_owned()
    } else {
        target_with_id.to_owned()
    };

    (target, detail_suffix.filter(|detail| !detail.is_empty()))
}

fn render_error_block_lines(
    title: &str,
    summary: &str,
    details: &[String],
    width: u16,
) -> Vec<Line<'static>> {
    let title_label = format!("[{title}]");
    let summary = summary.trim();
    let details_body = details
        .iter()
        .map(|detail| detail.trim())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");

    let mut rendered = Vec::new();

    rendered.push(Line::from(""));

    let inline_width = crate::presentation::display_width(&title_label)
        + if summary.is_empty() {
            0
        } else {
            1 + crate::presentation::display_width(summary)
        };
    if inline_width <= width as usize {
        let mut spans = vec![Span::styled(
            title_label,
            Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
        )];
        if !summary.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                summary.to_owned(),
                Style::default().fg(PI_RED).add_modifier(Modifier::DIM),
            ));
        }
        rendered.push(Line::from(spans));
    } else {
        rendered.push(Line::from(vec![Span::styled(
            title_label,
            Style::default().fg(PI_RED).add_modifier(Modifier::BOLD),
        )]));

        if !summary.is_empty() {
            for wrapped in crate::presentation::render_wrapped_display_line(summary, width as usize)
            {
                rendered.push(Line::from(vec![Span::styled(
                    wrapped,
                    Style::default().fg(PI_RED).add_modifier(Modifier::DIM),
                )]));
            }
        }
    }

    if !details_body.is_empty() {
        for wrapped in
            crate::presentation::render_wrapped_display_line(&details_body, width as usize)
        {
            rendered.push(Line::from(vec![Span::styled(
                wrapped,
                Style::default().fg(PI_RED).add_modifier(Modifier::DIM),
            )]));
        }
    }

    rendered.push(Line::from(""));
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
    use super::{
        MessageContent, MessageList, ToolStatus, adjust_scroll_start_for_message_boundary,
        build_assistant_contents, dominant_block_bg,
    };
    use crate::chat::pi_surface::utils::{PI_ACCENT, PI_GRAY, PI_GREEN, PI_USER_MSG_BG};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};

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
    fn tool_activity_renders_without_background_block() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [completed] read_file (id=call-1)\n> stdout: ok".to_owned(),
        );

        let rendered = list.get_rendered_lines(40);
        assert!(
            rendered
                .iter()
                .filter(|line| line.spans.iter().any(|span| {
                    span.content.contains("Closed") || span.content.contains("stdout")
                }))
                .all(|line| dominant_block_bg(line).is_none())
        );
    }

    #[test]
    fn tool_activity_wraps_long_called_lines_cleanly() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.state_write({\"mode\":\"workflow\",\"current_phase\":\"verification\",\"iteration\":2})".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(42)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) <= 42)
        );
        assert!(rendered.iter().any(|line| line.contains("Called")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("demo_mcp.state_write"))
        );
    }

    #[test]
    fn tool_activity_wraps_arrow_child_lines_cleanly() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> ↳ args {\"path\":\"src/README.md\",\"depth\":2,\"includeHidden\":false}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(44)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) <= 44)
        );
        assert!(rendered.iter().any(|line| line.contains("↳ args")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("path=src/README.md"))
        );
    }

    #[test]
    fn tool_activity_compacts_bracket_status_lines_into_called_closed_flow() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [completed] read_file (id=call-1) - ok\n> stdout: done"
                .to_owned(),
        );

        let rendered = list
            .get_rendered_lines(48)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Closed read_file · ok"))
        );
        assert!(!rendered.iter().any(|line| line.contains("(id=call-1)")));
    }

    #[test]
    fn tool_activity_compacts_request_json_into_arrow_child_line() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> request: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ request")));
        assert!(rendered.iter().any(|line| line.contains("query=rust")));
        assert!(rendered.iter().any(|line| line.contains("limit=5")));
    }

    #[test]
    fn tool_activity_compacts_plain_args_without_arrow_prefix() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> args {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ args")));
        assert!(rendered.iter().any(|line| line.contains("query=rust")));
        assert!(rendered.iter().any(|line| line.contains("limit=5")));
    }

    #[test]
    fn tool_activity_compacts_plain_args_with_colon_prefix() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> args: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ args")));
        assert!(rendered.iter().any(|line| line.contains("query=rust")));
        assert!(rendered.iter().any(|line| line.contains("limit=5")));
    }

    #[test]
    fn tool_activity_compacts_indented_arrow_args_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n>   ↳ args {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}"
                .to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ args")));
        assert!(rendered.iter().any(|line| line.contains("query=rust")));
        assert!(rendered.iter().any(|line| line.contains("limit=5")));
    }

    #[test]
    fn plain_called_closed_lines_render_with_bullet_status_flow() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> Closed demo_mcp.search · ok".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Called demo_mcp.search"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Closed demo_mcp.search · ok"))
        );
    }

    #[test]
    fn plain_approval_and_denied_lines_render_with_bullet_status_flow() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Approval demo_mcp.search\n> Denied demo_mcp.search · blocked"
                .to_owned(),
        );

        let rendered = list
            .get_rendered_lines(56)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Approval demo_mcp.search"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Denied demo_mcp.search · blocked"))
        );
    }

    #[test]
    fn bracket_approval_and_denied_lines_normalize_into_status_flow() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [needs_approval] read_file (id=call-1) - operator confirmation required\n> [denied] read_file (id=call-1) - blocked".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Approval read_file · operator confirmation required"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("• Denied read_file · blocked"))
        );
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_approval_and_denied_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Approval demo_mcp.search\n> Approval demo_mcp.search\n> Denied demo_mcp.search · blocked\n> Denied demo_mcp.search · blocked".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let approval_count = rendered
            .iter()
            .filter(|line| line.contains("• Approval demo_mcp.search"))
            .count();
        let denied_count = rendered
            .iter()
            .filter(|line| line.contains("• Denied demo_mcp.search · blocked"))
            .count();

        assert_eq!(approval_count, 1);
        assert_eq!(denied_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_consecutive_bracket_approval_lines_with_different_ids() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [needs_approval] read_file (id=call-1) - operator confirmation required\n> [needs_approval] read_file (id=call-2) - operator confirmation required".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let approval_count = rendered
            .iter()
            .filter(|line| line.contains("• Approval read_file · operator confirmation required"))
            .count();

        assert_eq!(approval_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_args_when_request_and_args_match() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> request: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}\n> args {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ request")));
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("↳ args query=rust"))
        );
    }

    #[test]
    fn tool_activity_resets_request_dedupe_for_new_called_group() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> request: {\"query\":\"rust\",\"limit\":5}\n> Called demo_mcp.search_again\n> request: {\"query\":\"rust\",\"limit\":5}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let request_label_count = rendered
            .iter()
            .filter(|line| line.contains("↳ request"))
            .count();
        let query_count = rendered
            .iter()
            .filter(|line| line.contains("query=rust"))
            .count();

        assert_eq!(request_label_count, 2);
        assert!(query_count >= 2);
    }

    #[test]
    fn tool_activity_dedupes_request_when_matching_args_arrive_first() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> args {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}\n> request: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ args")));
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("↳ request query=rust"))
        );
    }

    #[test]
    fn tool_activity_dedupes_args_with_colon_prefix_when_request_matches() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> request: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}\n> args: {\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("↳ request")));
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("↳ args query=rust"))
        );
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_status_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> Called demo_mcp.search\n> Closed demo_mcp.search · ok\n> Closed demo_mcp.search · ok".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let called_count = rendered
            .iter()
            .filter(|line| line.contains("• Called demo_mcp.search"))
            .count();
        let closed_count = rendered
            .iter()
            .filter(|line| line.contains("• Closed demo_mcp.search · ok"))
            .count();

        assert_eq!(called_count, 1);
        assert_eq!(closed_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_consecutive_bracket_status_lines_with_different_ids() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [completed] read_file (id=call-1) - ok\n> [completed] read_file (id=call-2) - ok".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let closed_count = rendered
            .iter()
            .filter(|line| line.contains("• Closed read_file · ok"))
            .count();

        assert_eq!(closed_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_bracket_approval_lines_with_different_ids() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> [needs_approval] read_file (id=call-1) - operator confirmation required\n> [needs_approval] read_file (id=call-2) - operator confirmation required".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(72)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let approval_count = rendered
            .iter()
            .filter(|line| line.contains("• Approval read_file · operator confirmation required"))
            .count();

        assert_eq!(approval_count, 1);
    }

    #[test]
    fn tool_activity_compacts_file_and_metrics_into_arrow_children() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.edit\n> file: edit src/lib.rs (+2 / -1)\n> metrics: 42ms · exit=0".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ file edit src/lib.rs (+2 / -1)"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ metrics 42ms · exit=0"))
        );
    }

    #[test]
    fn tool_activity_compacts_stdout_into_arrow_children() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.exec\n> stdout: 2 lines · 22 bytes".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ stdout 2 lines · 22 bytes"))
        );
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_stdout_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.exec\n> stdout: 2 lines · 22 bytes\n> stdout: 2 lines · 22 bytes".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let stdout_count = rendered
            .iter()
            .filter(|line| line.contains("↳ stdout 2 lines · 22 bytes"))
            .count();

        assert_eq!(stdout_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_stderr_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.exec\n> stderr: 1 lines · 12 bytes\n> stderr: 1 lines · 12 bytes".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let stderr_count = rendered
            .iter()
            .filter(|line| line.contains("↳ stderr 1 lines · 12 bytes"))
            .count();

        assert_eq!(stderr_count, 1);
    }

    #[test]
    fn tool_activity_compacts_stderr_into_arrow_children() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.exec\n> stderr: 1 lines · 12 bytes".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(52)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ stderr 1 lines · 12 bytes"))
        );
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_metrics_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.exec\n> metrics: 42ms · exit=0\n> metrics: 42ms · exit=0".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(60)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let metrics_count = rendered
            .iter()
            .filter(|line| line.contains("↳ metrics 42ms · exit=0"))
            .count();

        assert_eq!(metrics_count, 1);
    }

    #[test]
    fn tool_activity_dedupes_consecutive_duplicate_file_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.edit\n> file: edit src/lib.rs (+2 / -1)\n> file: edit src/lib.rs (+2 / -1)".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let file_count = rendered
            .iter()
            .filter(|line| line.contains("↳ file edit src/lib.rs (+2 / -1)"))
            .count();

        assert_eq!(file_count, 1);
    }

    #[test]
    fn tool_activity_burst_keeps_unique_request_children_per_called_group() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "### Tool activity\n> Called demo_mcp.search\n> request: {\"query\":\"rust\",\"limit\":5}\n> Called demo_mcp.search_again\n> request: {\"query\":\"rust\",\"limit\":5}\n> file: edit src/lib.rs (+2 / -1)\n> metrics: 42ms · exit=0".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let request_label_count = rendered
            .iter()
            .filter(|line| line.contains("↳ request"))
            .count();

        assert_eq!(request_label_count, 2);
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ file edit src/lib.rs"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("↳ metrics 42ms · exit=0"))
        );
    }

    #[test]
    fn provider_error_promotes_to_structured_error_block() {
        let contents = build_assistant_contents(
            "[provider_error] provider returned status 401 for model `gpt-5.4` on attempt 1/3: {\"code\":\"INVALID_API_KEY\",\"message\":\"Invalid API key\"} | provider_failover={\"reason\":\"auth_rejected\",\"stage\":\"status_failure\",\"model\":\"gpt-5.4\",\"attempt\":1,\"max_attempts\":3,\"status_code\":401}",
        );

        assert!(matches!(
            contents.first(),
            Some(MessageContent::Error { title, summary, details })
                if title == "provider error"
                    && summary == "401 · gpt-5.4 · 1/3"
                    && details.iter().any(|line| {
                        line.contains("INVALID_API_KEY") && line.contains("Invalid API key")
                    })
                    && details.iter().any(|line| line.contains("auth_rejected"))
        ));
    }

    #[test]
    fn provider_error_rendering_wraps_long_jsonish_details() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "[provider_error] provider returned status 401 for model `gpt-5.4` on attempt 1/3: {\"code\":\"INVALID_API_KEY\",\"message\":\"Invalid API key\"} | provider_failover={\"reason\":\"auth_rejected\",\"stage\":\"status_failure\",\"model\":\"gpt-5.4\",\"attempt\":1,\"max_attempts\":3,\"status_code\":401}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(36)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) <= 36)
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("[provider error]"))
        );
        assert!(rendered.iter().any(|line| line.contains("INVALID_API_KEY")));
        assert!(rendered.iter().any(|line| line.contains("auth_rejected")));
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("provider_failover."))
        );
    }

    #[test]
    fn provider_error_renders_title_and_summary_inline_when_width_allows() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "[provider_error] status 401 · gpt-5.4 · attempt 1/3: {\"code\":\"INVALID_API_KEY\",\"message\":\"Invalid API key\"}".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(80)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| {
            line.contains("[provider error]") && line.contains("gpt-5.4") && line.contains("1/3")
        }));
    }

    #[test]
    fn provider_error_renders_without_background_block() {
        let mut list = MessageList::new();
        list.add_user_message("hi".to_owned());
        list.add_assistant_message(
            "[provider_error] provider returned status 401 for model `gpt-5.4` on attempt 1/3: {\"code\":\"INVALID_API_KEY\",\"message\":\"Invalid API key\"}".to_owned(),
        );

        let rendered = list.get_rendered_lines(40);
        assert!(
            rendered
                .iter()
                .filter(|line| line
                    .spans
                    .iter()
                    .any(|span| span.content.contains("provider error")))
                .all(|line| dominant_block_bg(line).is_none())
        );
    }

    #[test]
    fn inserts_blank_spacer_between_adjacent_colored_blocks() {
        let mut list = MessageList::new();
        list.add_user_message("hi".to_owned());
        list.add_assistant_message("### Tool activity\n> [completed] read_file".to_owned());

        let rendered = list.get_rendered_lines(40);
        let last_user_block_row = rendered
            .iter()
            .rposition(|line| dominant_block_bg(line) == Some(PI_USER_MSG_BG))
            .expect("user block row");
        let first_tool_block_row = rendered
            .iter()
            .enumerate()
            .find_map(|(idx, line)| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("read_file"))
                    .then_some(idx)
            })
            .expect("tool block row");

        assert!(first_tool_block_row > last_user_block_row);
        assert!(
            rendered[last_user_block_row + 1..first_tool_block_row]
                .iter()
                .any(|line| line
                    .spans
                    .iter()
                    .all(|span| span.style.bg.is_none() && span.content.trim().is_empty()))
        );
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

    #[test]
    fn assistant_markdown_table_renders_as_structured_grid() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "| 指标 | 数值 |\n| --- | --- |\n| 覆盖率 | 68% |\n| 平均响应时间 | 220ms |".to_owned(),
        );

        let rendered = list
            .get_rendered_lines(64)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("┌")));
        assert!(rendered.iter().any(|line| line.contains("指标")));
        assert!(rendered.iter().any(|line| line.contains("覆盖率")));
        assert!(rendered.iter().any(|line| line.contains("220ms")));
        assert!(!rendered.iter().any(|line| line.contains("| --- |")));
    }

    #[test]
    fn assistant_markdown_code_block_preserves_line_breaks_and_green_styling() {
        let mut list = MessageList::new();
        list.add_assistant_message(
            "```rust
let alpha = 1;
let beta = alpha + 1;
```"
            .to_owned(),
        );

        let rendered = list.get_rendered_lines(48);
        let flattened = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let alpha_index = flattened
            .iter()
            .position(|line| line.contains("let alpha = 1;"))
            .expect("alpha line");
        let beta_index = flattened
            .iter()
            .position(|line| line.contains("let beta = alpha + 1;"))
            .expect("beta line");

        assert_ne!(alpha_index, beta_index);
        assert!(!flattened[alpha_index].contains("let beta = alpha + 1;"));
        assert!(!flattened[beta_index].contains("let alpha = 1;"));

        let alpha_span = rendered[alpha_index]
            .spans
            .iter()
            .find(|span| span.content.contains("let alpha = 1;"))
            .expect("alpha span");
        let beta_span = rendered[beta_index]
            .spans
            .iter()
            .find(|span| span.content.contains("let beta = alpha + 1;"))
            .expect("beta span");

        assert_eq!(alpha_span.style.fg, Some(PI_GREEN));
        assert_eq!(beta_span.style.fg, Some(PI_GREEN));
    }

    #[test]
    fn user_message_renders_single_bottom_padding_line() {
        let mut list = MessageList::new();
        list.add_user_message("你好".to_owned());

        let rendered = list
            .get_rendered_lines(20)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let non_empty = rendered
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.trim_end().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(non_empty, vec!["  你好"]);
    }

    #[test]
    fn rendered_lines_are_pre_padded_for_stable_cached_redraws() {
        let mut list = MessageList::new();
        list.add_assistant_message("hello".to_owned());

        let rendered = list
            .get_rendered_lines(18)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) == 18)
        );
    }

    #[test]
    fn mouse_scroll_is_symmetric() {
        let mut list = MessageList::new();
        list.scroll_offset = 10;
        list.mouse_step = 3;

        list.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(list.scroll_offset, 7);

        list.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(list.scroll_offset, 10);
    }

    #[test]
    fn key_scroll_uses_same_direction_model() {
        let mut list = MessageList::new();
        list.scroll_offset = 5;
        list.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(list.scroll_offset, 4);
        list.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(list.scroll_offset, 5);
    }

    #[test]
    fn space_scroll_matches_page_keys() {
        let mut list = MessageList::new();
        list.scroll_offset = 20;

        list.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(list.scroll_offset, 8);

        list.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT));
        assert_eq!(list.scroll_offset, 20);
    }

    #[test]
    fn page_step_uses_viewport_height_minus_overlap() {
        assert_eq!(super::page_step_for_height(1), 1);
        assert_eq!(super::page_step_for_height(2), 1);
        assert_eq!(super::page_step_for_height(8), 6);
        assert_eq!(super::page_step_for_height(20), 18);
    }

    #[test]
    fn mouse_step_tracks_viewport_height_fraction() {
        assert_eq!(super::mouse_step_for_height(1), 1);
        assert_eq!(super::mouse_step_for_height(8), 2);
        assert_eq!(super::mouse_step_for_height(20), 5);
    }

    #[test]
    fn render_updates_page_scroll_step_from_viewport_height() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..8 {
            list.add_assistant_message(format!("line-{idx}"));
        }
        list.scroll_offset = 20;

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let before = list.scroll_offset;
        list.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

        assert_eq!(list.scroll_offset, before.saturating_sub(6));
    }

    #[test]
    fn render_updates_mouse_scroll_step_from_viewport_height() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..8 {
            list.add_assistant_message(format!("line-{idx}"));
        }
        list.scroll_offset = 10;

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        list.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(list.scroll_offset, 8);
    }

    #[test]
    fn resize_preserves_top_visible_line_when_scrolled_up() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..20 {
            list.add_assistant_message(format!("line-{idx}"));
        }
        list.scroll_offset = 10;

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let before = terminal.backend().buffer().clone();
        let before_area = before.area;
        let before_top_line = (0..before_area.width)
            .map(|x| before[(x, 0)].symbol())
            .collect::<String>();

        terminal.backend_mut().resize(40, 10);
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let after_top_line = (0..after_area.width)
            .map(|x| after[(x, 0)].symbol())
            .collect::<String>();

        assert_eq!(before_top_line.trim_end(), after_top_line.trim_end());
    }

    #[test]
    fn new_messages_do_not_teleport_transcript_when_scrolled_up() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..20 {
            list.add_assistant_message(format!("line-{idx}"));
        }
        list.scroll_offset = 10;

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let before = terminal.backend().buffer().clone();
        let before_area = before.area;
        let before_top_line = (0..before_area.width)
            .map(|x| before[(x, 0)].symbol())
            .collect::<String>();

        list.add_assistant_message("new-tail-line".to_owned());
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let after_top_line = (0..after_area.width)
            .map(|x| after[(x, 0)].symbol())
            .collect::<String>();

        assert_eq!(before_top_line.trim_end(), after_top_line.trim_end());
    }

    #[test]
    fn toggling_compaction_does_not_teleport_transcript_when_scrolled_up() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..10 {
            list.add_assistant_message(format!("line-{idx}"));
        }
        list.add_assistant_message(
            "[session_local_recall_compacted_window]\nThis compacted checkpoint is session-local recall only.\nCompacted 2 earlier turns\nUser context:\n- ask"
                .to_owned(),
        );
        list.scroll_offset = 6;

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let before = terminal.backend().buffer().clone();
        let before_area = before.area;
        let before_top_line = (0..before_area.width)
            .map(|x| before[(x, 0)].symbol())
            .collect::<String>();

        assert!(list.toggle_latest_compaction());
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let after_top_line = (0..after_area.width)
            .map(|x| after[(x, 0)].symbol())
            .collect::<String>();

        assert_eq!(before_top_line.trim_end(), after_top_line.trim_end());
    }

    #[test]
    fn new_messages_keep_bottom_anchor_when_following_tail() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..20 {
            list.add_assistant_message(format!("line-{idx}"));
        }

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        list.add_assistant_message("new-tail-line".to_owned());
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let flattened = (0..after_area.height)
            .map(|y| {
                (0..after_area.width)
                    .map(|x| after[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(flattened.contains("new-tail-line"));
        assert_eq!(list.scroll_offset, 0);
    }

    #[test]
    fn width_resize_preserves_bottom_anchor_for_wrapped_tail_content() {
        let backend = TestBackend::new(48, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..8 {
            list.add_assistant_message(format!(
                "line-{idx} keeps a long wrapped transcript chunk stable while the terminal width shrinks"
            ));
        }

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        terminal.backend_mut().resize(24, 8);
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let flattened = (0..after_area.height)
            .map(|y| {
                (0..after_area.width)
                    .map(|x| after[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(flattened.contains("line-7"));
        assert_eq!(list.scroll_offset, 0);
    }

    #[test]
    fn resize_preserves_bottom_anchor_when_following_tail() {
        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut list = MessageList::new();
        for idx in 0..20 {
            list.add_assistant_message(format!("line-{idx}"));
        }

        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        terminal.backend_mut().resize(40, 10);
        terminal.draw(|f| list.render(f, f.area())).expect("draw");
        let after = terminal.backend().buffer().clone();
        let after_area = after.area;
        let flattened = (0..after_area.height)
            .map(|y| {
                (0..after_area.width)
                    .map(|x| after[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(flattened.contains("line-19"));
        assert_eq!(list.scroll_offset, 0);
    }

    #[test]
    fn assistant_message_keeps_single_trailing_blank_line() {
        let mut list = MessageList::new();
        list.add_assistant_message("Hello.".to_owned());
        list.add_user_message("next".to_owned());

        let rendered = list
            .get_rendered_lines(20)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let hello_index = rendered
            .iter()
            .position(|line| line.contains("Hello."))
            .expect("assistant line");
        let next_index = rendered
            .iter()
            .position(|line| line.contains("next"))
            .expect("next user line");

        assert_eq!(next_index.saturating_sub(hello_index), 3);
    }

    #[test]
    fn transcript_does_not_start_with_a_forced_blank_row() {
        let mut list = MessageList::new();
        list.add_startup_header("0.1.0".to_owned(), "help".to_owned(), Vec::new());

        let rendered = list
            .get_rendered_lines(24)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.first().is_some_and(|line| line.contains("loong")));
    }

    #[test]
    fn startup_header_wraps_long_section_values_to_viewport_width() {
        let mut list = MessageList::new();
        list.add_startup_header(
            "0.1.0".to_owned(),
            "help".to_owned(),
            vec![(
                "Skills".to_owned(),
                vec!["demo-skill, demo-helper, browser-preview, docx-helper".to_owned()],
            )],
        );

        let rendered = list
            .get_rendered_lines(28)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) <= 28)
        );
        assert!(rendered.iter().any(|line| line.contains("demo-skill")));
        assert!(rendered.iter().any(|line| line.contains("docx-helper")));
    }

    #[test]
    fn startup_header_wraps_version_and_tutorial_to_viewport_width() {
        let mut list = MessageList::new();
        list.add_startup_header(
            "0.1.0-alpha.3 · feat/ui-ux-droid-parity-final-20260414 · 4fd18d6".to_owned(),
            "escape interrupt · : deck · / commands · ctrl+o compaction".to_owned(),
            Vec::new(),
        );

        let rendered = list
            .get_rendered_lines(24)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered
                .iter()
                .all(|line| crate::presentation::display_width(line) <= 24)
        );
        assert!(rendered.iter().any(|line| line.contains("loong ")));
        assert!(rendered.iter().any(|line| line.contains("compaction")));
    }

    #[test]
    fn startup_header_version_line_does_not_duplicate_v_prefix() {
        let mut list = MessageList::new();
        list.add_startup_header("v0.1.0-alpha.3".to_owned(), "help".to_owned(), Vec::new());

        let rendered = list
            .get_rendered_lines(40)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("loong v0.1.0-alpha.3"));
        assert!(!rendered.contains("loong vv0.1.0-alpha.3"));
    }

    #[test]
    fn startup_header_current_build_version_line_does_not_duplicate_v_prefix() {
        let version = crate::presentation::BuildVersionInfo::current().render_version_line();
        let mut list = MessageList::new();
        list.add_startup_header(version.clone(), "help".to_owned(), Vec::new());

        let rendered = list
            .get_rendered_lines(80)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(format!("loong {version}").as_str()));
        assert!(!rendered.contains(format!("loong v{version}").as_str()));
    }

    #[test]
    fn assistant_messages_trim_renderer_blank_edges_and_keep_two_space_indent() {
        let mut list = MessageList::new();
        list.add_assistant_message("Hello.".to_owned());

        let rendered = list
            .get_rendered_lines(20)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        let hello_index = rendered
            .iter()
            .position(|line| line.contains("Hello."))
            .expect("assistant line");
        assert!(rendered[hello_index].starts_with("  Hello."));
        assert!(
            rendered
                .get(hello_index + 1)
                .is_some_and(|line| line.trim().is_empty())
        );
    }

    #[test]
    fn assistant_inline_bullet_runs_split_into_separate_lines() {
        let mut list = MessageList::new();
        list.add_assistant_message("• first item • second item • third item".to_owned());

        let rendered = list
            .get_rendered_lines(36)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("• first item")));
        assert!(rendered.iter().any(|line| line.contains("• second item")));
        assert!(rendered.iter().any(|line| line.contains("• third item")));
    }

    #[test]
    fn rendered_system_activity_headline_uses_colored_spans() {
        let mut list = MessageList::new();
        list.add_rendered_lines(vec!["• Ran cargo test -p loong-app".to_owned()]);

        let rendered = list.get_rendered_lines(48);
        let line = rendered
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("cargo test"))
            })
            .expect("system activity line");

        assert_eq!(line.spans[0].content.as_ref(), "• ");
        assert_eq!(line.spans[0].style.fg, Some(PI_GREEN));
        assert_eq!(line.spans[1].content.as_ref(), "Ran ");
        assert_eq!(line.spans[1].style.fg, Some(PI_ACCENT));
    }

    #[test]
    fn rendered_system_activity_child_uses_tree_and_action_styling() {
        let mut list = MessageList::new();
        list.add_rendered_lines(vec!["  └ Read app.rs".to_owned()]);

        let rendered = list.get_rendered_lines(32);
        let line = rendered
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("app.rs"))
            })
            .expect("system child line");

        assert_eq!(line.spans[0].content.as_ref(), "  ");
        assert_eq!(line.spans[1].content.as_ref(), "└ ");
        assert_eq!(line.spans[1].style.fg, Some(PI_GRAY));
        assert_eq!(line.spans[2].content.as_ref(), "Read ");
        assert_eq!(line.spans[2].style.fg, Some(PI_ACCENT));
    }

    #[test]
    fn scroll_start_snaps_to_user_block_boundary() {
        let mut list = MessageList::new();
        list.add_startup_header("0.1.0".to_owned(), "help".to_owned(), Vec::new());
        list.add_user_message("hello world".to_owned());
        list.add_assistant_message("reply".to_owned());

        let rendered = list.get_rendered_lines(24);
        let first_user_bg = rendered
            .iter()
            .position(|line| dominant_block_bg(line).is_some())
            .expect("user block should be present");
        let inside_user_block = first_user_bg + 1;

        let snapped = adjust_scroll_start_for_message_boundary(&rendered, inside_user_block);
        assert_eq!(snapped, first_user_bg + 1);
    }
}
