use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde::Deserialize;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::CliResult;
use crate::chat::CliChatOptions;
use crate::chat::CliTurnRuntime;
use crate::chat::control_plane::ChatControlPlaneStore;
use crate::conversation::ConversationRuntimeBinding;
use crate::tui_surface::{TuiKeyValueSpec, TuiMessageSpec, TuiSectionSpec};

use super::command_palette::{CommandAction, CommandPalette};
use super::composer::Composer;
use super::i18n::{I18nService, PiCopy, resolve_default_language};
use super::message_list::MessageList;
use super::utils::*;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Focus {
    Composer,
    CommandPalette,
    MessageList,
}

pub struct App {
    pub message_list: MessageList,
    pub composer: Composer,
    pub command_palette: CommandPalette,
    pub focus: Focus,
    pub pending_turn: bool,
    pub turn_start: Option<std::time::Instant>,
    pub live_lines: Arc<StdMutex<Vec<String>>>,
    pub pending_task: Option<JoinHandle<CliResult<String>>>,
    pub pending_steers: VecDeque<String>,
    pub pending_queue: VecDeque<String>,
    pub composer_follow_up_intent: bool,
    pub live_render_width: Arc<AtomicUsize>,
    pub live_rerender: Option<super::super::CliChatLiveSurfaceRerender>,
    pub spinner_seed: u64,
    pub last_pending_signature: Option<u64>,
    pub last_render_width: u16,
    pub last_render_height: u16,
    pub cwd: String,
    pub model: String,
    pub i18n: I18nService,
}

impl App {
    pub fn new(
        runtime: &CliTurnRuntime,
        options: &CliChatOptions,
        render_width: usize,
    ) -> CliResult<Self> {
        let language = resolve_default_language();
        let mut app = Self {
            message_list: MessageList::new(),
            composer: Composer::new(),
            command_palette: CommandPalette::new(language),
            focus: Focus::Composer,
            pending_turn: false,
            turn_start: None,
            live_lines: Arc::new(StdMutex::new(Vec::new())),
            pending_task: None,
            pending_steers: VecDeque::new(),
            pending_queue: VecDeque::new(),
            composer_follow_up_intent: false,
            live_render_width: Arc::new(AtomicUsize::new(render_width.max(1))),
            live_rerender: None,
            spinner_seed: spinner_seed(),
            last_pending_signature: None,
            last_render_width: render_width as u16,
            last_render_height: 0,
            cwd: format_cwd(runtime),
            model: runtime.config.provider.model.clone(),
            i18n: I18nService::new(language),
        };

        let (version, tutorial, sections) =
            build_pi_startup_content(runtime, options, render_width, &app.i18n);
        app.message_list
            .add_startup_header(version, tutorial, sections);

        Ok(app)
    }

    pub fn render(&mut self, f: &mut Frame) {
        let size = f.area();
        self.last_render_width = size.width;
        self.last_render_height = size.height;
        let composer_height = self.composer.height_for_width(size.width);
        let palette_height = if matches!(self.focus, Focus::CommandPalette) {
            self.command_palette.desired_height() as u16
        } else {
            0
        };
        let reserved_without_pending = 1
            + composer_height
            + if palette_height > 0 {
                1 + palette_height
            } else {
                0
            }
            + 1
            + 1
            + 1;
        let max_pending_height = size.height.saturating_sub(reserved_without_pending).max(3);
        let max_pending_preview_lines = max_pending_height.saturating_sub(3).max(1) as usize;
        let live_lines = pending_live_lines(&self.live_lines, max_pending_preview_lines);
        let pending_lines = if self.pending_turn {
            let raw_pending_lines = build_pending_lines(
                self.turn_start,
                &live_lines,
                self.spinner_seed,
                &self.pending_steers,
                &self.pending_queue,
                size.width,
            );
            compact_pending_lines_for_height(raw_pending_lines, max_pending_height)
        } else {
            Vec::new()
        };
        let pending_height = if self.pending_turn {
            pending_lines.len() as u16
        } else {
            0
        };
        let transcript_line_count = self.message_list.rendered_line_count(size.width) as u16;
        let bottom_band_height = pending_height
            + 1
            + composer_height
            + if palette_height > 0 {
                1 + palette_height
            } else {
                0
            }
            + 1
            + 1;
        let available_transcript_height = size.height.saturating_sub(bottom_band_height).max(1);
        let transcript_height = if self.message_list.messages.is_empty() {
            0
        } else {
            transcript_line_count.min(available_transcript_height)
        };
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(transcript_height),
                Constraint::Length(0),
                Constraint::Length(pending_height),
                Constraint::Length(1),
                Constraint::Length(composer_height),
                Constraint::Length(if palette_height > 0 { 1 } else { 0 }),
                Constraint::Length(palette_height),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(size);

        let [
            transcript_area,
            _spacer_area,
            pending_area,
            composer_separator_area,
            composer_area,
            palette_separator_area,
            palette_area,
            footer_separator_area,
            footer_area,
        ] = main_layout.as_ref()
        else {
            return;
        };

        self.message_list.render(f, *transcript_area);

        if self.pending_turn {
            f.render_widget(Paragraph::new(pending_lines), *pending_area);
        }

        let line_color = PI_COTTON_CANDY;
        let composer_separator_is_blank =
            !self.pending_turn && self.message_list.trailing_colored_block(size.width);
        if composer_separator_is_blank {
            f.render_widget(Paragraph::new(""), *composer_separator_area);
        } else {
            f.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(line_color)),
                *composer_separator_area,
            );
        }

        self.composer
            .render(f, *composer_area, matches!(self.focus, Focus::Composer));
        if matches!(self.focus, Focus::Composer) {
            let (x, y) = self.composer.cursor_position(*composer_area);
            f.set_cursor_position((x, y));
        }

        if palette_height > 0 {
            f.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(line_color)),
                *palette_separator_area,
            );
            self.command_palette.render(f, *palette_area);
        }

        f.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(line_color)),
            *footer_separator_area,
        );

        let footer_line = if self.pending_turn && !self.composer.is_empty() {
            build_queue_footer_line(&self.i18n, self.pending_queue.len(), size.width)
        } else if self.pending_turn && !self.pending_queue.is_empty() {
            build_restore_footer_line(&self.i18n, self.pending_queue.len(), size.width)
        } else {
            build_status_footer_line(&self.cwd, &self.model, size.width)
        };
        f.render_widget(Paragraph::new(footer_line), *footer_area);
    }
}

pub async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    runtime: CliTurnRuntime,
    options: CliChatOptions,
) -> CliResult<()> {
    let mut last_known_size = terminal
        .size()
        .map_err(|e| format!("failed to query terminal size: {e}"))?;
    let render_width = last_known_size.width as usize;
    let mut app = App::new(&runtime, &options, render_width)?;
    if let Some(lines) = load_startup_release_lines(render_width).await {
        app.message_list.add_rendered_lines(lines);
    }
    let mut dirty = true;
    let mut last_resize_at: Option<std::time::Instant> = None;
    let mut last_resize_requires_quiet = false;
    let mut last_resize_draw_at: Option<std::time::Instant> = None;
    let mut pending_live_resize_rerender = false;

    loop {
        if maybe_finalize_pending_turn(terminal, &mut app, &runtime).await? {
            dirty = true;
        }

        if app.pending_turn {
            let signature = pending_render_signature(&app);
            if signature != app.last_pending_signature {
                app.last_pending_signature = signature;
                dirty = true;
            }
        } else {
            app.last_pending_signature = None;
        }

        let resize_ready = redraw_throttle_ready(
            last_resize_requires_quiet,
            last_resize_draw_at.map(|instant| instant.elapsed()),
        );
        if dirty && resize_ready {
            if pending_live_resize_rerender {
                if let Some(rerender) = app.live_rerender.as_ref() {
                    rerender();
                }
                pending_live_resize_rerender = false;
            }
            terminal
                .draw(|f| app.render(f))
                .map_err(|e| format!("draw error: {}", e))?;
            dirty = false;
            if last_resize_requires_quiet {
                last_resize_draw_at = Some(std::time::Instant::now());
                if last_resize_at
                    .map(|instant| instant.elapsed() >= Duration::from_millis(70))
                    .unwrap_or(true)
                {
                    last_resize_at = None;
                    last_resize_requires_quiet = false;
                    last_resize_draw_at = None;
                }
            } else {
                last_resize_at = None;
                last_resize_draw_at = None;
            }
        }

        let poll_timeout = if dirty && last_resize_requires_quiet && !resize_ready {
            Duration::from_millis(16)
        } else if app.pending_turn {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(poll_timeout).map_err(|e| format!("poll error: {}", e))? {
            let event = event::read().map_err(|e| format!("read error: {}", e))?;

            match event {
                Event::Key(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }

                    if key.code == KeyCode::Char('o')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if app.message_list.toggle_latest_compaction() {
                            app.focus = Focus::MessageList;
                        }
                        continue;
                    }

                    if app.pending_turn {
                        let mut pending_command = None;
                        let mut pending_submission = None;
                        if key.code == KeyCode::Up
                            && key.modifiers.contains(KeyModifiers::ALT)
                            && dequeue_pending_steer(&mut app)
                        {
                            continue;
                        }
                        match app.focus {
                            Focus::Composer => {
                                if matches!(key.code, KeyCode::Char('/') | KeyCode::Char(':'))
                                    && app.composer.is_empty()
                                {
                                    app.command_palette.show(":");
                                    app.focus = Focus::CommandPalette;
                                } else if is_transcript_navigation_key(key)
                                    && app.composer.is_empty()
                                {
                                    app.message_list.handle_key(key);
                                } else if key.code == KeyCode::Tab {
                                    if !app.composer.is_empty() {
                                        queue_pending_message(&mut app);
                                    } else {
                                        app.focus = Focus::MessageList;
                                    }
                                } else if let Some(msg) = app.composer.handle_key(key) {
                                    pending_submission = Some(msg);
                                } else if !app.composer.is_empty() {
                                    app.composer_follow_up_intent = true;
                                }
                            }
                            Focus::MessageList => {
                                if matches!(key.code, KeyCode::Char('/') | KeyCode::Char(':'))
                                    && app.composer.is_empty()
                                {
                                    app.command_palette.show(":");
                                    app.focus = Focus::CommandPalette;
                                } else {
                                    app.message_list.handle_key(key);
                                }
                                if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                                    app.focus = Focus::Composer;
                                }
                            }
                            Focus::CommandPalette => {
                                if let Some(action) = app.command_palette.handle_key(key) {
                                    match action {
                                        CommandAction::RunCommand(command) => {
                                            pending_command = Some(command.to_owned());
                                            app.focus = Focus::Composer;
                                        }
                                        CommandAction::Close => {
                                            app.focus = Focus::Composer;
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(msg) = pending_submission {
                            if msg == "/exit" {
                                break;
                            }
                            if msg.starts_with('/') || msg.starts_with(':') {
                                let command = if msg.starts_with(':') {
                                    format!("/{}", msg.trim_start_matches(':'))
                                } else {
                                    msg
                                };
                                pending_command = Some(command);
                            } else {
                                queue_pending_steer(&mut app, msg);
                            }
                        }
                        if let Some(command) = pending_command {
                            if command == "/exit" {
                                break;
                            }
                            run_surface_command(terminal, &mut app, &runtime, &options, &command)
                                .await?;
                        }
                        dirty = true;
                        continue;
                    }

                    let mut command_to_run = None;
                    let mut submitted_message = None;

                    match app.focus {
                        Focus::Composer => {
                            if key.code == KeyCode::Esc {
                                if !app.composer.is_empty() {
                                    app.composer.clear();
                                    app.composer_follow_up_intent = false;
                                }
                            } else if matches!(key.code, KeyCode::Char('/') | KeyCode::Char(':'))
                                && app.composer.is_empty()
                            {
                                app.command_palette.show(":");
                                app.focus = Focus::CommandPalette;
                            } else if is_transcript_navigation_key(key) && app.composer.is_empty() {
                                app.message_list.handle_key(key);
                            } else if key.code == KeyCode::Tab {
                                app.focus = Focus::MessageList;
                            } else if let Some(msg) = app.composer.handle_key(key) {
                                submitted_message = Some(msg);
                            }
                        }
                        Focus::CommandPalette => {
                            if let Some(action) = app.command_palette.handle_key(key) {
                                match action {
                                    CommandAction::RunCommand(command) => {
                                        command_to_run = Some(command.to_owned());
                                        app.focus = Focus::Composer;
                                    }
                                    CommandAction::Close => {
                                        app.focus = Focus::Composer;
                                    }
                                }
                            }
                        }
                        Focus::MessageList => {
                            if key.code == KeyCode::Tab {
                                app.focus = Focus::Composer;
                            } else {
                                app.message_list.handle_key(key);
                                if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                                    app.focus = Focus::Composer;
                                }
                            }
                        }
                    }

                    if let Some(msg) = submitted_message {
                        if msg == "/exit" {
                            break;
                        }

                        if msg.starts_with('/') || msg.starts_with(':') {
                            app.command_palette.show(&msg);
                            app.focus = Focus::CommandPalette;
                            continue;
                        }

                        if submitted_message_is_follow_up(&app, &msg) {
                            start_turn(terminal, &mut app, &runtime, msg, false).await?;
                        } else {
                            submit_user_turn(terminal, &mut app, &runtime, msg).await?;
                        }
                    } else if let Some(command) = command_to_run {
                        if command == "/exit" {
                            break;
                        }

                        run_surface_command(terminal, &mut app, &runtime, &options, &command)
                            .await?;
                    }
                    dirty = true;
                }
                Event::Mouse(mouse_event) => {
                    app.message_list.handle_mouse(mouse_event);
                    dirty = true;
                }
                Event::Resize(width, height) => {
                    let new_size = ratatui::layout::Rect::new(0, 0, width, height);
                    if new_size.width == last_known_size.width
                        && new_size.height == last_known_size.height
                    {
                        continue;
                    }
                    let width_changed = last_known_size.width != new_size.width;
                    last_resize_requires_quiet =
                        resize_reflow_required(last_known_size.width, new_size.width);
                    last_resize_at = last_resize_requires_quiet.then(std::time::Instant::now);
                    last_resize_draw_at = None;
                    last_known_size = new_size;
                    app.live_render_width
                        .store(new_size.width.max(1) as usize, Ordering::Relaxed);
                    if width_changed && app.live_rerender.is_some() {
                        pending_live_resize_rerender = true;
                    }
                    dirty = true;
                }
                Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }
    Ok(())
}

async fn run_surface_command<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    options: &CliChatOptions,
    input: &str,
) -> CliResult<()> {
    let width = current_render_width(terminal)?;
    let lines = build_command_lines(runtime, options, input, width).await?;
    app.message_list.add_rendered_lines(lines);
    app.focus = Focus::Composer;
    Ok(())
}

async fn submit_user_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    input: String,
) -> CliResult<()> {
    start_turn(terminal, app, runtime, input, true).await
}

async fn start_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    input: String,
    echo_user_message: bool,
) -> CliResult<()> {
    let width = current_render_width(terminal)?;
    app.live_render_width.store(width.max(1), Ordering::Relaxed);
    if echo_user_message {
        app.message_list.add_user_message(input.clone());
    }
    app.composer_follow_up_intent = false;
    app.spinner_seed = spinner_seed();
    app.last_pending_signature = None;
    app.pending_turn = true;
    app.turn_start = Some(std::time::Instant::now());
    app.focus = Focus::Composer;
    clear_live_lines(&app.live_lines);

    terminal
        .draw(|f| app.render(f))
        .map_err(|e| format!("draw error: {}", e))?;

    let sink = {
        let live_lines = Arc::clone(&app.live_lines);
        Arc::new(move |lines: Vec<String>| {
            if let Ok(mut state) = live_lines.lock() {
                *state = lines;
            }
        })
    };
    let (observer, rerender) = super::super::build_cli_chat_live_compact_observer_controller(
        Arc::clone(&app.live_render_width),
        sink,
    );
    app.live_rerender = Some(rerender);
    app.pending_task = Some(spawn_pending_turn(runtime.clone(), input, observer));
    Ok(())
}

fn queue_pending_steer(app: &mut App, input: String) {
    if input.trim().is_empty() {
        return;
    }
    app.pending_steers.push_back(input);
    app.focus = Focus::Composer;
}

fn queue_pending_message(app: &mut App) {
    let input = app.composer.take_input();
    if input.trim().is_empty() {
        return;
    }
    app.composer_follow_up_intent = false;
    app.pending_queue.push_back(input);
    app.focus = Focus::Composer;
}

fn dequeue_pending_steer(app: &mut App) -> bool {
    if let Some(input) = app.pending_queue.pop_back() {
        app.composer.set_input(input);
        app.focus = Focus::Composer;
        return true;
    }
    let Some(input) = app.pending_steers.pop_back() else {
        return false;
    };
    app.composer.set_input(input);
    app.focus = Focus::Composer;
    true
}

fn is_transcript_navigation_key(key: crossterm::event::KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char('j')
            | KeyCode::Char('k')
    ) || (matches!(key.code, KeyCode::Char(' '))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER))
}

fn submitted_message_is_follow_up(app: &App, msg: &str) -> bool {
    app.composer_follow_up_intent && !msg.starts_with('/') && !msg.starts_with(':')
}

fn display_columns(text: &str) -> usize {
    crate::presentation::display_width(text)
}

fn truncate_right_for_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_columns(text) <= width {
        return text.to_owned();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = crate::presentation::char_display_width(ch);
        if used + ch_width > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

fn truncate_middle_for_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_columns(text) <= width {
        return text.to_owned();
    }
    if width == 1 {
        return "…".to_owned();
    }

    let target_prefix_width = width.saturating_sub(1).div_ceil(2);
    let target_suffix_width = width.saturating_sub(1).saturating_sub(target_prefix_width);

    let mut prefix = String::new();
    let mut prefix_used = 0usize;
    for ch in text.chars() {
        let ch_width = crate::presentation::char_display_width(ch);
        if prefix_used + ch_width > target_prefix_width {
            break;
        }
        prefix.push(ch);
        prefix_used += ch_width;
    }

    let mut suffix_chars = Vec::new();
    let mut suffix_used = 0usize;
    for ch in text.chars().rev() {
        let ch_width = crate::presentation::char_display_width(ch);
        if suffix_used + ch_width > target_suffix_width {
            break;
        }
        suffix_chars.push(ch);
        suffix_used += ch_width;
    }
    suffix_chars.reverse();
    let suffix = suffix_chars.into_iter().collect::<String>();

    format!("{prefix}…{suffix}")
}

fn build_status_footer_line(cwd: &str, model: &str, width: u16) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::from(String::new());
    }

    let mut model_text = model.to_owned();
    let mut cwd_text = cwd.to_owned();
    let mut model_width = display_columns(&model_text);
    let mut cwd_width = display_columns(&cwd_text);

    if model_width >= width {
        model_text = truncate_right_for_width(&model_text, width.saturating_sub(1).max(1));
        model_width = display_columns(&model_text);
    }

    let available_for_cwd = width.saturating_sub(model_width + 1);
    if cwd_width > available_for_cwd {
        cwd_text = truncate_middle_for_width(&cwd_text, available_for_cwd);
        cwd_width = display_columns(&cwd_text);
    }

    let mut spacer_width = width.saturating_sub(cwd_width + model_width);
    if !cwd_text.is_empty() && !model_text.is_empty() && spacer_width == 0 {
        if cwd_width > model_width {
            cwd_text = truncate_middle_for_width(&cwd_text, cwd_width.saturating_sub(1));
            cwd_width = display_columns(&cwd_text);
        } else {
            model_text = truncate_right_for_width(&model_text, model_width.saturating_sub(1));
            model_width = display_columns(&model_text);
        }
        spacer_width = width.saturating_sub(cwd_width + model_width);
    }

    Line::from(vec![
        Span::styled(cwd_text, Style::default().fg(PI_GRAY)),
        Span::raw(" ".repeat(spacer_width)),
        Span::styled(model_text, Style::default().fg(PI_GRAY)),
    ])
}

fn build_queue_footer_line(i18n: &I18nService, queued: usize, width: u16) -> Line<'static> {
    let hint = i18n.text(PiCopy::FooterQueueHint).to_owned();
    let short_hint = i18n.text(PiCopy::FooterQueueShort).to_owned();
    let suffix = if queued > 0 {
        format!(" · queued ×{queued}")
    } else {
        String::new()
    };
    let max_width = width as usize;
    let total_width = display_columns(&hint) + display_columns(&suffix);
    if total_width <= max_width {
        let mut spans = vec![Span::styled(hint, Style::default().fg(PI_ACCENT))];
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(PI_GRAY)));
        }
        return Line::from(spans);
    }

    let short_total_width = display_columns(&short_hint) + display_columns(&suffix);
    if short_total_width <= max_width {
        let mut spans = vec![Span::styled(short_hint, Style::default().fg(PI_ACCENT))];
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(PI_GRAY)));
        }
        return Line::from(spans);
    }

    if display_columns(&short_hint) >= max_width {
        return Line::from(vec![Span::styled(
            truncate_right_for_width(&short_hint, max_width),
            Style::default().fg(PI_ACCENT),
        )]);
    }

    let remaining = max_width.saturating_sub(display_columns(&short_hint));
    Line::from(vec![
        Span::styled(short_hint, Style::default().fg(PI_ACCENT)),
        Span::styled(
            truncate_right_for_width(&suffix, remaining),
            Style::default().fg(PI_GRAY),
        ),
    ])
}

fn build_restore_footer_line(i18n: &I18nService, queued: usize, width: u16) -> Line<'static> {
    let full_text = format!(
        "{} {} · queued ×{}",
        queue_restore_shortcut_label(),
        i18n.text(PiCopy::FooterRestoreQueued),
        queued
    );
    let short_text = format!(
        "{} {} · ×{}",
        queue_restore_shortcut_label(),
        i18n.text(PiCopy::FooterRestoreShort),
        queued
    );
    let selected = if display_columns(&full_text) <= width as usize {
        full_text
    } else {
        short_text
    };
    Line::from(vec![Span::styled(
        truncate_right_for_width(&selected, width as usize),
        Style::default().fg(PI_GRAY),
    )])
}

fn queue_restore_shortcut_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Option + Up"
    } else {
        "Alt + Up"
    }
}

async fn build_command_lines(
    runtime: &CliTurnRuntime,
    options: &CliChatOptions,
    input: &str,
    width: usize,
) -> CliResult<Vec<String>> {
    let trimmed = input.trim();

    match trimmed {
        super::super::CLI_CHAT_HELP_COMMAND => {
            Ok(super::super::operator_surfaces::render_cli_chat_help_lines_with_width(width))
        }
        super::super::CLI_CHAT_STATUS_COMMAND => {
            let summary =
                super::super::operator_surfaces::build_cli_chat_startup_summary(runtime, options)?;
            Ok(
                super::super::operator_surfaces::render_cli_chat_status_lines_with_width(
                    &summary, width,
                ),
            )
        }
        super::super::CLI_CHAT_HISTORY_COMMAND => {
            #[cfg(feature = "memory-sqlite")]
            {
                let history_lines = super::super::operator_surfaces::load_history_lines(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                    &runtime.memory_config,
                )
                .await?;
                Ok(
                    super::super::operator_surfaces::render_cli_chat_history_lines_with_width(
                        &runtime.session_id,
                        runtime.config.memory.sliding_window,
                        &history_lines,
                        width,
                    ),
                )
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "history",
                        "history unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        super::super::CLI_CHAT_COMPACT_COMMAND => {
            #[cfg(feature = "memory-sqlite")]
            {
                let result = super::super::operator_surfaces::load_manual_compaction_result(
                    &runtime.config,
                    &runtime.session_id,
                    &runtime.turn_coordinator,
                    ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                )
                .await?;
                Ok(
                    super::super::operator_surfaces::render_manual_compaction_lines_with_width(
                        &runtime.session_id,
                        &result,
                        width,
                    ),
                )
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "compact",
                        "manual compaction unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/fast_lane_summary" => {
            #[cfg(feature = "memory-sqlite")]
            {
                let summary = crate::conversation::load_fast_lane_tool_batch_event_summary(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                    &runtime.memory_config,
                )
                .await?;
                Ok(super::super::render_fast_lane_summary_lines_with_width(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    &summary,
                    width,
                ))
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "fast_lane_summary",
                        "fast lane summary unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/safe_lane_summary" => {
            #[cfg(feature = "memory-sqlite")]
            {
                let summary = crate::conversation::load_safe_lane_event_summary(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                    &runtime.memory_config,
                )
                .await?;
                Ok(super::super::render_safe_lane_summary_lines_with_width(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    &runtime.config.conversation,
                    &summary,
                    width,
                ))
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "safe_lane_summary",
                        "safe lane summary unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/turn_checkpoint_summary" => {
            #[cfg(feature = "memory-sqlite")]
            {
                let diagnostics = runtime
                    .turn_coordinator
                    .load_turn_checkpoint_diagnostics_with_limit(
                        &runtime.config,
                        &runtime.session_id,
                        runtime.config.memory.sliding_window,
                        ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                    )
                    .await?;
                Ok(
                    super::super::render_turn_checkpoint_summary_lines_with_width(
                        &runtime.session_id,
                        runtime.config.memory.sliding_window,
                        &diagnostics,
                        width,
                    ),
                )
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "turn_checkpoint_summary",
                        "turn checkpoint summary unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/turn_checkpoint_repair" => {
            #[cfg(feature = "memory-sqlite")]
            {
                let outcome = runtime
                    .turn_coordinator
                    .repair_turn_checkpoint_tail(
                        &runtime.config,
                        &runtime.session_id,
                        ConversationRuntimeBinding::kernel(&runtime.kernel_ctx),
                    )
                    .await?;
                Ok(
                    super::super::render_turn_checkpoint_repair_lines_with_width(
                        &runtime.session_id,
                        &outcome,
                        width,
                    ),
                )
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "turn_checkpoint_repair",
                        "turn checkpoint repair unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/sessions" => {
            #[cfg(feature = "memory-sqlite")]
            {
                Ok(render_sessions_lines(runtime, width)?)
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "sessions",
                        "session queue unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/workers" => {
            #[cfg(feature = "memory-sqlite")]
            {
                Ok(render_workers_lines(runtime, width)?)
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "workers",
                        "worker queue unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/review" => {
            #[cfg(feature = "memory-sqlite")]
            {
                Ok(render_review_lines(runtime, width)?)
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "review",
                        "review queue unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        "/mission" => {
            #[cfg(feature = "memory-sqlite")]
            {
                Ok(render_mission_lines(runtime, width)?)
            }
            #[cfg(not(feature = "memory-sqlite"))]
            {
                Ok(
                    super::super::render_cli_chat_feature_unavailable_lines_with_width(
                        "mission",
                        "mission control unavailable: memory-sqlite feature disabled",
                        width,
                    ),
                )
            }
        }
        _ => Ok(
            super::super::render_cli_chat_command_usage_lines_with_width(
                "usage: /help | /status | /history | /compact | /sessions | /workers | /review | /mission | /exit",
                width,
            ),
        ),
    }
}

#[cfg(feature = "memory-sqlite")]
fn render_sessions_lines(runtime: &CliTurnRuntime, width: usize) -> CliResult<Vec<String>> {
    let store = ChatControlPlaneStore::new(&runtime.memory_config)?;
    let sessions = store.visible_sessions(&runtime.session_id, 24)?;
    let mut items = Vec::new();
    for session in sessions.iter().take(12) {
        items.push(TuiKeyValueSpec::Plain {
            key: session.session_id.clone(),
            value: format!(
                "{} · {} · turns={}{}",
                session.label,
                session.state,
                session.turn_count,
                session
                    .last_error
                    .as_deref()
                    .map(|error| format!(" · error={error}"))
                    .unwrap_or_default()
            ),
        });
    }
    if items.is_empty() {
        items.push(TuiKeyValueSpec::Plain {
            key: "queue".to_owned(),
            value: "No visible sessions rooted at the current scope.".to_owned(),
        });
    }
    let mut sections = vec![TuiSectionSpec::KeyValues {
        title: Some("visible lineage".to_owned()),
        items,
    }];
    if let Some(primary) = sessions.first()
        && let Some(details) = store.session_details(&primary.session_id, false)?
    {
        sections.push(TuiSectionSpec::KeyValues {
            title: Some("selected session detail".to_owned()),
            items: vec![
                TuiKeyValueSpec::Plain {
                    key: "label".to_owned(),
                    value: primary.label.clone(),
                },
                TuiKeyValueSpec::Plain {
                    key: "lineage root".to_owned(),
                    value: details
                        .lineage_root_session_id
                        .unwrap_or_else(|| "-".to_owned()),
                },
                TuiKeyValueSpec::Plain {
                    key: "lineage depth".to_owned(),
                    value: details.lineage_depth.to_string(),
                },
                TuiKeyValueSpec::Plain {
                    key: "trajectory turns".to_owned(),
                    value: details.trajectory_turn_count.to_string(),
                },
                TuiKeyValueSpec::Plain {
                    key: "events".to_owned(),
                    value: details.event_count.to_string(),
                },
                TuiKeyValueSpec::Plain {
                    key: "approvals".to_owned(),
                    value: details.approval_count.to_string(),
                },
            ],
        });
        if !details.recent_events.is_empty() {
            sections.push(TuiSectionSpec::Narrative {
                title: Some("recent events".to_owned()),
                lines: details.recent_events,
            });
        }
    }
    let message_spec = TuiMessageSpec {
        role: "sessions".to_owned(),
        caption: Some(format!("scope={}", runtime.session_id)),
        sections,
        footer_lines: vec!["Use /workers for delegate lanes and /review for approvals.".to_owned()],
    };
    Ok(super::super::render_cli_chat_message_spec_with_width(
        &message_spec,
        width,
    ))
}

#[cfg(feature = "memory-sqlite")]
fn render_workers_lines(runtime: &CliTurnRuntime, width: usize) -> CliResult<Vec<String>> {
    let store = ChatControlPlaneStore::new(&runtime.memory_config)?;
    let workers = store.visible_worker_sessions(&runtime.session_id, 24)?;
    let mut items = Vec::new();
    for worker in workers.iter().take(12) {
        items.push(TuiKeyValueSpec::Plain {
            key: worker.session_id.clone(),
            value: format!(
                "{} · {} · turns={}{}",
                worker.label,
                worker.state,
                worker.turn_count,
                worker
                    .last_error
                    .as_deref()
                    .map(|error| format!(" · error={error}"))
                    .unwrap_or_default()
            ),
        });
    }
    if items.is_empty() {
        items.push(TuiKeyValueSpec::Plain {
            key: "queue".to_owned(),
            value: "No visible delegate workers in the current scope.".to_owned(),
        });
    }
    let mut sections = vec![TuiSectionSpec::KeyValues {
        title: Some("delegate lanes".to_owned()),
        items,
    }];
    if let Some(primary) = workers.first()
        && let Some(details) = store.session_details(&primary.session_id, true)?
    {
        sections.push(TuiSectionSpec::KeyValues {
            title: Some("selected worker detail".to_owned()),
            items: vec![
                TuiKeyValueSpec::Plain {
                    key: "label".to_owned(),
                    value: primary.label.clone(),
                },
                TuiKeyValueSpec::Plain {
                    key: "state".to_owned(),
                    value: primary.state.clone(),
                },
                TuiKeyValueSpec::Plain {
                    key: "turns".to_owned(),
                    value: primary.turn_count.to_string(),
                },
                TuiKeyValueSpec::Plain {
                    key: "lineage depth".to_owned(),
                    value: details.lineage_depth.to_string(),
                },
                TuiKeyValueSpec::Plain {
                    key: "delegate events".to_owned(),
                    value: details.delegate_events.len().to_string(),
                },
            ],
        });
        if !details.delegate_events.is_empty() {
            sections.push(TuiSectionSpec::Narrative {
                title: Some("delegate lifecycle".to_owned()),
                lines: details.delegate_events,
            });
        }
    }
    let message_spec = TuiMessageSpec {
        role: "workers".to_owned(),
        caption: Some(format!("scope={}", runtime.session_id)),
        sections,
        footer_lines: vec![
            "Use /sessions for the full lineage and /mission for lane rollups.".to_owned(),
        ],
    };
    Ok(super::super::render_cli_chat_message_spec_with_width(
        &message_spec,
        width,
    ))
}

#[cfg(feature = "memory-sqlite")]
fn render_review_lines(runtime: &CliTurnRuntime, width: usize) -> CliResult<Vec<String>> {
    let store = ChatControlPlaneStore::new(&runtime.memory_config)?;
    let approvals = store.approval_queue(&runtime.session_id, 16)?;
    let mut sections = Vec::new();
    let mut queue_items = Vec::new();
    for approval in approvals.iter().take(8) {
        queue_items.push(TuiKeyValueSpec::Plain {
            key: approval.approval_request_id.clone(),
            value: format!(
                "{} · {}{}{}",
                approval.tool_name,
                approval.status,
                approval
                    .reason
                    .as_deref()
                    .map(|reason| format!(" · {reason}"))
                    .unwrap_or_default(),
                approval
                    .last_error
                    .as_deref()
                    .map(|error| format!(" · error={error}"))
                    .unwrap_or_default()
            ),
        });
    }
    if queue_items.is_empty() {
        queue_items.push(TuiKeyValueSpec::Plain {
            key: "queue".to_owned(),
            value: "No approval requests are currently recorded for this session.".to_owned(),
        });
    }
    sections.push(TuiSectionSpec::KeyValues {
        title: Some("review queue".to_owned()),
        items: queue_items,
    });
    if let Some(latest) = approvals.first() {
        let mut detail_lines = vec![
            format!("tool={}", latest.tool_name),
            format!("status={}", latest.status),
            format!("turn_id={}", latest.turn_id),
            format!("requested_at={}", latest.requested_at),
        ];
        if let Some(reason) = latest.reason.as_deref() {
            detail_lines.push(format!("reason={reason}"));
        }
        if let Some(rule_id) = latest.rule_id.as_deref() {
            detail_lines.push(format!("rule_id={rule_id}"));
        }
        if let Some(error) = latest.last_error.as_deref() {
            detail_lines.push(format!("last_error={error}"));
        }
        sections.push(TuiSectionSpec::Narrative {
            title: Some("latest approval".to_owned()),
            lines: detail_lines,
        });
    }
    let message_spec = TuiMessageSpec {
        role: "review".to_owned(),
        caption: Some(format!("scope={}", runtime.session_id)),
        sections,
        footer_lines: vec![
            "Governed actions will surface approval screens here when needed.".to_owned(),
        ],
    };
    Ok(super::super::render_cli_chat_message_spec_with_width(
        &message_spec,
        width,
    ))
}

#[cfg(feature = "memory-sqlite")]
fn render_mission_lines(runtime: &CliTurnRuntime, width: usize) -> CliResult<Vec<String>> {
    let store = ChatControlPlaneStore::new(&runtime.memory_config)?;
    let sessions = store.visible_sessions(&runtime.session_id, 32)?;
    let workers = store.visible_worker_sessions(&runtime.session_id, 32)?;
    let approvals = store.approval_queue(&runtime.session_id, 32)?;
    let state_mix = summarize_state_mix(sessions.iter().map(|session| session.state.as_str()));
    let worker_mix = summarize_state_mix(workers.iter().map(|worker| worker.state.as_str()));
    let summary_items = vec![
        TuiKeyValueSpec::Plain {
            key: "scope".to_owned(),
            value: runtime.session_id.clone(),
        },
        TuiKeyValueSpec::Plain {
            key: "provider".to_owned(),
            value: runtime
                .config
                .active_provider_id()
                .unwrap_or("-")
                .to_owned(),
        },
        TuiKeyValueSpec::Plain {
            key: "visible sessions".to_owned(),
            value: sessions.len().to_string(),
        },
        TuiKeyValueSpec::Plain {
            key: "delegate lanes".to_owned(),
            value: workers.len().to_string(),
        },
        TuiKeyValueSpec::Plain {
            key: "review queue".to_owned(),
            value: approvals.len().to_string(),
        },
        TuiKeyValueSpec::Plain {
            key: "session mix".to_owned(),
            value: state_mix.unwrap_or_else(|| "-".to_owned()),
        },
        TuiKeyValueSpec::Plain {
            key: "worker mix".to_owned(),
            value: worker_mix.unwrap_or_else(|| "-".to_owned()),
        },
    ];
    let recent_session_values = sessions
        .iter()
        .take(6)
        .map(|session| format!("{} ({})", session.label, session.state))
        .collect::<Vec<_>>();
    let recent_worker_values = workers
        .iter()
        .take(6)
        .map(|worker| format!("{} ({})", worker.label, worker.state))
        .collect::<Vec<_>>();
    let mut sections = vec![TuiSectionSpec::KeyValues {
        title: Some("mission control".to_owned()),
        items: summary_items,
    }];
    if !recent_session_values.is_empty() {
        sections.push(TuiSectionSpec::KeyValues {
            title: Some("recent sessions".to_owned()),
            items: vec![TuiKeyValueSpec::Csv {
                key: "sessions".to_owned(),
                values: recent_session_values,
            }],
        });
    }
    if !recent_worker_values.is_empty() {
        sections.push(TuiSectionSpec::KeyValues {
            title: Some("recent workers".to_owned()),
            items: vec![TuiKeyValueSpec::Csv {
                key: "workers".to_owned(),
                values: recent_worker_values,
            }],
        });
    }
    let message_spec = TuiMessageSpec {
        role: "mission".to_owned(),
        caption: Some("control plane".to_owned()),
        sections,
        footer_lines: vec![
            "Use /sessions, /workers, and /review to drill into each lane.".to_owned(),
        ],
    };
    Ok(super::super::render_cli_chat_message_spec_with_width(
        &message_spec,
        width,
    ))
}

#[cfg(feature = "memory-sqlite")]
fn summarize_state_mix<'a>(states: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut counts = std::collections::BTreeMap::new();
    for state in states {
        *counts.entry(state.to_owned()).or_insert(0usize) += 1;
    }
    if counts.is_empty() {
        return None;
    }
    Some(
        counts
            .into_iter()
            .map(|(state, count)| format!("{state}={count}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

async fn maybe_finalize_pending_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
) -> CliResult<bool> {
    let Some(handle) = app.pending_task.as_ref() else {
        return Ok(false);
    };
    if !handle.is_finished() {
        return Ok(false);
    }

    let handle = app
        .pending_task
        .take()
        .ok_or_else(|| "pending turn handle disappeared".to_owned())?;
    let assistant_text = handle
        .await
        .map_err(|error| format!("pending turn task failed to join: {error}"))??;
    let width = current_render_width(terminal)?;
    app.pending_turn = false;
    app.turn_start = None;
    app.live_rerender = None;
    clear_live_lines(&app.live_lines);
    app.focus = Focus::Composer;
    if super::super::build_cli_chat_approval_screen_spec(&assistant_text).is_some() {
        app.message_list.add_rendered_lines(
            super::super::render_cli_chat_assistant_lines_with_width(&assistant_text, width),
        );
    } else {
        app.message_list.add_assistant_message(assistant_text);
    }
    if let Some(next_input) = app.pending_steers.pop_front() {
        start_turn(terminal, app, runtime, next_input, true).await?;
    } else if let Some(next_input) = app.pending_queue.pop_front() {
        start_turn(terminal, app, runtime, next_input, true).await?;
    }
    Ok(true)
}

fn current_render_width<B: Backend>(terminal: &Terminal<B>) -> CliResult<usize> {
    terminal
        .size()
        .map(|size| size.width as usize)
        .map_err(|e| format!("failed to query terminal size: {e}"))
}

fn spawn_pending_turn(
    runtime: CliTurnRuntime,
    input: String,
    observer: crate::conversation::ConversationTurnObserverHandle,
) -> JoinHandle<CliResult<String>> {
    tokio::spawn(async move {
        let result = crate::agent_runtime::AgentRuntime::new()
            .run_turn_with_runtime_and_observer(
                &runtime,
                &crate::agent_runtime::AgentTurnRequest {
                    message: input,
                    turn_mode: crate::agent_runtime::AgentTurnMode::Interactive,
                    channel_id: runtime.session_address.channel_id.clone(),
                    account_id: runtime.session_address.account_id.clone(),
                    conversation_id: runtime.session_address.conversation_id.clone(),
                    thread_id: runtime.session_address.thread_id.clone(),
                    metadata: std::collections::BTreeMap::new(),
                    acp: runtime.explicit_acp_request,
                    acp_event_stream: false,
                    acp_bootstrap_mcp_servers: runtime.effective_bootstrap_mcp_servers.clone(),
                    acp_cwd: runtime
                        .effective_working_directory
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    live_surface_enabled: true,
                },
                None,
                Some(observer),
            )
            .await?;
        Ok(result.output_text)
    })
}

fn clear_live_lines(live_lines: &Arc<StdMutex<Vec<String>>>) {
    if let Ok(mut state) = live_lines.lock() {
        state.clear();
    }
}

fn pending_live_lines(live_lines: &Arc<StdMutex<Vec<String>>>, max_lines: usize) -> Vec<String> {
    let max_lines = max_lines.max(1);
    live_lines
        .lock()
        .map(|state| {
            let normalize = |mut lines: Vec<String>| {
                while lines.first().is_some_and(|line| line.trim().is_empty()) {
                    lines.remove(0);
                }
                while lines.last().is_some_and(|line| line.trim().is_empty()) {
                    lines.pop();
                }

                let mut normalized = Vec::new();
                let mut last_was_blank = false;
                for line in lines {
                    let is_blank = line.trim().is_empty();
                    if is_blank && last_was_blank {
                        continue;
                    }
                    last_was_blank = is_blank;
                    normalized.push(line);
                }
                normalized
            };

            if state.len() <= max_lines {
                return normalize(state.clone());
            }

            if let Some(blank_idx) = state.iter().position(|line| line.trim().is_empty()) {
                let (reasoning_lines, trailing_lines) = state.split_at(blank_idx);
                let visible_lines = trailing_lines.get(1..).unwrap_or(&[]);
                let reasoning = reasoning_lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .take((max_lines / 2).max(1))
                    .cloned()
                    .collect::<Vec<_>>();
                let visible = visible_lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .take(max_lines.saturating_sub(reasoning.len() + 1))
                    .cloned()
                    .collect::<Vec<_>>();
                if !reasoning.is_empty() && !visible.is_empty() {
                    let mut lines = reasoning;
                    lines.push(String::new());
                    lines.extend(visible);
                    return normalize(lines);
                }
            }

            normalize(state.iter().take(max_lines).cloned().collect())
        })
        .unwrap_or_default()
}

fn pending_render_signature(app: &App) -> Option<u64> {
    if !app.pending_turn {
        return None;
    }
    let start = app.turn_start?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    focus_ring_frame(start).hash(&mut hasher);
    get_spinner_verb_with_seed(start, app.spinner_seed).hash(&mut hasher);
    app.pending_steers
        .iter()
        .for_each(|message| message.hash(&mut hasher));
    app.pending_queue
        .iter()
        .for_each(|message| message.hash(&mut hasher));
    for line in pending_live_lines(&app.live_lines, pending_signature_preview_budget(app)) {
        line.hash(&mut hasher);
    }
    Some(hasher.finish())
}

fn pending_signature_preview_budget(app: &App) -> usize {
    if app.last_render_width == 0 || app.last_render_height == 0 {
        return 6;
    }

    let composer_height = app.composer.height_for_width(app.last_render_width);
    let palette_height = if matches!(app.focus, Focus::CommandPalette) {
        app.command_palette.desired_height() as u16
    } else {
        0
    };
    let reserved_without_pending = 1
        + composer_height
        + if palette_height > 0 {
            1 + palette_height
        } else {
            0
        }
        + 1
        + 1
        + 1;
    let max_pending_height = app
        .last_render_height
        .saturating_sub(reserved_without_pending)
        .max(3);
    max_pending_height.saturating_sub(3).max(1) as usize
}

fn build_pending_lines(
    turn_start: Option<std::time::Instant>,
    live_lines: &[String],
    spinner_seed: u64,
    pending_steers: &VecDeque<String>,
    pending_queue: &VecDeque<String>,
    width: u16,
) -> Vec<Line<'static>> {
    let start = turn_start.unwrap_or_else(std::time::Instant::now);
    let spinner_spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{} ", focus_ring_frame(start)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{}...", get_spinner_verb_with_seed(start, spinner_seed)),
            Style::default().fg(PI_CYAN).add_modifier(Modifier::BOLD),
        ),
    ];

    let content_width = width.saturating_sub(2).max(1) as usize;
    let mut lines = vec![Line::from(""), Line::from(spinner_spans)];
    if !live_lines.is_empty() {
        lines.push(Line::from(""));
    }
    let has_visible_reply_after_blank = live_lines
        .iter()
        .position(|line| line.trim().is_empty())
        .is_some_and(|blank_idx| {
            live_lines
                .iter()
                .skip(blank_idx + 1)
                .any(|line| !line.trim().is_empty())
        });
    let mut in_reasoning_block = has_visible_reply_after_blank;

    for line in live_lines {
        if line.trim().is_empty() {
            lines.push(Line::from(""));
            if has_visible_reply_after_blank {
                in_reasoning_block = false;
            }
            continue;
        }

        let style = if in_reasoning_block {
            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(ratatui::style::Color::White)
        };
        for wrapped in
            crate::presentation::render_wrapped_display_line(line.as_str(), content_width)
        {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(wrapped, style),
            ]));
        }
    }
    append_pending_input_preview_lines(
        &mut lines,
        pending_steers,
        pending_queue,
        width,
        !live_lines.is_empty(),
    );
    lines.push(Line::from(""));
    lines
}

fn append_pending_input_preview_lines(
    lines: &mut Vec<Line<'static>>,
    pending_steers: &VecDeque<String>,
    pending_queue: &VecDeque<String>,
    width: u16,
    has_live_preview: bool,
) {
    if pending_steers.is_empty() && pending_queue.is_empty() {
        return;
    }

    if has_live_preview || lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::from(""));
    }

    let content_width = width.saturating_sub(6).max(1) as usize;
    if !pending_steers.is_empty() {
        push_pending_input_header(
            lines,
            content_width,
            "Messages to be submitted after next tool call",
            Some("Esc"),
            "to interrupt and send immediately",
        );
        let preview_items = pending_steers
            .iter()
            .map(|message| {
                (
                    message.as_str(),
                    Style::default().fg(PI_CYAN).add_modifier(Modifier::DIM),
                )
            })
            .collect::<Vec<_>>();
        push_pending_input_lines(lines, &preview_items, content_width, "    ↳ ");
    }

    if !pending_queue.is_empty() {
        if !pending_steers.is_empty() {
            lines.push(Line::from(""));
        }
        push_pending_input_header(lines, content_width, "Queued follow-up messages", None, "");
        let preview_items = pending_queue
            .iter()
            .map(|message| {
                (
                    message.as_str(),
                    Style::default()
                        .fg(PI_GRAY)
                        .add_modifier(Modifier::DIM | Modifier::ITALIC),
                )
            })
            .collect::<Vec<_>>();
        push_pending_input_lines(lines, &preview_items, content_width, "    ↳ ");
    }
}

fn push_pending_input_header(
    lines: &mut Vec<Line<'static>>,
    content_width: usize,
    title: &str,
    key_hint: Option<&str>,
    suffix: &str,
) {
    let mut spans = vec![
        Span::styled(
            "• ",
            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
        ),
        Span::styled(title.to_owned(), Style::default().fg(PI_GRAY)),
    ];
    if let Some(key_hint) = key_hint {
        spans.push(Span::styled(
            " (press ".to_owned(),
            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(
            key_hint.to_owned(),
            Style::default().fg(PI_ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {suffix})"),
            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
        ));
    }
    for (line_index, wrapped) in crate::presentation::render_wrapped_text_line(
        "",
        &spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        content_width + 2,
    )
    .into_iter()
    .enumerate()
    {
        let prefix = if line_index == 0 { "" } else { "  " };
        lines.push(Line::from(vec![Span::styled(
            format!("{prefix}{wrapped}"),
            Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
        )]));
    }
}

fn push_pending_input_lines(
    lines: &mut Vec<Line<'static>>,
    messages: &[(&str, Style)],
    content_width: usize,
    first_prefix: &str,
) {
    let max_preview_messages = 3;
    for (message, message_style) in messages.iter().take(max_preview_messages) {
        let wrapped_lines =
            crate::presentation::render_wrapped_display_line(message, content_width);
        let wrapped_count = wrapped_lines.len();
        for (line_index, wrapped) in wrapped_lines.into_iter().take(3).enumerate() {
            let prefix = if line_index == 0 {
                first_prefix.to_owned()
            } else {
                "      ".to_owned()
            };
            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(wrapped, *message_style),
            ]));
        }

        if wrapped_count > 3 {
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled("…".to_owned(), *message_style),
            ]));
        }
    }

    let remaining_messages = messages.len().saturating_sub(max_preview_messages);
    if remaining_messages > 0 {
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(
                format!("… +{remaining_messages} more"),
                Style::default().fg(PI_GRAY).add_modifier(Modifier::DIM),
            ),
        ]));
    }
}

fn compact_pending_lines_for_height(
    mut lines: Vec<Line<'static>>,
    max_height: u16,
) -> Vec<Line<'static>> {
    let max_height = max_height.max(1) as usize;
    if lines.len() <= max_height {
        return lines;
    }

    let removable_blank_indices = [0usize, lines.len().saturating_sub(1), 2usize];
    for index in removable_blank_indices {
        if lines.len() <= max_height {
            break;
        }
        if lines
            .get(index)
            .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        {
            lines.remove(index);
        }
    }

    while lines.len() > max_height {
        if let Some(index) = lines.iter().enumerate().skip(2).find_map(|(idx, line)| {
            line.spans
                .iter()
                .all(|span| span.content.trim().is_empty())
                .then_some(idx)
        }) {
            lines.remove(index);
        } else {
            break;
        }
    }

    lines.truncate(max_height);
    lines
}

fn format_cwd(runtime: &CliTurnRuntime) -> String {
    if let Some(path) = runtime.effective_working_directory.as_ref() {
        return path.display().to_string();
    }

    std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "~".to_owned())
}

fn build_pi_startup_content(
    runtime: &CliTurnRuntime,
    options: &CliChatOptions,
    _render_width: usize,
    i18n: &I18nService,
) -> (String, String, Vec<(String, Vec<String>)>) {
    let version = crate::presentation::BuildVersionInfo::current().render_version_line();
    let mcp_servers = if runtime.effective_bootstrap_mcp_servers.is_empty() {
        vec!["none configured".to_owned()]
    } else {
        runtime.effective_bootstrap_mcp_servers.clone()
    };
    let skills = detect_repo_skills();
    let skills = if skills.is_empty() {
        vec!["none detected".to_owned()]
    } else {
        vec![skills.join(", ")]
    };

    let tutorial = i18n.text(PiCopy::Tutorial).to_owned();
    let mut sections = vec![
        (i18n.text(PiCopy::StartupSectionMcp).to_owned(), mcp_servers),
        (i18n.text(PiCopy::StartupSectionSkills).to_owned(), skills),
    ];

    if options.acp_event_stream || runtime.explicit_acp_request {
        sections.push((
            i18n.text(PiCopy::StartupSectionAcp).to_owned(),
            vec![format!(
                "requested={} · event_stream={}",
                runtime.explicit_acp_request, options.acp_event_stream
            )],
        ));
    }

    (version, tutorial, sections)
}

fn detect_repo_skills() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("skills") else {
        return Vec::new();
    };
    let mut names = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry.file_type().ok().and_then(|kind| {
                if kind.is_dir() {
                    Some(entry.file_name().to_string_lossy().to_string())
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    published_at: Option<String>,
    html_url: Option<String>,
    body: Option<String>,
}

async fn load_startup_release_lines(width: usize) -> Option<Vec<String>> {
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    let client = reqwest::Client::builder()
        .user_agent("loongclaw-pi-surface")
        .build()
        .ok()?;
    let response = tokio::time::timeout(
        Duration::from_millis(1500),
        client
            .get("https://api.github.com/repos/eastreams/loong/releases/latest")
            .send(),
    )
    .await
    .ok()?
    .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let release: GithubRelease = response.json().await.ok()?;
    format_startup_release_lines(&release, &current, width)
}

fn format_startup_release_lines(
    release: &GithubRelease,
    current: &str,
    width: usize,
) -> Option<Vec<String>> {
    if normalize_tag(&release.tag_name) == normalize_tag(current) {
        return None;
    }

    let rule = "─".repeat(width.max(12));
    let mut lines = vec![
        rule.clone(),
        " What's New".to_owned(),
        String::new(),
        format!(
            " [{}]{}",
            release.tag_name,
            release
                .published_at
                .as_deref()
                .and_then(|value| value.get(..10))
                .map(|date| format!(" - {date}"))
                .unwrap_or_default()
        ),
        String::new(),
    ];

    let mut added = 0usize;
    for line in release.body.as_deref().unwrap_or_default().lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if lines.last().is_some_and(|last| !last.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }
        lines.push(trimmed.to_owned());
        added += 1;
        if added >= 28 {
            break;
        }
    }

    if let Some(url) = release.html_url.as_deref() {
        lines.push(String::new());
        lines.push(format!(" Release: {url}"));
    }
    lines.push(rule);
    Some(lines)
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_ascii_lowercase()
}

fn resize_reflow_required(previous_width: u16, next_width: u16) -> bool {
    previous_width != next_width
}

fn redraw_throttle_ready(
    resize_requires_throttle: bool,
    since_last_draw: Option<Duration>,
) -> bool {
    !resize_requires_throttle
        || since_last_draw
            .map(|elapsed| elapsed >= Duration::from_millis(16))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{App, Focus};
    use crate::chat::pi_surface::command_palette::CommandPalette;
    use crate::chat::pi_surface::composer::Composer;
    use crate::chat::pi_surface::i18n::{I18nService, Language};
    use crate::chat::pi_surface::message_list::MessageList;
    use crate::chat::pi_surface::utils::PI_USER_MSG_BG;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    fn blank_app() -> App {
        App {
            message_list: MessageList::new(),
            composer: Composer::new(),
            command_palette: CommandPalette::new(Language::En),
            focus: Focus::Composer,
            pending_turn: false,
            turn_start: None,
            live_lines: Arc::new(StdMutex::new(Vec::new())),
            pending_task: None,
            pending_steers: Default::default(),
            pending_queue: Default::default(),
            composer_follow_up_intent: false,
            live_render_width: Arc::new(AtomicUsize::new(1)),
            live_rerender: None,
            spinner_seed: 1,
            last_pending_signature: None,
            last_render_width: 0,
            last_render_height: 0,
            cwd: "/tmp/example".to_owned(),
            model: "gpt-test".to_owned(),
            i18n: I18nService::new(Language::En),
        }
    }

    #[test]
    fn resize_reflow_only_requires_quiet_window_for_width_changes() {
        assert!(super::resize_reflow_required(80, 72));
        assert!(!super::resize_reflow_required(80, 80));
    }

    #[test]
    fn redraw_throttle_only_delays_rapid_width_resize_frames() {
        assert!(super::redraw_throttle_ready(false, None));
        assert!(super::redraw_throttle_ready(true, None));
        assert!(!super::redraw_throttle_ready(
            true,
            Some(Duration::from_millis(8))
        ));
        assert!(super::redraw_throttle_ready(
            true,
            Some(Duration::from_millis(16))
        ));
    }

    fn sample_release() -> super::GithubRelease {
        super::GithubRelease {
            tag_name: "v9.9.9".to_owned(),
            published_at: Some("2026-04-20T00:00:00Z".to_owned()),
            html_url: Some("https://github.com/eastreams/loong/releases/tag/v9.9.9".to_owned()),
            body: Some(
                "- Added a very long changelog line that should wrap cleanly inside narrow startup surfaces without overflowing the transcript width.".to_owned(),
            ),
        }
    }

    fn buffer_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn find_row(terminal: &Terminal<TestBackend>, needle: &str) -> Option<u16> {
        let buf = terminal.backend().buffer();
        let area = buf.area;
        for y in 0..area.height {
            let line = (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>();
            if line.contains(needle) {
                return Some(y);
            }
        }
        None
    }

    fn row_has_background(
        terminal: &Terminal<TestBackend>,
        row: u16,
        bg: ratatui::style::Color,
    ) -> bool {
        let buf = terminal.backend().buffer();
        let area = buf.area;
        (0..area.width).all(|x| buf[(x, row)].bg == bg)
    }

    #[test]
    fn status_footer_truncates_long_cwd_from_the_left() {
        let line = super::build_status_footer_line(
            "/Users/chum/.paseo/worktrees/07om2gl0/ui-ux-parity-final-20260414",
            "gpt-5.4",
            32,
        );
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 32);
        assert!(rendered.contains("gpt-5.4"));
        assert!(rendered.contains("…"));
        assert!(
            rendered.contains(
                "ui-ux-parity-final-20260414"
                    .chars()
                    .rev()
                    .take(10)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
                    .as_str()
            )
        );
        assert!(rendered.contains("/Users"));
    }

    #[test]
    fn status_footer_truncates_model_when_width_is_extremely_narrow() {
        let line =
            super::build_status_footer_line("/tmp/project", "gpt-5.4-super-long-model-name", 12);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 12);
        assert!(rendered.contains("…"));
    }

    #[test]
    fn status_footer_respects_display_width_for_cjk_paths() {
        let line = super::build_status_footer_line("/tmp/项目/聊天记录", "gpt-5.4", 16);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 16);
        assert!(rendered.contains("gpt-5.4"));
    }

    #[test]
    fn middle_truncation_preserves_both_path_ends() {
        let truncated =
            super::truncate_middle_for_width("/Users/chum/worktrees/project-name/session", 20);

        assert!(truncated.starts_with("/Users"));
        assert!(truncated.ends_with("session"));
        assert_eq!(crate::presentation::display_width(&truncated), 20);
    }

    #[test]
    fn startup_release_lines_wrap_to_requested_width() {
        let release = sample_release();
        let lines =
            super::format_startup_release_lines(&release, "v0.1.0", 80).expect("release lines");
        let mut list = MessageList::new();
        list.add_rendered_lines(lines);

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
                .all(|line| line.is_empty() || crate::presentation::display_width(line) <= 24)
        );
        assert!(rendered.iter().any(|line| line.contains("What's New")));
        assert!(rendered.iter().any(|line| line.contains("Release:")));
    }

    #[test]
    fn startup_release_lines_skip_current_version() {
        let release = sample_release();

        assert!(super::format_startup_release_lines(&release, "v9.9.9", 24).is_none());
    }

    #[test]
    fn queue_footer_truncates_to_available_width() {
        let line = super::build_queue_footer_line(&I18nService::new(Language::En), 12, 14);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 14);
        assert!(rendered.contains("…"));
    }

    #[test]
    fn queue_footer_prefers_short_hint_before_truncating() {
        let line = super::build_queue_footer_line(&I18nService::new(Language::En), 2, 20);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("Tab to queue"));
        assert!(!rendered.contains("Tab to queue message"));
    }

    #[test]
    fn restore_footer_truncates_to_available_width() {
        let line = super::build_restore_footer_line(&I18nService::new(Language::En), 12, 14);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 14);
        assert!(rendered.contains("…"));
    }

    #[test]
    fn restore_footer_prefers_short_hint_before_truncating() {
        let line = super::build_restore_footer_line(&I18nService::new(Language::En), 2, 32);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("restore queued"));
        assert!(!rendered.contains("to restore queued message"));
    }

    #[test]
    fn footer_tracks_content_when_transcript_is_short() {
        let backend = TestBackend::new(50, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.message_list.add_assistant_message("hello".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let footer_row = lines
            .iter()
            .position(|line| line.contains("/tmp/example"))
            .expect("footer row");

        assert!(footer_row < lines.len().saturating_sub(1));
    }

    #[test]
    fn wrapped_composer_expands_before_footer() {
        let backend = TestBackend::new(16, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.composer.set_input("abcdefg".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let footer_row = lines
            .iter()
            .position(|line| line.contains("gpt-test"))
            .expect("footer row");
        let wrapped_row = lines
            .iter()
            .enumerate()
            .find_map(|(idx, line)| line.contains("defg").then_some(idx))
            .expect("wrapped composer row");

        assert!(footer_row > wrapped_row);
    }

    #[test]
    fn footer_reaches_bottom_when_transcript_fills_available_height() {
        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..8 {
            app.message_list.add_user_message(format!("msg-{idx}"));
            app.message_list
                .add_assistant_message(format!("reply-{idx}"));
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let footer_row = lines
            .iter()
            .position(|line| line.contains("/tmp/example"))
            .expect("footer row");

        assert_eq!(footer_row, lines.len().saturating_sub(1));
    }

    #[test]
    fn pending_band_grows_when_live_lines_exist() {
        let backend = TestBackend::new(50, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["streamed reply line".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("streamed reply line"))
        );
    }

    #[test]
    fn startup_header_remains_visible_after_first_message() {
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_startup_header(
            "0.1.0".to_owned(),
            "tutorial".to_owned(),
            vec![("MCP".to_owned(), vec!["none".to_owned()])],
        );
        app.message_list.add_user_message("hi".to_owned());
        app.message_list.add_assistant_message("hello".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");
        assert!(lines.contains("loong"));
        assert!(lines.contains("[MCP]"));
        assert!(lines.contains("hi"));
        assert!(lines.contains("hello"));
    }

    #[test]
    fn pending_band_keeps_blank_padding_rows() {
        let backend = TestBackend::new(50, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let spinner_row = lines
            .iter()
            .position(|line| line.contains("..."))
            .expect("spinner row");
        assert!(spinner_row > 0);
        assert!(lines[spinner_row - 1].trim().is_empty());
    }

    #[test]
    fn compact_pending_lines_drops_padding_before_content_on_tiny_height() {
        let lines = super::build_pending_lines(
            Some(std::time::Instant::now()),
            &["visible reply".to_owned()],
            1,
            &std::collections::VecDeque::new(),
            &std::collections::VecDeque::new(),
            40,
        );

        let compacted = super::compact_pending_lines_for_height(lines, 3);
        let rendered = compacted
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 3);
        assert!(rendered.iter().any(|line| line.contains("visible reply")));
    }

    #[test]
    fn pending_band_renders_compact_live_preview_without_card_chrome() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "first streamed sentence".to_owned(),
                "second streamed sentence".to_owned(),
            ];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");
        assert!(lines.contains("first streamed sentence"));
        assert!(lines.contains("second streamed sentence"));
        assert!(!lines.contains("╭─"));
        assert!(!lines.contains("turn pipeline"));
    }

    #[test]
    fn pending_preview_renders_between_transcript_and_composer() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["streamed reply line".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let user_row = lines
            .iter()
            .position(|line| line.contains("hi"))
            .expect("user row");
        let preview_row = lines
            .iter()
            .position(|line| line.contains("streamed reply line"))
            .expect("preview row");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");

        assert!(preview_row > user_row);
        assert!(preview_row < composer_row);
    }

    #[test]
    fn composer_immediately_follows_pending_preview_band() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["streamed reply line".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let preview_row = lines
            .iter()
            .position(|line| line.contains("streamed reply line"))
            .expect("preview row");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");

        assert_eq!(composer_row, preview_row + 3);
    }

    #[test]
    fn pending_preview_shows_reasoning_before_visible_reply() {
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["quiet reasoning".to_owned(), "visible reply".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let reasoning_row = lines
            .iter()
            .position(|line| line.contains("quiet reasoning"))
            .expect("reasoning row");
        let visible_row = lines
            .iter()
            .position(|line| line.contains("visible reply"))
            .expect("visible row");

        assert!(reasoning_row < visible_row);
    }

    #[test]
    fn pending_preview_keeps_blank_row_between_spinner_and_live_lines() {
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["visible reply".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let spinner_row = lines
            .iter()
            .position(|line| line.contains("..."))
            .expect("spinner row");
        let preview_row = lines
            .iter()
            .position(|line| line.contains("visible reply"))
            .expect("preview row");

        assert_eq!(preview_row, spinner_row + 2);
        assert!(lines[spinner_row + 1].trim().is_empty());
    }

    #[test]
    fn pending_preview_live_lines_are_indented_like_assistant_output() {
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["visible reply".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let preview_row = lines
            .iter()
            .position(|line| line.contains("visible reply"))
            .expect("preview row");

        assert!(lines[preview_row].contains("  visible reply"));
    }
    #[test]
    fn pending_preview_wraps_long_live_lines_on_narrow_width() {
        let backend = TestBackend::new(28, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["visible reply wraps across the pending band".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let first_row = lines
            .iter()
            .position(|line| line.contains("visible reply"))
            .expect("first wrapped preview row");
        let second_row = lines
            .iter()
            .skip(first_row + 1)
            .position(|line| line.contains("pending band"))
            .map(|offset| first_row + 1 + offset)
            .expect("second wrapped preview row");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");

        assert_eq!(second_row, first_row + 1);
        assert!(composer_row > second_row);
    }

    #[test]
    fn pending_preview_expands_beyond_legacy_cap_when_height_allows() {
        let backend = TestBackend::new(18, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines =
                vec![(
                "a1 a2 a3 a4 a5 a6 a7 a8 a9 a10 a11 a12 a13 a14 a15 a16 a17 a18 a19 a20 omega"
            )
                .to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let rendered = buffer_lines(&terminal).join("\n");

        assert!(rendered.contains("omega"));
    }

    #[test]
    fn pending_preview_preserves_blank_separator_between_reasoning_and_reply() {
        let backend = TestBackend::new(70, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "quiet reasoning".to_owned(),
                String::new(),
                "visible reply".to_owned(),
            ];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let reasoning_row = lines
            .iter()
            .position(|line| line.contains("quiet reasoning"))
            .expect("reasoning row");
        let visible_row = lines
            .iter()
            .position(|line| line.contains("visible reply"))
            .expect("visible row");

        assert!(visible_row > reasoning_row + 1);
        assert!(lines[reasoning_row + 1].trim().is_empty());
    }

    #[test]
    fn pending_preview_styles_reasoning_dim_before_visible_reply() {
        let backend = TestBackend::new(70, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "quiet reasoning".to_owned(),
                String::new(),
                "visible reply".to_owned(),
            ];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let buf = terminal.backend().buffer();
        let reasoning_row = find_row(&terminal, "quiet reasoning").expect("reasoning row");
        let visible_row = find_row(&terminal, "visible reply").expect("visible row");

        assert_eq!(
            buf[(2, reasoning_row)].fg,
            crate::chat::pi_surface::utils::PI_GRAY
        );
        assert_eq!(buf[(2, visible_row)].fg, ratatui::style::Color::White);
    }

    #[test]
    fn pending_preview_truncation_preserves_reasoning_and_visible_segments() {
        let backend = TestBackend::new(70, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "reason-1".to_owned(),
                "reason-2".to_owned(),
                "reason-3".to_owned(),
                "reason-4".to_owned(),
                String::new(),
                "reply-1".to_owned(),
                "reply-2".to_owned(),
                "reply-3".to_owned(),
            ];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let rendered = buffer_lines(&terminal).join("\n");

        assert!(rendered.contains("reason-1"));
        assert!(rendered.contains("reason-2"));
        assert!(!rendered.contains("reason-3"));
        assert!(!rendered.contains("reason-4"));
        assert!(rendered.contains("reply-1"));
        assert!(!rendered.contains("reply-2"));
    }

    #[test]
    fn pending_live_lines_trim_outer_blank_lines_and_collapse_repeats() {
        let lines = Arc::new(StdMutex::new(vec![
            String::new(),
            String::new(),
            "reasoning".to_owned(),
            String::new(),
            String::new(),
            "reply".to_owned(),
            String::new(),
            String::new(),
        ]));

        let normalized = super::pending_live_lines(&lines, 6);
        assert_eq!(
            normalized,
            vec!["reasoning".to_owned(), String::new(), "reply".to_owned(),]
        );
    }

    #[test]
    fn pending_live_lines_expand_with_larger_preview_budget() {
        let lines = Arc::new(StdMutex::new(vec![
            "reason-1".to_owned(),
            "reason-2".to_owned(),
            "reason-3".to_owned(),
            String::new(),
            "reply-1".to_owned(),
            "reply-2".to_owned(),
            "reply-3".to_owned(),
            "reply-4".to_owned(),
        ]));

        let compact = super::pending_live_lines(&lines, 4);
        let expanded = super::pending_live_lines(&lines, 7);

        assert!(compact.len() < expanded.len());
        assert!(expanded.iter().any(|line| line.contains("reply-3")));
    }

    #[test]
    fn pending_signature_preview_budget_tracks_last_render_geometry() {
        let mut app = blank_app();
        app.last_render_width = 40;
        app.last_render_height = 20;

        assert!(super::pending_signature_preview_budget(&app) > 1);

        app.last_render_height = 8;
        assert_eq!(super::pending_signature_preview_budget(&app), 1);
    }

    #[test]
    fn transcript_navigation_key_helper_accepts_space_and_vim_scroll_keys() {
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE,)
        ));
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE,)
        ));
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE,)
        ));
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT,)
        ));
        assert!(!super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::ALT,)
        ));
    }

    #[test]
    fn submitted_message_typed_while_pending_stays_follow_up_after_turn_finishes() {
        let mut app = blank_app();
        app.composer_follow_up_intent = true;

        assert!(super::submitted_message_is_follow_up(&app, "follow up"));
        assert!(!super::submitted_message_is_follow_up(&app, "/status"));
        assert!(!super::submitted_message_is_follow_up(&app, ":status"));
    }

    #[test]
    fn pending_footer_yields_to_queue_hint_when_draft_exists() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.composer.set_input("queued draft".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");

        assert!(lines.contains("Tab to queue message"));
        assert!(!lines.contains("/tmp/example"));
    }

    #[test]
    fn pending_footer_shows_restore_hint_when_queue_exists() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_queue.push_back("queued draft".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");

        assert!(lines.contains("queued ×1"));
        assert!(lines.contains("Option + Up") || lines.contains("Alt + Up"));
        assert!(!lines.contains("/tmp/example"));
    }

    #[test]
    fn pending_preview_shows_queued_steer_and_follow_up_above_composer() {
        let backend = TestBackend::new(72, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_steers
            .push_back("nudge the current answer toward the root cause".to_owned());
        app.pending_queue
            .push_back("after that, summarize the diff".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let steer_header_row = lines
            .iter()
            .position(|line| line.contains("Messages to be submitted after next tool call"))
            .expect("steer header");
        let steer_row = lines
            .iter()
            .position(|line| line.contains("nudge the current answer"))
            .expect("steer preview");
        let queue_header_row = lines
            .iter()
            .position(|line| line.contains("Queued follow-up messages"))
            .expect("queue header");
        let queued_row = lines
            .iter()
            .position(|line| line.contains("after that, summarize"))
            .expect("queued preview");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");

        assert!(steer_header_row < steer_row);
        assert!(lines[steer_row].contains("↳"));
        assert!(queue_header_row < queued_row);
        assert!(lines[queued_row].contains("↳"));
        assert!(steer_row < queued_row);
        assert!(queued_row < composer_row);
    }

    #[test]
    fn pending_preview_collapses_extra_messages_into_overflow_count() {
        let backend = TestBackend::new(72, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_steers.push_back("first steer".to_owned());
        app.pending_steers.push_back("second steer".to_owned());
        app.pending_steers.push_back("third steer".to_owned());
        app.pending_steers.push_back("fourth steer".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);

        assert!(lines.iter().any(|line| line.contains("first steer")));
        assert!(lines.iter().any(|line| line.contains("third steer")));
        assert!(!lines.iter().any(|line| line.contains("fourth steer")));
        assert!(lines.iter().any(|line| line.contains("… +1 more")));
    }

    #[test]
    fn pending_preview_caps_total_items_across_steer_and_follow_up_queues() {
        let backend = TestBackend::new(72, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_steers.push_back("first steer".to_owned());
        app.pending_steers.push_back("second steer".to_owned());
        app.pending_queue.push_back("first follow-up".to_owned());
        app.pending_queue.push_back("second follow-up".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);

        assert!(lines.iter().any(|line| line.contains("first steer")));
        assert!(lines.iter().any(|line| line.contains("second steer")));
        assert!(lines.iter().any(|line| line.contains("first follow-up")));
        assert!(!lines.iter().any(|line| line.contains("second follow-up")));
        assert!(lines.iter().any(|line| line.contains("… +1 more")));
    }

    #[test]
    fn queue_pending_message_moves_draft_into_follow_up_queue() {
        let mut app = blank_app();
        app.composer.set_input("queued draft".to_owned());
        app.composer_follow_up_intent = true;

        super::queue_pending_message(&mut app);

        assert_eq!(app.pending_queue.len(), 1);
        assert_eq!(
            app.pending_queue.front().map(String::as_str),
            Some("queued draft")
        );
        assert!(app.composer.is_empty());
        assert!(!app.composer_follow_up_intent);
    }

    #[test]
    fn dequeue_pending_steer_prefers_follow_up_queue_before_steer_stack() {
        let mut app = blank_app();
        app.pending_steers.push_back("steer text".to_owned());
        app.pending_queue.push_back("queued follow-up".to_owned());

        assert!(super::dequeue_pending_steer(&mut app));
        assert_eq!(app.composer.take_input(), "queued follow-up");
        assert_eq!(app.pending_steers.len(), 1);
    }

    #[test]
    fn pending_signature_ignores_hidden_tail_lines() {
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "reason-1".to_owned(),
                "reason-2".to_owned(),
                "reason-3".to_owned(),
                String::new(),
                "reply-1".to_owned(),
                "reply-2".to_owned(),
                "hidden-tail".to_owned(),
            ];
        }
        let before = super::pending_render_signature(&app);
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec![
                "reason-1".to_owned(),
                "reason-2".to_owned(),
                "reason-3".to_owned(),
                String::new(),
                "reply-1".to_owned(),
                "reply-2".to_owned(),
                "different-hidden-tail".to_owned(),
            ];
        }
        let after = super::pending_render_signature(&app);

        assert_eq!(before, after);
    }

    #[test]
    fn pending_signature_changes_when_follow_up_preview_changes() {
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_steers.push_back("first steer".to_owned());
        let before = super::pending_render_signature(&app);
        app.pending_steers.clear();
        app.pending_queue
            .push_back("first queued follow-up".to_owned());
        let after = super::pending_render_signature(&app);

        assert_ne!(before, after);
    }

    #[test]
    fn pending_signature_changes_when_visible_preview_changes() {
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["reason-1".to_owned(), String::new(), "reply-1".to_owned()];
        }
        let before = super::pending_render_signature(&app);
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["reason-1".to_owned(), String::new(), "reply-2".to_owned()];
        }
        let after = super::pending_render_signature(&app);

        assert_ne!(before, after);
    }

    #[test]
    fn startup_overflow_still_keeps_user_block_top_padding_visible() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_startup_header(
            "0.1.0".to_owned(),
            "tutorial".to_owned(),
            vec![
                (
                    "MCP".to_owned(),
                    vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                ),
                (
                    "Skills".to_owned(),
                    vec![
                        "alpha".to_owned(),
                        "beta".to_owned(),
                        "gamma".to_owned(),
                        "delta".to_owned(),
                    ],
                ),
            ],
        );
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());

        terminal.draw(|f| app.render(f)).expect("draw");
        let user_row = find_row(&terminal, "hi").expect("user row");
        assert!(user_row > 0);
        assert!(
            row_has_background(&terminal, user_row - 1, PI_USER_MSG_BG),
            "expected the row above the visible user text to be the user block top padding"
        );
    }

    #[test]
    fn pending_transcript_keeps_user_block_bottom_padding_visible() {
        let backend = TestBackend::new(50, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_startup_header(
            "0.1.0".to_owned(),
            "tutorial".to_owned(),
            vec![
                (
                    "MCP".to_owned(),
                    vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                ),
                (
                    "Skills".to_owned(),
                    vec![
                        "alpha".to_owned(),
                        "beta".to_owned(),
                        "gamma".to_owned(),
                        "delta".to_owned(),
                    ],
                ),
            ],
        );
        app.message_list.add_user_message("nihao".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());

        terminal.draw(|f| app.render(f)).expect("draw");
        let user_row = find_row(&terminal, "nihao").expect("user row");
        let pending_row = find_row(&terminal, "…")
            .or_else(|| find_row(&terminal, "中"))
            .unwrap_or(0);

        assert!(row_has_background(&terminal, user_row + 1, PI_USER_MSG_BG));
        assert!(pending_row > user_row);
    }

    #[test]
    fn startup_overflow_with_pending_preview_keeps_user_block_and_preview_visible() {
        let backend = TestBackend::new(50, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_startup_header(
            "0.1.0".to_owned(),
            "tutorial".to_owned(),
            vec![
                (
                    "MCP".to_owned(),
                    vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                ),
                (
                    "Skills".to_owned(),
                    vec![
                        "alpha".to_owned(),
                        "beta".to_owned(),
                        "gamma".to_owned(),
                        "delta".to_owned(),
                    ],
                ),
            ],
        );
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["pending reply".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let user_row = find_row(&terminal, "hi").expect("user row");
        let preview_row = find_row(&terminal, "pending reply").expect("preview row");
        let composer_row = find_row(&terminal, "›").expect("composer row");

        assert!(row_has_background(&terminal, user_row - 1, PI_USER_MSG_BG));
        assert!(preview_row > user_row);
        assert!(preview_row < composer_row);
    }
}
