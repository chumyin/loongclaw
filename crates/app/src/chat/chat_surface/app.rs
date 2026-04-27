use crossterm::event::{
    self, Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

use crate::CliResult;
use crate::chat::CliChatOptions;
use crate::chat::CliTurnRuntime;
use crate::chat::control_plane::ChatControlPlaneStore;
use crate::tui_surface::{TuiCalloutTone, TuiKeyValueSpec, TuiMessageSpec, TuiSectionSpec};

use super::command_palette::{
    CommandAction, CommandPalette, SkillEntry, find_slash_command_spec, slash_command_specs,
};
use super::composer::Composer;
use super::i18n::{I18nService, SurfaceCopy, resolve_default_language};
use super::message_list::{MessageList, StartupEyeAnimation, StartupEyeFocus, StartupPanel};
use super::onboarding::{
    RepoOptionalSkill, StartupOnboardingController, StartupQuickstartFocus, StartupSurfaceSnapshot,
    startup_palette_eye_focus,
};
use super::utils::*;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Focus {
    Composer,
    CommandPalette,
    MessageList,
}

const FOOTER_BOTTOM_BREATHING_HEIGHT: u16 = 1;
const FOOTER_HORIZONTAL_INDENT: u16 = 2;
const PENDING_TOOL_ANIMATION_FRAME_MS: u64 = 90;
const PENDING_TOOL_LABEL_COLORS: [Color; 6] = [
    SURFACE_DIM_GRAY,
    SURFACE_GRAY,
    SURFACE_ACCENT,
    SURFACE_CYAN,
    Color::White,
    SURFACE_CYAN,
];
const PENDING_TOOL_BODY_COLORS: [Color; 6] = [
    SURFACE_GRAY,
    SURFACE_ACCENT,
    SURFACE_CYAN,
    Color::White,
    SURFACE_CYAN,
    SURFACE_ACCENT,
];

#[derive(Clone)]
struct PendingRenderCache {
    signature: u64,
    max_pending_height: u16,
    lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy)]
struct StartupEyeFeedback {
    animation: StartupEyeAnimation,
    until: std::time::Instant,
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
    pending_render_cache: Option<PendingRenderCache>,
    inline_skill_popup_active: bool,
    pub last_render_width: u16,
    pub last_render_height: u16,
    pub last_transcript_area: Rect,
    pub last_composer_area: Rect,
    pub last_palette_area: Rect,
    pub cwd: String,
    pub model: String,
    pub title: Option<String>,
    pub i18n: I18nService,
    startup_eye_feedback: Option<StartupEyeFeedback>,
    startup_onboarding: StartupOnboardingController,
}

impl App {
    pub fn new(
        runtime: &CliTurnRuntime,
        _options: &CliChatOptions,
        render_width: usize,
    ) -> CliResult<Self> {
        let language = resolve_default_language();
        let detected_skills =
            detect_available_skills(runtime.effective_working_directory.as_deref());
        let onboarding_optional_skills =
            detect_onboarding_optional_repo_skills(runtime.effective_working_directory.as_deref());
        let startup_mcp_count = runtime.effective_bootstrap_mcp_servers.len();
        let startup_onboarding = StartupOnboardingController::new(
            Some(runtime.resolved_path.clone()),
            runtime.effective_working_directory.clone(),
            onboarding_optional_skills,
            startup_mcp_count,
        );
        let mut command_palette = CommandPalette::new(language, detected_skills);
        refresh_command_palette_extension_commands(&mut command_palette, runtime);
        let mut app = Self {
            message_list: MessageList::new(),
            composer: Composer::new(),
            command_palette,
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
            pending_render_cache: None,
            inline_skill_popup_active: false,
            last_render_width: render_width as u16,
            last_render_height: 0,
            last_transcript_area: Rect::default(),
            last_composer_area: Rect::default(),
            last_palette_area: Rect::default(),
            cwd: format_cwd(runtime),
            model: runtime.config.provider.model.clone(),
            title: None,
            i18n: I18nService::new(language),
            startup_eye_feedback: None,
            startup_onboarding,
        };

        let (version, tutorial, sections, tips) = app.startup_onboarding.startup_content(&app.i18n);
        app.message_list.add_startup_header_with_tips_and_eye(
            version,
            tutorial,
            sections,
            tips,
            StartupEyeAnimation::Ambient,
        );

        Ok(app)
    }

    pub fn render(&mut self, f: &mut Frame) {
        let startup_eye_animation = self.startup_eye_animation();
        self.message_list
            .set_latest_startup_eye_animation(startup_eye_animation);
        self.message_list
            .set_latest_startup_panel(self.startup_panel());
        let size = f.area();
        self.last_render_width = size.width;
        self.last_render_height = size.height;
        let palette_visible =
            matches!(self.focus, Focus::CommandPalette) || self.inline_skill_popup_active;
        let history_browse_mode = should_use_history_browse_layout(self, palette_visible);
        let composer_height = if history_browse_mode {
            0
        } else {
            self.composer.height_for_area(size.width, size.height)
        };
        let palette_height = if palette_visible {
            self.command_palette.desired_height() as u16
        } else {
            0
        };
        let pending_lines =
            self.pending_lines_for(size.width, size.height, composer_height, palette_height);
        let pending_height = if self.pending_turn {
            pending_lines.len() as u16
        } else {
            0
        };
        let transcript_line_count = self.message_list.rendered_line_count(size.width) as u16;
        let composer_separator_height = if composer_height > 0 { 1 } else { 0 };
        let palette_separator_height = if palette_height > 0 { 1 } else { 0 };
        let footer_separator_height = if history_browse_mode { 0 } else { 1 };
        let footer_height = if history_browse_mode { 0 } else { 1 };
        let footer_bottom_spacing_height = if history_browse_mode {
            0
        } else {
            FOOTER_BOTTOM_BREATHING_HEIGHT
        };
        let bottom_band_height = pending_height
            + composer_separator_height
            + composer_height
            + palette_separator_height
            + palette_height
            + footer_separator_height
            + footer_height
            + footer_bottom_spacing_height;
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
                Constraint::Length(composer_separator_height),
                Constraint::Length(composer_height),
                Constraint::Length(palette_separator_height),
                Constraint::Length(palette_height),
                Constraint::Length(footer_separator_height),
                Constraint::Length(footer_height),
                Constraint::Length(footer_bottom_spacing_height),
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
            footer_bottom_spacing_area,
        ] = main_layout.as_ref()
        else {
            return;
        };

        self.last_transcript_area = *transcript_area;
        self.last_composer_area = if composer_height > 0 {
            *composer_area
        } else {
            Rect::default()
        };
        self.last_palette_area = if palette_visible {
            *palette_area
        } else {
            Rect::default()
        };

        self.message_list.render(f, *transcript_area);

        if self.pending_turn {
            f.render_widget(Paragraph::new(pending_lines), *pending_area);
        }

        let line_color = SURFACE_COTTON_CANDY;
        if composer_height > 0 {
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

            let composer_preview_override = matches!(self.focus, Focus::CommandPalette)
                .then(|| self.command_palette.composer_preview_text())
                .flatten();
            self.composer.render(
                f,
                *composer_area,
                matches!(self.focus, Focus::Composer),
                composer_preview_override.as_deref(),
            );
            if matches!(self.focus, Focus::Composer) {
                let (x, y) = self.composer.cursor_position(*composer_area);
                f.set_cursor_position((x, y));
            }
        }

        if palette_visible {
            f.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(line_color)),
                *palette_separator_area,
            );
            self.command_palette.render(f, *palette_area);
        }

        if footer_height > 0 {
            f.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(line_color)),
                *footer_separator_area,
            );

            let footer_content_area = footer_content_area(*footer_area);
            let footer_line = if self.pending_turn && !self.composer.is_empty() {
                build_queue_footer_line(
                    &self.i18n,
                    self.pending_queue.len(),
                    footer_content_area.width,
                )
            } else if self.pending_turn && !self.pending_queue.is_empty() {
                build_restore_footer_line(
                    &self.i18n,
                    self.pending_queue.len(),
                    footer_content_area.width,
                )
            } else {
                build_status_footer_line(&self.cwd, &self.model, footer_content_area.width)
            };
            f.render_widget(Paragraph::new(footer_line), footer_content_area);
            f.render_widget(Paragraph::new(""), *footer_bottom_spacing_area);
        }
    }

    fn startup_eye_animation(&self) -> StartupEyeAnimation {
        if let Some(feedback) = self.startup_eye_feedback
            && std::time::Instant::now() < feedback.until
        {
            return feedback.animation;
        }

        if self.startup_onboarding_active() {
            return self.startup_onboarding.eye_animation();
        }

        if self.pending_turn {
            return StartupEyeAnimation::Thinking(StartupEyeFocus::DownCenter);
        }

        if matches!(self.focus, Focus::CommandPalette) || self.inline_skill_popup_active {
            let focus = self
                .command_palette
                .selection_progress()
                .map(|(selected, total)| startup_palette_eye_focus(selected, total))
                .unwrap_or_else(|| {
                    if self.command_palette.is_skills_mode() {
                        StartupEyeFocus::DownLeft
                    } else {
                        StartupEyeFocus::DownRight
                    }
                });
            return if self.command_palette.is_commands_mode() {
                StartupEyeAnimation::Thinking(focus)
            } else {
                StartupEyeAnimation::Focus(focus)
            };
        }

        if !self.composer.is_empty() {
            return StartupEyeAnimation::Focus(StartupEyeFocus::DownCenter);
        }

        StartupEyeAnimation::Ambient
    }

    fn startup_panel(&self) -> Option<StartupPanel> {
        self.startup_onboarding
            .panel_for_surface(self.startup_surface_snapshot())
    }

    fn set_startup_eye_feedback(&mut self, animation: StartupEyeAnimation, duration: Duration) {
        self.startup_eye_feedback = Some(StartupEyeFeedback {
            animation,
            until: std::time::Instant::now() + duration,
        });
    }

    fn apply_startup_language_selection(&mut self) {
        self.startup_onboarding.apply_language_selection(
            &mut self.i18n,
            &mut self.command_palette,
            &mut self.message_list,
        );
    }

    fn startup_onboarding_active(&self) -> bool {
        self.startup_onboarding
            .active_in_surface(self.startup_surface_snapshot())
    }

    fn startup_surface_snapshot(&self) -> StartupSurfaceSnapshot {
        let quickstart_focus =
            if self.inline_skill_popup_active || self.command_palette.is_skills_mode() {
                StartupQuickstartFocus::Skills
            } else if matches!(self.focus, Focus::CommandPalette)
                && self.command_palette.is_commands_mode()
            {
                StartupQuickstartFocus::Commands
            } else if matches!(self.focus, Focus::MessageList) {
                StartupQuickstartFocus::Transcript
            } else {
                StartupQuickstartFocus::Chat
            };

        StartupSurfaceSnapshot {
            pending_turn: self.pending_turn,
            following_tail: self.message_list.is_following_tail(),
            has_conversation_messages: self
                .message_list
                .messages
                .iter()
                .any(|message| matches!(message.role.as_str(), "You" | "Assistant")),
            composer_empty: self.composer.is_empty(),
            command_palette_active: matches!(self.focus, Focus::CommandPalette),
            inline_skill_popup_active: self.inline_skill_popup_active,
            quickstart_focus,
        }
    }

    fn handle_startup_onboarding_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if !self.startup_onboarding_active() {
            return false;
        }
        let outcome = self.startup_onboarding.handle_key(key);
        if outcome.language_applied {
            self.apply_startup_language_selection();
        }
        if outcome.finished {
            self.finish_startup_onboarding();
        }
        if let Some(animation) = outcome.animation {
            let duration =
                if outcome.finished || matches!(animation, StartupEyeAnimation::Celebrate) {
                    320
                } else if matches!(animation, StartupEyeAnimation::Confirm(_)) {
                    180
                } else {
                    0
                };
            if duration > 0 {
                self.set_startup_eye_feedback(animation, Duration::from_millis(duration));
            }
        }
        outcome.handled
    }

    fn finish_startup_onboarding(&mut self) {
        self.startup_onboarding.finish(&mut self.message_list);
        let workspace_root = Path::new(self.cwd.as_str());
        self.command_palette
            .set_skills(detect_available_skills(Some(workspace_root)));
        self.focus = Focus::Composer;
        self.inline_skill_popup_active = false;
        self.composer_follow_up_intent = false;
    }

    fn absorb_first_turn_calibration_if_needed(&mut self, message: &str) -> bool {
        self.startup_onboarding
            .absorb_first_turn_calibration_if_needed(message, &mut self.message_list)
    }

    fn take_first_turn_calibration_addendum(&mut self) -> Option<String> {
        self.startup_onboarding
            .take_first_turn_calibration_addendum()
    }

    fn apply_palette_action(&mut self, action: CommandAction) -> Option<String> {
        match action {
            CommandAction::RunCommand(command) => {
                self.composer.clear();
                self.inline_skill_popup_active = false;
                self.focus = Focus::Composer;
                self.set_startup_eye_feedback(
                    StartupEyeAnimation::Confirm(StartupEyeFocus::DownCenter),
                    Duration::from_millis(420),
                );
                Some(command)
            }
            CommandAction::InsertText(text) => {
                if let Some(range) = current_skill_token_range(&self.composer) {
                    let replacement =
                        inline_skill_replacement_text(self.composer.text(), &range, text.as_str());
                    self.composer.replace_range(range, replacement.as_str());
                } else {
                    self.composer.set_input(text);
                }
                self.inline_skill_popup_active = false;
                self.focus = Focus::Composer;
                self.set_startup_eye_feedback(
                    StartupEyeAnimation::Confirm(StartupEyeFocus::DownLeft),
                    Duration::from_millis(420),
                );
                None
            }
            CommandAction::Close => {
                self.inline_skill_popup_active = false;
                self.focus = Focus::Composer;
                None
            }
        }
    }

    fn handle_mouse_event(&mut self, mouse_event: MouseEvent) -> Option<String> {
        if rect_contains_point(self.last_palette_area, mouse_event.column, mouse_event.row)
            && (matches!(self.focus, Focus::CommandPalette) || self.inline_skill_popup_active)
        {
            return self
                .command_palette
                .handle_mouse(mouse_event, self.last_palette_area)
                .and_then(|action| self.apply_palette_action(action));
        }

        if rect_contains_point(self.last_composer_area, mouse_event.column, mouse_event.row) {
            if matches!(mouse_event.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.focus = Focus::Composer;
                self.sync_inline_skill_popup();
            }
            return None;
        }

        if rect_contains_point(
            self.last_transcript_area,
            mouse_event.column,
            mouse_event.row,
        ) {
            if matches!(
                mouse_event.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Down(MouseButton::Right)
                    | MouseEventKind::Down(MouseButton::Middle)
                    | MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
            ) {
                self.focus = Focus::MessageList;
                self.sync_inline_skill_popup();
            }
            self.message_list.handle_mouse(mouse_event);
        }

        None
    }

    fn sync_inline_skill_popup(&mut self) {
        if !matches!(self.focus, Focus::Composer) {
            self.inline_skill_popup_active = false;
            return;
        }

        if self.command_palette.has_skills()
            && let Some(query) = current_skill_token_query(&self.composer)
        {
            self.command_palette.show_skills(query.as_str());
            self.inline_skill_popup_active = true;
        } else {
            self.inline_skill_popup_active = false;
        }
    }

    fn confirm_inline_skill_popup(&mut self) {
        if let Some(action) = self
            .command_palette
            .handle_key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))
        {
            let _ = self.apply_palette_action(action);
        } else {
            self.inline_skill_popup_active = false;
        }
    }

    fn handle_inline_skill_popup_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if !self.inline_skill_popup_active {
            return false;
        }

        if matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
        ) {
            let _ = self.command_palette.handle_key(key);
            return true;
        }

        if key.code == KeyCode::Esc {
            self.inline_skill_popup_active = false;
            return true;
        }

        if (key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::SHIFT))
            || key.code == KeyCode::Tab
        {
            self.confirm_inline_skill_popup();
            return true;
        }

        false
    }

    fn pending_lines_for(
        &mut self,
        width: u16,
        height: u16,
        composer_height: u16,
        palette_height: u16,
    ) -> Vec<Line<'static>> {
        if !self.pending_turn {
            self.pending_render_cache = None;
            return Vec::new();
        }

        let max_pending_height = pending_band_max_height(height, composer_height, palette_height);
        let Some(signature) = pending_render_signature_for_geometry(
            self,
            width,
            height,
            composer_height,
            palette_height,
        ) else {
            self.pending_render_cache = None;
            return Vec::new();
        };

        if let Some(cache) = self.pending_render_cache.as_ref()
            && cache.signature == signature
            && cache.max_pending_height == max_pending_height
        {
            return cache.lines.clone();
        }

        let max_pending_preview_lines = max_pending_height.saturating_sub(2).max(1) as usize;
        let live_lines = pending_live_lines(&self.live_lines, max_pending_preview_lines);
        let raw_pending_lines = build_pending_lines(
            self.turn_start,
            &live_lines,
            self.spinner_seed,
            self.i18n.spinner_verbs(),
            &self.pending_steers,
            &self.pending_queue,
            width,
        );
        let lines = compact_pending_lines_for_height(raw_pending_lines, max_pending_height);
        self.pending_render_cache = Some(PendingRenderCache {
            signature,
            max_pending_height,
            lines: lines.clone(),
        });
        lines
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
    let mut startup_release_task = Some(tokio::spawn(load_startup_release_lines(render_width)));
    let mut dirty = true;
    let mut last_resize_at: Option<std::time::Instant> = None;
    let mut pending_live_resize_rerender = false;

    loop {
        if let Some(task) = startup_release_task.as_ref()
            && task.is_finished()
            && let Some(task) = startup_release_task.take()
            && let Ok(Some(lines)) = task.await
        {
            app.message_list.add_rendered_lines(lines);
            dirty = true;
        }

        if maybe_finalize_pending_turn(terminal, &mut app, &runtime).await? {
            dirty = true;
        }

        if app.message_list.refresh_startup_animation() {
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

        if resize_live_rerender_ready(
            pending_live_resize_rerender,
            last_resize_at.map(|instant| instant.elapsed()),
        ) {
            if let Some(rerender) = app.live_rerender.as_ref() {
                rerender();
            }
            pending_live_resize_rerender = false;
            last_resize_at = None;
            dirty = true;
        }

        if dirty {
            terminal
                .draw(|f| app.render(f))
                .map_err(|e| format!("draw error: {}", e))?;
            dirty = false;
            if !pending_live_resize_rerender {
                last_resize_at = None;
            }
        }

        let poll_timeout = if pending_live_resize_rerender {
            Duration::from_millis(16)
        } else if app.pending_turn {
            Duration::from_millis(80)
        } else if app.message_list.startup_animation_active() {
            Duration::from_millis(70)
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

                    if app.handle_startup_onboarding_key(key) {
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
                                    refresh_command_palette_extension_commands(
                                        &mut app.command_palette,
                                        &runtime,
                                    );
                                    let query = if key.code == KeyCode::Char(':') {
                                        ":"
                                    } else {
                                        "/"
                                    };
                                    open_command_palette_with_query(&mut app, query);
                                } else if app.handle_inline_skill_popup_key(key) {
                                } else if should_route_composer_key_to_transcript(&app, key) {
                                    app.message_list.handle_key(key);
                                    app.focus = Focus::MessageList;
                                    app.sync_inline_skill_popup();
                                } else if key.code == KeyCode::Tab {
                                    if !app.composer.is_empty() {
                                        queue_pending_message(&mut app);
                                        app.inline_skill_popup_active = false;
                                    } else {
                                        app.focus = Focus::MessageList;
                                    }
                                } else if let Some(msg) = app.composer.handle_key(key) {
                                    pending_submission = Some(msg);
                                    app.sync_inline_skill_popup();
                                } else if !app.composer.is_empty() {
                                    app.composer_follow_up_intent = true;
                                    app.sync_inline_skill_popup();
                                } else {
                                    app.sync_inline_skill_popup();
                                }
                            }
                            Focus::MessageList => {
                                if matches!(key.code, KeyCode::Char('/') | KeyCode::Char(':'))
                                    && app.composer.is_empty()
                                {
                                    refresh_command_palette_extension_commands(
                                        &mut app.command_palette,
                                        &runtime,
                                    );
                                    let query = if key.code == KeyCode::Char(':') {
                                        ":"
                                    } else {
                                        "/"
                                    };
                                    open_command_palette_with_query(&mut app, query);
                                } else if should_focus_composer_for_transcript_key(key) {
                                    pending_submission =
                                        route_transcript_key_to_composer(&mut app, key);
                                } else {
                                    app.message_list.handle_key(key);
                                    if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                                        app.focus = Focus::Composer;
                                    }
                                }
                            }
                            Focus::CommandPalette => {
                                if let Some(action) = handle_command_palette_key(&mut app, key) {
                                    match action {
                                        CommandAction::RunCommand(command) => {
                                            pending_command = Some(command);
                                            app.composer.clear();
                                            app.composer_follow_up_intent = false;
                                            app.inline_skill_popup_active = false;
                                            app.focus = Focus::Composer;
                                        }
                                        CommandAction::InsertText(text) => {
                                            app.composer.set_input(text);
                                            app.inline_skill_popup_active = false;
                                            app.focus = Focus::Composer;
                                        }
                                        CommandAction::Close => {
                                            app.inline_skill_popup_active = false;
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
                                    app.inline_skill_popup_active = false;
                                }
                            } else if matches!(key.code, KeyCode::Char('/') | KeyCode::Char(':'))
                                && app.composer.is_empty()
                            {
                                refresh_command_palette_extension_commands(
                                    &mut app.command_palette,
                                    &runtime,
                                );
                                let query = if key.code == KeyCode::Char(':') {
                                    ":"
                                } else {
                                    "/"
                                };
                                open_command_palette_with_query(&mut app, query);
                            } else if app.handle_inline_skill_popup_key(key) {
                            } else if should_route_composer_key_to_transcript(&app, key) {
                                app.message_list.handle_key(key);
                                app.focus = Focus::MessageList;
                                app.sync_inline_skill_popup();
                            } else if key.code == KeyCode::Tab {
                                app.focus = Focus::MessageList;
                            } else if let Some(msg) = app.composer.handle_key(key) {
                                submitted_message = Some(msg);
                                app.sync_inline_skill_popup();
                            } else {
                                app.sync_inline_skill_popup();
                            }
                        }
                        Focus::CommandPalette => {
                            if let Some(action) = handle_command_palette_key(&mut app, key) {
                                match action {
                                    CommandAction::RunCommand(command) => {
                                        command_to_run = Some(command);
                                        app.composer.clear();
                                        app.composer_follow_up_intent = false;
                                        app.inline_skill_popup_active = false;
                                        app.focus = Focus::Composer;
                                    }
                                    CommandAction::InsertText(text) => {
                                        app.composer.set_input(text);
                                        app.inline_skill_popup_active = false;
                                        app.focus = Focus::Composer;
                                    }
                                    CommandAction::Close => {
                                        app.inline_skill_popup_active = false;
                                        app.focus = Focus::Composer;
                                    }
                                }
                            }
                        }
                        Focus::MessageList => {
                            if key.code == KeyCode::Tab {
                                app.focus = Focus::Composer;
                            } else if should_focus_composer_for_transcript_key(key) {
                                submitted_message = route_transcript_key_to_composer(&mut app, key);
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
                            refresh_command_palette_extension_commands(
                                &mut app.command_palette,
                                &runtime,
                            );
                            open_command_palette_with_query(&mut app, &msg);
                            continue;
                        }

                        let calibration_addendum = app.take_first_turn_calibration_addendum();
                        if calibration_addendum.is_none()
                            && app.absorb_first_turn_calibration_if_needed(&msg)
                        {
                            dirty = true;
                            continue;
                        }

                        if submitted_message_is_follow_up(&app, &msg) {
                            start_turn(
                                terminal,
                                &mut app,
                                &runtime,
                                msg,
                                false,
                                calibration_addendum,
                            )
                            .await?;
                        } else {
                            submit_user_turn(
                                terminal,
                                &mut app,
                                &runtime,
                                msg,
                                calibration_addendum,
                            )
                            .await?;
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
                    if let Some(command) = app.handle_mouse_event(mouse_event) {
                        if command == "/exit" {
                            break;
                        }
                        run_surface_command(terminal, &mut app, &runtime, &options, &command)
                            .await?;
                    }
                    dirty = true;
                }
                Event::Resize(width, height) => {
                    let new_size = ratatui::layout::Size::new(width, height);
                    if new_size.width == last_known_size.width
                        && new_size.height == last_known_size.height
                    {
                        continue;
                    }
                    let width_changed = last_known_size.width != new_size.width;
                    let layout_changed = resize_reflow_required(
                        last_known_size.width,
                        last_known_size.height,
                        new_size.width,
                        new_size.height,
                    );
                    if layout_changed {
                        last_resize_at = Some(std::time::Instant::now());
                    }
                    last_known_size = new_size;
                    app.last_render_width = new_size.width;
                    app.last_render_height = new_size.height;
                    app.live_render_width
                        .store(new_size.width.max(1) as usize, Ordering::Relaxed);
                    if width_changed && app.live_rerender.is_some() {
                        pending_live_resize_rerender = true;
                    }
                    dirty = true;
                }
                Event::Paste(text) => {
                    paste_into_composer(&mut app, text.as_str());
                    dirty = true;
                }
                Event::FocusGained | Event::FocusLost => {}
            }
        }
    }
    Ok(())
}

fn paste_into_composer(app: &mut App, text: &str) {
    if text.is_empty() {
        return;
    }
    app.composer.insert_paste(text);
    app.focus = Focus::Composer;
    if app.pending_turn && !app.composer.is_empty() {
        app.composer_follow_up_intent = true;
    }
    app.sync_inline_skill_popup();
}

async fn run_surface_command<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    options: &CliChatOptions,
    input: &str,
) -> CliResult<()> {
    let trimmed = input.trim();
    let (command, args) = split_surface_command(trimmed);
    let width = current_render_width(terminal)?;

    match command {
        "/clear" => {
            app.message_list.clear_transcript();
            app.focus = Focus::Composer;
            Ok(())
        }
        "/new" => {
            app.message_list.clear_transcript();
            app.message_list
                .add_rendered_lines(render_new_conversation_lines_with_width(width));
            app.focus = Focus::Composer;
            Ok(())
        }
        "/copy" => {
            let copy_result = copy_command_text(app, args)
                .and_then(|text| copy_to_system_clipboard(text.as_str()).map(|()| text));
            app.message_list
                .add_rendered_lines(render_copy_command_lines_with_width(copy_result, width));
            app.focus = Focus::Composer;
            Ok(())
        }
        "/diff" => {
            let cwd = current_working_directory(runtime);
            let lines = render_git_diff_command_lines_with_width(cwd.as_path(), width);
            app.message_list.add_rendered_lines(lines);
            app.focus = Focus::Composer;
            Ok(())
        }
        "/export" | "/share" => {
            let cwd = current_working_directory(runtime);
            let markdown = app.message_list.export_markdown();
            let result = write_transcript_export(
                cwd.as_path(),
                runtime.session_id.as_str(),
                command.trim_start_matches('/'),
                markdown.as_str(),
            );
            app.message_list
                .add_rendered_lines(render_export_command_lines_with_width(
                    command, result, width,
                ));
            app.focus = Focus::Composer;
            Ok(())
        }
        "/import" => {
            if args.trim().is_empty() {
                let lines = build_command_lines(runtime, options, input, width).await?;
                app.message_list.add_rendered_lines(lines);
            } else {
                let cwd = current_working_directory(runtime);
                let result = import_context_into_composer(app, cwd.as_path(), args);
                app.message_list
                    .add_rendered_lines(render_import_command_lines_with_width(result, width));
            }
            app.focus = Focus::Composer;
            Ok(())
        }
        "/simplify" => {
            let result = stage_simplify_prompt(app, args);
            app.message_list
                .add_rendered_lines(render_prompt_staging_lines_with_width(
                    "simplify", result, width,
                ));
            app.focus = Focus::Composer;
            Ok(())
        }
        "/plan" => {
            let result = stage_plan_prompt(app, args);
            app.message_list
                .add_rendered_lines(render_prompt_staging_lines_with_width(
                    "plan", result, width,
                ));
            app.focus = Focus::Composer;
            Ok(())
        }
        "/title" | "/rename" => {
            if !args.trim().is_empty() {
                app.title = Some(args.trim().to_owned());
            }
            let lines = render_title_command_lines_with_width(command, args, width);
            app.message_list.add_rendered_lines(lines);
            app.focus = Focus::Composer;
            Ok(())
        }
        "/feedback" => {
            let result = stage_feedback_prompt(app, args);
            app.message_list
                .add_rendered_lines(render_prompt_staging_lines_with_width(
                    "feedback", result, width,
                ));
            app.focus = Focus::Composer;
            Ok(())
        }
        _ => {
            if let Some(outcome) =
                crate::tools::extension_runtime_tools::maybe_execute_extension_runtime_command(
                    input,
                    runtime.session_id.as_str(),
                    &runtime.config,
                )?
            {
                app.message_list.add_rendered_lines(
                    render_extension_command_outcome_lines_with_width(&outcome, width),
                );
            } else {
                let lines = build_command_lines(runtime, options, input, width).await?;
                app.message_list.add_rendered_lines(lines);
            }
            app.focus = Focus::Composer;
            Ok(())
        }
    }
}

fn split_surface_command(input: &str) -> (&str, &str) {
    let trimmed = input.trim();
    if let Some((command, rest)) = trimmed.split_once(char::is_whitespace) {
        (command, rest.trim())
    } else {
        (trimmed, "")
    }
}

fn current_working_directory(runtime: &CliTurnRuntime) -> PathBuf {
    runtime
        .effective_working_directory
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn render_new_conversation_lines_with_width(width: usize) -> Vec<String> {
    let message_spec = TuiMessageSpec {
        role: "new".to_owned(),
        caption: Some("fresh conversation".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone: TuiCalloutTone::Info,
            title: Some("ready".to_owned()),
            lines: vec![
                "The visible transcript has been cleared and the composer is ready for the next turn."
                    .to_owned(),
            ],
        }],
        footer_lines: vec!["Type immediately; no extra focus step is needed.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn copy_command_text(app: &App, args: &str) -> Result<String, String> {
    if !args.trim().is_empty() {
        return Ok(args.trim().to_owned());
    }
    app.message_list
        .latest_copy_text()
        .ok_or_else(|| "nothing copyable yet".to_owned())
}

fn copy_to_system_clipboard(text: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    };

    let mut last_error = "no clipboard command attempted".to_owned();
    for (program, args) in candidates {
        let spawn_result = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = spawn_result else {
            last_error = format!("{program} unavailable");
            continue;
        };
        if let Some(stdin) = child.stdin.as_mut()
            && let Err(error) = stdin.write_all(text.as_bytes())
        {
            last_error = format!("{program} write failed: {error}");
            let _ = child.kill();
            continue;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("{program} wait failed: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        last_error = if stderr.is_empty() {
            format!("{program} exited with {}", output.status)
        } else {
            format!("{program}: {stderr}")
        };
    }
    Err(last_error)
}

fn render_copy_command_lines_with_width(
    result: Result<String, String>,
    width: usize,
) -> Vec<String> {
    let (tone, title, lines) = match result {
        Ok(text) => {
            let char_count = text.chars().count();
            (
                TuiCalloutTone::Info,
                "copied".to_owned(),
                vec![format!(
                    "Copied {char_count} character(s) to the system clipboard."
                )],
            )
        }
        Err(error) => (
            TuiCalloutTone::Warning,
            "copy unavailable".to_owned(),
            vec![error],
        ),
    };
    let message_spec = TuiMessageSpec {
        role: "copy".to_owned(),
        caption: Some("clipboard".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone,
            title: Some(title),
            lines,
        }],
        footer_lines: vec![
            "/copy copies the latest reply, or /copy <text> copies explicit text.".to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn run_git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("git failed to start: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if output.status.success() {
        Ok(stdout)
    } else if stderr.is_empty() {
        Err(format!("git exited with {}", output.status))
    } else {
        Err(stderr)
    }
}

fn render_git_diff_command_lines_with_width(cwd: &Path, width: usize) -> Vec<String> {
    let status = run_git_capture(cwd, &["status", "--short"]);
    let stat = run_git_capture(cwd, &["diff", "--stat"]);
    let shortstat = run_git_capture(cwd, &["diff", "--shortstat"]);

    let mut sections = Vec::new();
    match (status, stat, shortstat) {
        (Ok(status), Ok(stat), Ok(shortstat)) => {
            let status_lines = if status.trim().is_empty() {
                vec!["working tree clean".to_owned()]
            } else {
                status.lines().map(ToOwned::to_owned).collect()
            };
            sections.push(TuiSectionSpec::Preformatted {
                title: Some("status".to_owned()),
                language: None,
                lines: status_lines,
            });
            if !stat.trim().is_empty() {
                sections.push(TuiSectionSpec::Preformatted {
                    title: Some("diff stat".to_owned()),
                    language: None,
                    lines: stat.lines().map(ToOwned::to_owned).collect(),
                });
            }
            if !shortstat.trim().is_empty() {
                sections.push(TuiSectionSpec::Narrative {
                    title: Some("summary".to_owned()),
                    lines: vec![shortstat],
                });
            }
        }
        (status, stat, shortstat) => {
            let errors = [status.err(), stat.err(), shortstat.err()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            sections.push(TuiSectionSpec::Callout {
                tone: TuiCalloutTone::Warning,
                title: Some("git diff unavailable".to_owned()),
                lines: if errors.is_empty() {
                    vec!["git did not return diff information".to_owned()]
                } else {
                    errors
                },
            });
        }
    }

    let message_spec = TuiMessageSpec {
        role: "diff".to_owned(),
        caption: Some("working tree".to_owned()),
        sections,
        footer_lines: vec![format!("cwd: {}", cwd.display())],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn safe_file_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(64)
        .collect::<String>()
}

fn write_transcript_export(
    cwd: &Path,
    session_id: &str,
    label: &str,
    markdown: &str,
) -> Result<PathBuf, String> {
    if markdown.trim().is_empty() {
        return Err("transcript is empty".to_owned());
    }
    let export_dir = cwd.join(".loong").join("exports");
    fs::create_dir_all(export_dir.as_path())
        .map_err(|error| format!("failed to create export directory: {error}"))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_secs();
    let session = safe_file_component(session_id);
    let label = safe_file_component(label);
    let file_name = format!("{label}-{session}-{timestamp}.md");
    let path = export_dir.join(file_name);
    fs::write(path.as_path(), markdown)
        .map_err(|error| format!("failed to write export: {error}"))?;
    Ok(path)
}

fn render_export_command_lines_with_width(
    command: &str,
    result: Result<PathBuf, String>,
    width: usize,
) -> Vec<String> {
    let (tone, title, lines) = match result {
        Ok(path) => (
            TuiCalloutTone::Info,
            "written".to_owned(),
            vec![format!("{} wrote {}", command, path.display())],
        ),
        Err(error) => (
            TuiCalloutTone::Warning,
            "not written".to_owned(),
            vec![error],
        ),
    };
    let message_spec = TuiMessageSpec {
        role: command.trim_start_matches('/').to_owned(),
        caption: Some("transcript artifact".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone,
            title: Some(title),
            lines,
        }],
        footer_lines: vec![
            "Artifacts stay local until you explicitly move or publish them.".to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn resolve_import_path(cwd: &Path, input: &str) -> PathBuf {
    let trimmed = input.trim().trim_matches('"').trim_matches('\'');
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn import_context_into_composer(app: &mut App, cwd: &Path, args: &str) -> Result<PathBuf, String> {
    let path = resolve_import_path(cwd, args);
    let content = fs::read_to_string(path.as_path())
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let clipped = if content.chars().count() > 20_000 {
        let prefix = content.chars().take(20_000).collect::<String>();
        format!("{prefix}\n\n[import truncated to first 20000 characters]")
    } else {
        content
    };
    app.composer.set_input(format!(
        "Use this imported context from {}:\n\n{}",
        path.display(),
        clipped
    ));
    Ok(path)
}

fn render_import_command_lines_with_width(
    result: Result<PathBuf, String>,
    width: usize,
) -> Vec<String> {
    let (tone, title, lines) = match result {
        Ok(path) => (
            TuiCalloutTone::Info,
            "staged".to_owned(),
            vec![format!(
                "Imported {} into the composer draft.",
                path.display()
            )],
        ),
        Err(error) => (
            TuiCalloutTone::Warning,
            "import failed".to_owned(),
            vec![error],
        ),
    };
    let message_spec = TuiMessageSpec {
        role: "import".to_owned(),
        caption: Some("composer context".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone,
            title: Some(title),
            lines,
        }],
        footer_lines: vec![
            "Review the staged draft before sending if the file is large.".to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn latest_text_or_args(app: &App, args: &str) -> Result<String, String> {
    if !args.trim().is_empty() {
        return Ok(args.trim().to_owned());
    }
    app.message_list
        .latest_copy_text()
        .ok_or_else(|| "no previous content to use".to_owned())
}

fn stage_simplify_prompt(app: &mut App, args: &str) -> Result<(), String> {
    let source = latest_text_or_args(app, args)?;
    app.composer.set_input(format!(
        "Please simplify and clarify the following content without losing important details:\n\n{source}"
    ));
    Ok(())
}

fn stage_plan_prompt(app: &mut App, args: &str) -> Result<(), String> {
    let subject = if args.trim().is_empty() {
        "the current task".to_owned()
    } else {
        args.trim().to_owned()
    };
    app.composer.set_input(format!(
        "Create a concise implementation plan for {subject}. Include risks, verification, and the smallest safe sequence."
    ));
    Ok(())
}

fn stage_feedback_prompt(app: &mut App, args: &str) -> Result<(), String> {
    let body = if args.trim().is_empty() {
        "Feedback: ".to_owned()
    } else {
        format!("Feedback: {}", args.trim())
    };
    app.composer.set_input(body);
    Ok(())
}

fn render_prompt_staging_lines_with_width(
    role: &str,
    result: Result<(), String>,
    width: usize,
) -> Vec<String> {
    let (tone, title, lines) = match result {
        Ok(()) => (
            TuiCalloutTone::Info,
            "draft staged".to_owned(),
            vec!["The composer has been populated; edit or press Enter to send.".to_owned()],
        ),
        Err(error) => (
            TuiCalloutTone::Warning,
            "not staged".to_owned(),
            vec![error],
        ),
    };
    let message_spec = TuiMessageSpec {
        role: role.to_owned(),
        caption: Some("composer draft".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone,
            title: Some(title),
            lines,
        }],
        footer_lines: vec!["Typing continues in the composer immediately.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_title_command_lines_with_width(command: &str, args: &str, width: usize) -> Vec<String> {
    let lines = if args.trim().is_empty() {
        vec![format!("Usage: {command} <title>")]
    } else {
        vec![format!(
            "Title noted for this local chat surface: {}",
            args.trim()
        )]
    };
    let message_spec = TuiMessageSpec {
        role: command.trim_start_matches('/').to_owned(),
        caption: Some("local title".to_owned()),
        sections: vec![TuiSectionSpec::Callout {
            tone: TuiCalloutTone::Info,
            title: Some("title".to_owned()),
            lines,
        }],
        footer_lines: vec!["The title is reflected in the footer for this TUI session.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

async fn submit_user_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    input: String,
    system_prompt_addendum: Option<String>,
) -> CliResult<()> {
    start_turn(terminal, app, runtime, input, true, system_prompt_addendum).await
}

async fn start_turn<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    runtime: &CliTurnRuntime,
    input: String,
    echo_user_message: bool,
    system_prompt_addendum: Option<String>,
) -> CliResult<()> {
    let width = current_render_width(terminal)?;
    let runtime = reload_runtime_for_surface_turn(runtime, system_prompt_addendum);
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
    app.pending_task = Some(spawn_pending_turn(runtime, input, observer));
    Ok(())
}

fn reload_runtime_for_surface_turn(
    runtime: &CliTurnRuntime,
    system_prompt_addendum: Option<String>,
) -> CliTurnRuntime {
    let Some(path) = runtime.resolved_path.to_str() else {
        let mut passthrough = runtime.clone();
        apply_transient_system_prompt_addendum(&mut passthrough, system_prompt_addendum);
        return passthrough;
    };
    let Ok((_, loaded)) = crate::config::load(Some(path)) else {
        let mut passthrough = runtime.clone();
        apply_transient_system_prompt_addendum(&mut passthrough, system_prompt_addendum);
        return passthrough;
    };
    let mut refreshed = runtime.clone();
    refreshed.config = loaded;
    apply_transient_system_prompt_addendum(&mut refreshed, system_prompt_addendum);
    refreshed
}

fn apply_transient_system_prompt_addendum(
    runtime: &mut CliTurnRuntime,
    system_prompt_addendum: Option<String>,
) {
    let Some(addendum) = system_prompt_addendum.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    runtime.config.cli.system_prompt_addendum = Some(
        runtime
            .config
            .cli
            .system_prompt_addendum
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|existing| format!("{existing}\n\n{addendum}"))
            .unwrap_or(addendum),
    );
    runtime.config.cli.refresh_native_system_prompt();
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
    )
}

fn should_focus_composer_for_transcript_key(key: crossterm::event::KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return false;
    }

    matches!(
        key.code,
        KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Enter
            | KeyCode::Left
            | KeyCode::Right
    )
}

fn route_transcript_key_to_composer(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Option<String> {
    app.message_list.restore_tail();
    app.focus = Focus::Composer;
    let submitted = app.composer.handle_key(key);
    app.sync_inline_skill_popup();
    submitted
}

fn should_route_composer_key_to_transcript(app: &App, key: crossterm::event::KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
    ) || (app.composer.is_empty() && is_transcript_navigation_key(key))
}

fn should_use_history_browse_layout(app: &App, palette_visible: bool) -> bool {
    !app.pending_turn
        && !palette_visible
        && app.composer.is_empty()
        && !app.message_list.is_following_tail()
}

fn open_command_palette_with_query(app: &mut App, query: &str) {
    app.command_palette.show_commands(query);
    app.composer.set_input(query.to_owned());
    app.inline_skill_popup_active = false;
    app.focus = Focus::CommandPalette;
    app.set_startup_eye_feedback(
        StartupEyeAnimation::Thinking(StartupEyeFocus::DownLeft),
        Duration::from_millis(320),
    );
}

fn sync_composer_to_command_palette(app: &mut App) {
    if let Some(text) = app.command_palette.composer_preview_text() {
        app.composer.set_input(text);
    }
}

fn handle_command_palette_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
) -> Option<CommandAction> {
    if key.code == KeyCode::Backspace
        && app.command_palette.is_commands_mode()
        && app.command_palette.query_is_empty()
    {
        app.composer.clear();
        app.inline_skill_popup_active = false;
        app.focus = Focus::Composer;
        return None;
    }

    let before_progress = app.command_palette.selection_progress();
    let action = app.command_palette.handle_key(key);
    if action.is_none() && app.command_palette.is_commands_mode() {
        sync_composer_to_command_palette(app);
    }
    let after_progress = app.command_palette.selection_progress();
    if matches!(
        key.code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Char(_)
    ) && after_progress != before_progress
    {
        let focus = after_progress
            .map(|(selected, total)| startup_palette_eye_focus(selected, total))
            .unwrap_or_else(|| {
                if app.command_palette.is_skills_mode() {
                    StartupEyeFocus::DownLeft
                } else {
                    StartupEyeFocus::DownRight
                }
            });
        let animation = if app.command_palette.is_commands_mode() {
            StartupEyeAnimation::Thinking(focus)
        } else {
            StartupEyeAnimation::Focus(focus)
        };
        app.set_startup_eye_feedback(animation, Duration::from_millis(360));
    }
    action
}

fn submitted_message_is_follow_up(app: &App, msg: &str) -> bool {
    app.pending_turn
        && app.composer_follow_up_intent
        && !msg.starts_with('/')
        && !msg.starts_with(':')
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

fn rect_contains_point(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn current_skill_token_query(composer: &Composer) -> Option<String> {
    let range = current_skill_token_range(composer)?;
    composer.text()[range]
        .strip_prefix('$')
        .map(|query| query.to_owned())
}

fn current_skill_token_range(composer: &Composer) -> Option<std::ops::Range<usize>> {
    let text = composer.text();
    let cursor = composer.cursor().min(text.len());
    if text.is_empty() {
        return None;
    }

    let before_cursor = &text[..cursor];
    let token_start = before_cursor
        .char_indices()
        .rfind(|(_, ch)| ch.is_whitespace())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    let after_cursor = &text[cursor..];
    let token_end = after_cursor
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, _)| cursor + idx)
        .unwrap_or(text.len());
    if token_start >= token_end {
        return None;
    }

    let token = &text[token_start..token_end];
    token.starts_with('$').then_some(token_start..token_end)
}

fn inline_skill_replacement_text(
    text: &str,
    range: &std::ops::Range<usize>,
    replacement: &str,
) -> String {
    let should_trim_trailing_space = replacement.ends_with(' ')
        && text
            .get(range.end..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(char::is_whitespace);

    if should_trim_trailing_space {
        replacement.trim_end_matches(' ').to_owned()
    } else {
        replacement.to_owned()
    }
}

fn build_status_footer_line(cwd: &str, model: &str, width: u16) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::from(String::new());
    }

    if width <= 24 {
        return single_footer_span(model, width, Style::default().fg(SURFACE_GRAY));
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
        Span::styled(cwd_text, Style::default().fg(SURFACE_GRAY)),
        Span::raw(" ".repeat(spacer_width)),
        Span::styled(model_text, Style::default().fg(SURFACE_GRAY)),
    ])
}

fn single_footer_span(text: &str, width: usize, style: Style) -> Line<'static> {
    let mut rendered = truncate_right_for_width(text, width);
    let rendered_width = display_columns(&rendered);
    if rendered_width < width {
        rendered.push_str(&" ".repeat(width - rendered_width));
    }
    Line::from(vec![Span::styled(rendered, style)])
}

fn footer_content_area(area: Rect) -> Rect {
    if area.width <= FOOTER_HORIZONTAL_INDENT {
        return area;
    }

    Rect {
        x: area.x.saturating_add(FOOTER_HORIZONTAL_INDENT),
        y: area.y,
        width: area.width.saturating_sub(FOOTER_HORIZONTAL_INDENT),
        height: area.height,
    }
}

fn build_queue_footer_line(i18n: &I18nService, queued: usize, width: u16) -> Line<'static> {
    let max_width = width as usize;
    if max_width == 0 {
        return Line::from(String::new());
    }
    if max_width <= 18 {
        let text = if queued > 0 {
            format!("queued ×{queued}")
        } else {
            i18n.text(SurfaceCopy::FooterQueueShort).to_owned()
        };
        return single_footer_span(
            text.as_str(),
            max_width,
            Style::default().fg(SURFACE_ACCENT),
        );
    }

    let hint = i18n.text(SurfaceCopy::FooterQueueHint).to_owned();
    let short_hint = i18n.text(SurfaceCopy::FooterQueueShort).to_owned();
    let suffix = if queued > 0 {
        format!(" · queued ×{queued}")
    } else {
        String::new()
    };
    let total_width = display_columns(&hint) + display_columns(&suffix);
    if total_width <= max_width {
        let mut spans = vec![Span::styled(hint, Style::default().fg(SURFACE_ACCENT))];
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(SURFACE_GRAY)));
        }
        return Line::from(spans);
    }

    let short_total_width = display_columns(&short_hint) + display_columns(&suffix);
    if short_total_width <= max_width {
        let mut spans = vec![Span::styled(
            short_hint,
            Style::default().fg(SURFACE_ACCENT),
        )];
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, Style::default().fg(SURFACE_GRAY)));
        }
        return Line::from(spans);
    }

    if display_columns(&short_hint) >= max_width {
        return Line::from(vec![Span::styled(
            truncate_right_for_width(&short_hint, max_width),
            Style::default().fg(SURFACE_ACCENT),
        )]);
    }

    let remaining = max_width.saturating_sub(display_columns(&short_hint));
    Line::from(vec![
        Span::styled(short_hint, Style::default().fg(SURFACE_ACCENT)),
        Span::styled(
            truncate_right_for_width(&suffix, remaining),
            Style::default().fg(SURFACE_GRAY),
        ),
    ])
}

fn build_restore_footer_line(i18n: &I18nService, queued: usize, width: u16) -> Line<'static> {
    let max_width = width as usize;
    if max_width == 0 {
        return Line::from(String::new());
    }
    if max_width <= 18 {
        return single_footer_span(
            format!("restore ×{queued}").as_str(),
            max_width,
            Style::default().fg(SURFACE_GRAY),
        );
    }

    let full_text = format!(
        "{} {} · queued ×{}",
        queue_restore_shortcut_label(),
        i18n.text(SurfaceCopy::FooterRestoreQueued),
        queued
    );
    let short_text = format!(
        "{} {} · ×{}",
        queue_restore_shortcut_label(),
        i18n.text(SurfaceCopy::FooterRestoreShort),
        queued
    );
    let selected = if display_columns(&full_text) <= width as usize {
        full_text
    } else {
        short_text
    };
    Line::from(vec![Span::styled(
        truncate_right_for_width(&selected, max_width),
        Style::default().fg(SURFACE_GRAY),
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
            let extension_commands =
                crate::tools::extension_runtime_tools::list_extension_runtime_commands(
                    &runtime.config,
                )
                .unwrap_or_default();
            Ok(
                render_chat_surface_help_lines_with_width_and_extension_commands(
                    width,
                    &extension_commands,
                ),
            )
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
                    runtime.conversation_binding(),
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
                    runtime.conversation_binding(),
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
        "/model" => Ok(render_model_command_lines_with_width(runtime, width)),
        "/permissions" => Ok(render_permissions_command_lines_with_width(width)),
        "/experimental" => Ok(render_experimental_command_lines_with_width(width)),
        "/themes" => Ok(render_themes_command_lines_with_width(width)),
        "/cwd" => Ok(render_cwd_command_lines_with_width(runtime, width)),
        "/language" => Ok(render_language_command_lines_with_width(width)),
        "/mcp" => Ok(render_mcp_command_lines_with_width(runtime, width)),
        "/skills" => Ok(render_skills_command_lines_with_width(runtime, width)),
        "/usage" => {
            let extension_commands =
                crate::tools::extension_runtime_tools::list_extension_runtime_commands(
                    &runtime.config,
                )
                .unwrap_or_default();
            Ok(
                render_slash_command_usage_lines_with_width_and_extension_commands(
                    width,
                    &extension_commands,
                ),
            )
        }
        "/fast_lane_summary" => {
            #[cfg(feature = "memory-sqlite")]
            {
                let summary = crate::conversation::load_fast_lane_tool_batch_event_summary(
                    &runtime.session_id,
                    runtime.config.memory.sliding_window,
                    runtime.conversation_binding(),
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
                    runtime.conversation_binding(),
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
                        runtime.conversation_binding(),
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
                        runtime.conversation_binding(),
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
        "/subagents" | "/workers" => {
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
        "/missions" | "/mission" => {
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
        _ => {
            if let Some(spec) = find_slash_command_spec(trimmed) {
                Ok(render_slash_command_detail_lines_with_width(spec, width))
            } else {
                let extension_commands =
                    crate::tools::extension_runtime_tools::list_extension_runtime_commands(
                        &runtime.config,
                    )
                    .unwrap_or_default();
                Ok(
                    render_slash_command_usage_lines_with_width_and_extension_commands(
                        width,
                        &extension_commands,
                    ),
                )
            }
        }
    }
}

fn refresh_command_palette_extension_commands(
    command_palette: &mut CommandPalette,
    runtime: &CliTurnRuntime,
) {
    let extension_commands =
        crate::tools::extension_runtime_tools::list_extension_runtime_commands(&runtime.config)
            .unwrap_or_default();
    command_palette.set_extension_commands(&extension_commands);
}

#[allow(dead_code)]
fn render_slash_command_usage_lines_with_width(width: usize) -> Vec<String> {
    render_slash_command_usage_lines_with_width_and_extension_commands(width, &[])
}

fn render_slash_command_usage_lines_with_width_and_extension_commands(
    width: usize,
    extension_commands: &[(String, String)],
) -> Vec<String> {
    let command_items = slash_command_specs()
        .iter()
        .map(|spec| TuiKeyValueSpec::Plain {
            key: spec.command.to_owned(),
            value: slash_command_help_value(spec),
        })
        .collect::<Vec<_>>();
    let extension_command_section =
        (!extension_commands.is_empty()).then(|| TuiSectionSpec::KeyValues {
            title: Some("extension commands".to_owned()),
            items: extension_commands
                .iter()
                .map(|(command, description)| TuiKeyValueSpec::Plain {
                    key: command.clone(),
                    value: description.clone(),
                })
                .collect(),
        });

    let message_spec = TuiMessageSpec {
        role: "usage".to_owned(),
        caption: Some("slash commands".to_owned()),
        sections: {
            let mut sections = vec![
                TuiSectionSpec::KeyValues {
                    title: Some("commands".to_owned()),
                    items: command_items,
                },
                TuiSectionSpec::Narrative {
                    title: Some("navigation".to_owned()),
                    lines: {
                        let mut lines = vec![
                        "Open this deck with / or : from an empty composer.".to_owned(),
                        "Every command stays visible in the same product order so muscle memory keeps working across releases."
                            .to_owned(),
                        ];
                        if !extension_commands.is_empty() {
                            lines.push(
                                "Runtime extension commands join this deck automatically when the palette reopens."
                                    .to_owned(),
                            );
                        }
                        lines
                    },
                },
            ];
            if let Some(extension_command_section) = extension_command_section {
                sections.insert(1, extension_command_section);
            }
            sections
        },
        footer_lines: vec![
            "Enter runs the command or opens its detail card without permission ceremony."
                .to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_extension_command_outcome_lines_with_width(
    outcome: &crate::tools::extension_runtime_tools::ExtensionRuntimeCommandOutcome,
    width: usize,
) -> Vec<String> {
    if let Some(ui_action) = outcome.ui_action.as_ref() {
        let message_spec = match ui_action {
            crate::tools::extension_runtime_tools::ExtensionRuntimeUiAction::Notify {
                tone,
                title,
                lines,
                footer_lines,
            } => TuiMessageSpec {
                role: "extension".to_owned(),
                caption: Some("ui".to_owned()),
                sections: vec![TuiSectionSpec::Callout {
                    tone: match tone {
                        crate::tools::extension_runtime_tools::ExtensionRuntimeUiTone::Info => {
                            TuiCalloutTone::Info
                        }
                        crate::tools::extension_runtime_tools::ExtensionRuntimeUiTone::Success => {
                            TuiCalloutTone::Success
                        }
                        crate::tools::extension_runtime_tools::ExtensionRuntimeUiTone::Warning => {
                            TuiCalloutTone::Warning
                        }
                        crate::tools::extension_runtime_tools::ExtensionRuntimeUiTone::Error => {
                            TuiCalloutTone::Warning
                        }
                    },
                    title: title.clone(),
                    lines: lines.clone(),
                }],
                footer_lines: footer_lines.clone(),
            },
            crate::tools::extension_runtime_tools::ExtensionRuntimeUiAction::Overlay {
                title,
                lines,
                footer_lines,
            } => TuiMessageSpec {
                role: "extension".to_owned(),
                caption: Some("panel".to_owned()),
                sections: vec![TuiSectionSpec::Narrative {
                    title: Some(title.clone()),
                    lines: lines.clone(),
                }],
                footer_lines: footer_lines.clone(),
            },
        };
        return super::super::render_cli_chat_message_spec_with_width(&message_spec, width);
    }

    let text =
        crate::tools::extension_runtime_tools::render_extension_command_outcome_text(outcome);
    super::super::render_cli_chat_assistant_lines_with_width(&text, width)
}

fn render_slash_command_detail_lines_with_width(
    spec: &super::command_palette::SlashCommandSpec,
    width: usize,
) -> Vec<String> {
    let alias_lines = (!spec.aliases.is_empty()).then(|| {
        spec.aliases
            .iter()
            .map(|alias| format!("alias: {alias}"))
            .collect::<Vec<_>>()
    });
    let message_spec = TuiMessageSpec {
        role: "command".to_owned(),
        caption: Some(spec.command.trim_start_matches('/').to_owned()),
        sections: {
            let mut sections = vec![TuiSectionSpec::Callout {
                tone: TuiCalloutTone::Info,
                title: Some("enabled".to_owned()),
                lines: vec![format!(
                    "{} is available in the command deck and keeps a stable slot in the local TUI.",
                    spec.command
                )],
            }];
            if let Some(alias_lines) = alias_lines {
                sections.push(TuiSectionSpec::Narrative {
                    title: Some("aliases".to_owned()),
                    lines: alias_lines,
                });
            }
            sections.push(TuiSectionSpec::Narrative {
                title: Some("intent".to_owned()),
                lines: vec![spec.description.to_owned()],
            });
            sections
        },
        footer_lines: vec!["Use /usage to see the complete command deck.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn slash_command_help_value(spec: &super::command_palette::SlashCommandSpec) -> String {
    if spec.aliases.is_empty() {
        spec.description.to_owned()
    } else {
        format!(
            "{} (aliases: {})",
            spec.description,
            spec.aliases.join(", ")
        )
    }
}

fn render_model_command_lines_with_width(runtime: &CliTurnRuntime, width: usize) -> Vec<String> {
    let provider = &runtime.config.provider;
    let active_profile = runtime
        .config
        .active_provider_id()
        .unwrap_or("legacy provider");
    let reasoning_effort = provider
        .reasoning_effort
        .map(|effort| format!("{effort:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "default".to_owned());

    let message_spec = TuiMessageSpec {
        role: "model".to_owned(),
        caption: Some("active model".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some("provider".to_owned()),
            items: vec![
                TuiKeyValueSpec::Plain {
                    key: "profile".to_owned(),
                    value: active_profile.to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "provider".to_owned(),
                    value: provider.kind.display_name().to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "model".to_owned(),
                    value: provider.model.clone(),
                },
                TuiKeyValueSpec::Plain {
                    key: "wire api".to_owned(),
                    value: format!("{:?}", provider.wire_api).to_ascii_lowercase(),
                },
                TuiKeyValueSpec::Plain {
                    key: "reasoning".to_owned(),
                    value: reasoning_effort,
                },
            ],
        }],
        footer_lines: vec![
            "Use /model <selector> to switch when you want a different model.".to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_permissions_command_lines_with_width(width: usize) -> Vec<String> {
    let message_spec = TuiMessageSpec {
        role: "permissions".to_owned(),
        caption: Some("YOLO".to_owned()),
        sections: vec![
            TuiSectionSpec::Callout {
                tone: TuiCalloutTone::Info,
                title: Some("YOLO by default".to_owned()),
                lines: vec![
                    "Hey yo, you only live once, take care.".to_owned(),
                ],
            },
            TuiSectionSpec::KeyValues {
                title: Some("default posture".to_owned()),
                items: vec![
                    TuiKeyValueSpec::Plain {
                        key: "mode".to_owned(),
                        value: "YOLO".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "commands".to_owned(),
                        value: "enabled".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "tools".to_owned(),
                        value: "enabled".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "slash deck".to_owned(),
                        value: "enabled".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "permission prompts".to_owned(),
                        value: "not part of the happy path".to_owned(),
                    },
                ],
            },
            TuiSectionSpec::Narrative {
                title: Some("behavior".to_owned()),
                lines: vec![
                    "This screen stays intentionally simple; it does not show allow/deny tables or ask the user to negotiate routine actions."
                        .to_owned(),
                ],
            },
        ],
        footer_lines: vec!["The default local TUI stays open; stricter deployments can still configure policy explicitly."
            .to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_experimental_command_lines_with_width(width: usize) -> Vec<String> {
    let message_spec = TuiMessageSpec {
        role: "experimental".to_owned(),
        caption: Some("experimental features".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some("enabled surface work".to_owned()),
            items: vec![
                TuiKeyValueSpec::Plain {
                    key: "streaming renderer".to_owned(),
                    value: "enabled".to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "startup animation".to_owned(),
                    value: "enabled".to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "markdown/diff/table preview".to_owned(),
                    value: "enabled".to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "resize smoothing".to_owned(),
                    value: "enabled".to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "slash command deck".to_owned(),
                    value: "enabled".to_owned(),
                },
                TuiKeyValueSpec::Plain {
                    key: "tool activity compaction".to_owned(),
                    value: "enabled".to_owned(),
                },
            ],
        }],
        footer_lines: vec!["No toggle ceremony in the default TUI path.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_themes_command_lines_with_width(width: usize) -> Vec<String> {
    let message_spec = TuiMessageSpec {
        role: "themes".to_owned(),
        caption: Some("theme".to_owned()),
        sections: vec![
            TuiSectionSpec::KeyValues {
                title: Some("current surface".to_owned()),
                items: vec![
                    TuiKeyValueSpec::Plain {
                        key: "palette".to_owned(),
                        value: "terminal-adaptive dark surface".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "accent".to_owned(),
                        value: "startup blue with semantic red/green/yellow states".to_owned(),
                    },
                    TuiKeyValueSpec::Plain {
                        key: "resize".to_owned(),
                        value: "layout recalculates from viewport on every draw".to_owned(),
                    },
                ],
            },
            TuiSectionSpec::Narrative {
                title: Some("behavior".to_owned()),
                lines: vec![
                    "The default theme path is already active: dark, terminal-adaptive, and readable without extra setup."
                        .to_owned(),
                ],
            },
        ],
        footer_lines: vec!["The terminal-adaptive theme is active for this session.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_cwd_command_lines_with_width(runtime: &CliTurnRuntime, width: usize) -> Vec<String> {
    let cwd = runtime
        .effective_working_directory
        .as_deref()
        .unwrap_or(runtime.resolved_path.as_path())
        .display()
        .to_string();
    let message_spec = TuiMessageSpec {
        role: "cwd".to_owned(),
        caption: Some("working directory".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some("current scope".to_owned()),
            items: vec![
                TuiKeyValueSpec::Plain {
                    key: "cwd".to_owned(),
                    value: cwd,
                },
                TuiKeyValueSpec::Plain {
                    key: "session".to_owned(),
                    value: runtime.session_id.clone(),
                },
            ],
        }],
        footer_lines: vec!["Use /cwd <path> to move the chat working directory.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_language_command_lines_with_width(width: usize) -> Vec<String> {
    let language = resolve_default_language();
    let message_spec = TuiMessageSpec {
        role: "language".to_owned(),
        caption: Some("language".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some("current language".to_owned()),
            items: vec![TuiKeyValueSpec::Plain {
                key: "detected".to_owned(),
                value: language_label(language).to_owned(),
            }],
        }],
        footer_lines: vec!["Use /language <locale> to switch the UI language.".to_owned()],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn language_label(language: super::i18n::Language) -> &'static str {
    match language {
        super::i18n::Language::En => "English",
        super::i18n::Language::ZhCn => "简体中文",
        super::i18n::Language::ZhTw => "繁體中文",
        super::i18n::Language::Ja => "日本語",
        super::i18n::Language::Ru => "Русский",
    }
}

fn render_mcp_command_lines_with_width(runtime: &CliTurnRuntime, width: usize) -> Vec<String> {
    let mut items = runtime
        .effective_bootstrap_mcp_servers
        .iter()
        .map(|server| TuiKeyValueSpec::Plain {
            key: server.clone(),
            value: "enabled for this chat".to_owned(),
        })
        .collect::<Vec<_>>();

    if items.is_empty() {
        items.push(TuiKeyValueSpec::Plain {
            key: "configured".to_owned(),
            value: "0".to_owned(),
        });
    }

    let message_spec = TuiMessageSpec {
        role: "mcp".to_owned(),
        caption: Some("MCP".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some(format!(
                "servers ({})",
                runtime.effective_bootstrap_mcp_servers.len()
            )),
            items,
        }],
        footer_lines: vec![
            "Startup keeps this compact; /mcp shows the details on demand.".to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
}

fn render_skills_command_lines_with_width(runtime: &CliTurnRuntime, width: usize) -> Vec<String> {
    let skills = detect_available_skills(runtime.effective_working_directory.as_deref());
    let mut items = skills
        .iter()
        .take(14)
        .map(|skill| {
            let key = if let Some(alias) = skill.source_alias.as_deref() {
                format!("${} ({alias})", skill.name)
            } else {
                format!("${}", skill.name)
            };
            TuiKeyValueSpec::Plain {
                key,
                value: skill.description.clone(),
            }
        })
        .collect::<Vec<_>>();

    if items.is_empty() {
        items.push(TuiKeyValueSpec::Plain {
            key: "available".to_owned(),
            value: "0".to_owned(),
        });
    }

    let hidden_count = skills.len().saturating_sub(items.len());
    let mut footer_lines =
        vec!["Type $skill-name directly in the composer to invoke a skill.".to_owned()];
    if hidden_count > 0 {
        footer_lines.push(format!(
            "Showing 14 of {}; keep typing to filter.",
            skills.len()
        ));
    }

    let message_spec = TuiMessageSpec {
        role: "skills".to_owned(),
        caption: Some("skills".to_owned()),
        sections: vec![TuiSectionSpec::KeyValues {
            title: Some(format!("available ({})", skills.len())),
            items,
        }],
        footer_lines,
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
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
    app.composer_follow_up_intent = false;
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
        start_turn(terminal, app, runtime, next_input, true, None).await?;
    } else if let Some(next_input) = app.pending_queue.pop_front() {
        start_turn(terminal, app, runtime, next_input, true, None).await?;
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
                    participant_id: runtime.session_address.participant_id.clone(),
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
    if app.last_render_width == 0 || app.last_render_height == 0 {
        if !app.pending_turn {
            return None;
        }
        let start = app.turn_start?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        focus_ring_frame(start).hash(&mut hasher);
        get_spinner_verb_with_seed(start, app.spinner_seed, app.i18n.spinner_verbs())
            .hash(&mut hasher);
        app.pending_steers
            .iter()
            .for_each(|message| message.hash(&mut hasher));
        app.pending_queue
            .iter()
            .for_each(|message| message.hash(&mut hasher));
        for line in pending_live_lines(&app.live_lines, 6) {
            line.hash(&mut hasher);
        }
        return Some(hasher.finish());
    }

    let composer_height = app.composer.height_for_width(app.last_render_width);
    let palette_height = if matches!(app.focus, Focus::CommandPalette) {
        app.command_palette.desired_height() as u16
    } else {
        0
    };
    pending_render_signature_for_geometry(
        app,
        app.last_render_width,
        app.last_render_height,
        composer_height,
        palette_height,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
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
    pending_signature_preview_budget_for_geometry(
        app.last_render_height,
        composer_height,
        palette_height,
    )
}

fn pending_signature_preview_budget_for_geometry(
    height: u16,
    composer_height: u16,
    palette_height: u16,
) -> usize {
    let max_pending_height = pending_band_max_height(height, composer_height, palette_height);
    max_pending_height.saturating_sub(2).max(1) as usize
}

fn pending_band_max_height(height: u16, composer_height: u16, palette_height: u16) -> u16 {
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
    height.saturating_sub(reserved_without_pending).max(3)
}

fn pending_render_signature_for_geometry(
    app: &App,
    width: u16,
    height: u16,
    composer_height: u16,
    palette_height: u16,
) -> Option<u64> {
    if !app.pending_turn {
        return None;
    }
    let start = app.turn_start?;
    let max_pending_preview_lines =
        pending_signature_preview_budget_for_geometry(height, composer_height, palette_height);
    let visible_lines = pending_live_lines(&app.live_lines, max_pending_preview_lines);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    focus_ring_frame(start).hash(&mut hasher);
    get_spinner_verb_with_seed(start, app.spinner_seed, app.i18n.spinner_verbs()).hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    visible_lines.hash(&mut hasher);
    app.pending_steers
        .iter()
        .for_each(|message| message.hash(&mut hasher));
    app.pending_queue
        .iter()
        .for_each(|message| message.hash(&mut hasher));
    Some(hasher.finish())
}

fn build_pending_lines(
    turn_start: Option<std::time::Instant>,
    live_lines: &[String],
    spinner_seed: u64,
    spinner_verbs: &'static [&'static str],
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
                .fg(SURFACE_CYAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{}...",
                get_spinner_verb_with_seed(start, spinner_seed, spinner_verbs)
            ),
            Style::default()
                .fg(SURFACE_CYAN)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let content_width = width.saturating_sub(2).max(1) as usize;
    let mut lines = Vec::new();
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
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(ratatui::style::Color::White)
        };
        lines.extend(render_pending_live_line(
            line.as_str(),
            content_width,
            style,
            start,
        ));
    }
    append_pending_input_preview_lines(
        &mut lines,
        pending_steers,
        pending_queue,
        width,
        !live_lines.is_empty(),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(spinner_spans));
    lines
}

fn render_pending_live_line(
    line: &str,
    content_width: usize,
    default_style: Style,
    start: std::time::Instant,
) -> Vec<Line<'static>> {
    if let Some(lines) = render_pending_tool_headline_line(line, content_width, start) {
        return lines;
    }

    if let Some(lines) = render_pending_tool_child_line(line, content_width) {
        return lines;
    }

    if let Some(lines) = render_pending_tool_sample_line(line, content_width) {
        return lines;
    }

    if let Some(lines) = render_pending_key_value_line(line, content_width) {
        return lines;
    }

    crate::presentation::render_wrapped_display_line(line, content_width)
        .into_iter()
        .map(|wrapped| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(render_inline_token_spans(wrapped.as_str(), default_style));
            Line::from(spans)
        })
        .collect()
}

fn render_pending_tool_headline_line(
    line: &str,
    content_width: usize,
    start: std::time::Instant,
) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix("• ").unwrap_or(trimmed);
    let (label, rest, label_style, body_style) = pending_tool_headline_parts(trimmed, start)?;
    let label_text = format!("{label} ");
    let prefix_width = 2 + crate::presentation::display_width(label_text.as_str());
    let body_width = content_width.saturating_sub(prefix_width).max(1);
    let mut wrapped = crate::presentation::render_wrapped_display_line(rest.trim(), body_width);
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled("• ", Style::default().fg(SURFACE_GRAY)),
                        Span::styled(label_text.clone(), label_style),
                    ];
                    spans.extend(render_inline_token_spans(wrapped_line.as_str(), body_style));
                    Line::from(spans)
                } else {
                    let mut spans = vec![Span::raw("  "), Span::raw(" ".repeat(prefix_width))];
                    spans.extend(render_inline_token_spans(wrapped_line.as_str(), body_style));
                    Line::from(spans)
                }
            })
            .collect(),
    )
}

fn pending_tool_headline_parts(
    trimmed: &str,
    start: std::time::Instant,
) -> Option<(&'static str, &str, Style, Style)> {
    if let Some(rest) = trimmed.strip_prefix("Called ") {
        return Some((
            "Called",
            rest,
            Style::default()
                .fg(pending_tool_label_color(start))
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(pending_tool_body_color(start))
                .add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(rest) = trimmed.strip_prefix("Closed ") {
        return Some((
            "Closed",
            rest,
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_GRAY),
        ));
    }

    if let Some(rest) = trimmed.strip_prefix("Approval ") {
        return Some((
            "Approval",
            rest,
            Style::default()
                .fg(SURFACE_ACCENT)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White),
        ));
    }

    if let Some(rest) = trimmed.strip_prefix("Denied ") {
        return Some((
            "Denied",
            rest,
            Style::default()
                .fg(SURFACE_RED)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_RED),
        ));
    }

    None
}

fn pending_tool_animation_frame(start: std::time::Instant) -> usize {
    if reduced_motion_enabled() {
        return PENDING_TOOL_LABEL_COLORS.len().saturating_sub(2);
    }
    pending_tool_animation_frame_for_elapsed(start.elapsed())
}

fn pending_tool_animation_frame_for_elapsed(elapsed: Duration) -> usize {
    let frame_count = PENDING_TOOL_LABEL_COLORS.len().max(1) as u64;
    ((elapsed.as_millis() as u64 / PENDING_TOOL_ANIMATION_FRAME_MS.max(1)) % frame_count) as usize
}

fn pending_tool_label_color(start: std::time::Instant) -> Color {
    let frame = pending_tool_animation_frame(start);
    *PENDING_TOOL_LABEL_COLORS
        .get(frame)
        .unwrap_or(&SURFACE_CYAN)
}

fn pending_tool_body_color(start: std::time::Instant) -> Color {
    let frame = pending_tool_animation_frame(start);
    *PENDING_TOOL_BODY_COLORS.get(frame).unwrap_or(&Color::White)
}

fn render_pending_tool_child_line(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix("↳ ")?;
    let (label, rest) = body.split_once(' ').unwrap_or((body, ""));
    let label_text = if rest.is_empty() {
        String::new()
    } else {
        format!("{label} ")
    };
    let (label_style, body_style) = pending_tool_child_styles(label);
    let prefix_width = 2 + crate::presentation::display_width(label_text.as_str());
    let body_width = content_width.saturating_sub(prefix_width).max(1);
    let mut wrapped = crate::presentation::render_wrapped_display_line(rest.trim(), body_width);
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled("↳ ", Style::default().fg(SURFACE_ACCENT)),
                    ];
                    if !label_text.is_empty() {
                        spans.push(Span::styled(label_text.clone(), label_style));
                    }
                    spans.extend(render_inline_token_spans(wrapped_line.as_str(), body_style));
                    Line::from(spans)
                } else {
                    let mut spans = vec![Span::raw("  "), Span::raw(" ".repeat(prefix_width))];
                    spans.extend(render_inline_token_spans(wrapped_line.as_str(), body_style));
                    Line::from(spans)
                }
            })
            .collect(),
    )
}

fn render_pending_key_value_line(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    let trimmed = line.trim_start();
    let body = trimmed.strip_prefix("- ")?;
    let (key, value) = body.split_once(": ")?;
    let key = key.trim();
    let value = value.trim();
    let prefix = format!("  - {key}: ");
    let prefix_width = crate::presentation::display_width(prefix.as_str());
    let value_width = content_width
        .saturating_sub(prefix_width.saturating_sub(2))
        .max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(value, value_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled("- ", Style::default().fg(SURFACE_DIM_GRAY)),
                        Span::styled(
                            format!("{key}: "),
                            Style::default()
                                .fg(SURFACE_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ];
                    spans.extend(render_inline_token_spans(
                        wrapped_line.as_str(),
                        Style::default().fg(Color::White),
                    ));
                    Line::from(spans)
                } else {
                    let mut spans = vec![Span::raw(" ".repeat(prefix_width))];
                    spans.extend(render_inline_token_spans(
                        wrapped_line.as_str(),
                        Style::default().fg(Color::White),
                    ));
                    Line::from(spans)
                }
            })
            .collect(),
    )
}

fn pending_tool_child_styles(label: &str) -> (Style, Style) {
    match label {
        "stdout" => (
            Style::default()
                .fg(SURFACE_GREEN)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_GRAY),
        ),
        "stderr" => (
            Style::default()
                .fg(SURFACE_RED)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_RED).add_modifier(Modifier::DIM),
        ),
        "file" => (
            Style::default()
                .fg(SURFACE_CYAN)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_GRAY),
        ),
        "metrics" => (
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_GRAY),
        ),
        "request" | "args" => (
            Style::default()
                .fg(SURFACE_ACCENT)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(SURFACE_GRAY),
        ),
        _ => (
            Style::default().fg(SURFACE_ACCENT),
            Style::default().fg(SURFACE_GRAY),
        ),
    }
}

fn render_pending_tool_sample_line(line: &str, content_width: usize) -> Option<Vec<Line<'static>>> {
    if !line.starts_with("    ") {
        return None;
    }

    let sample = line.trim_start();
    if sample.is_empty() {
        return None;
    }

    let sample_style = if sample.starts_with('+') {
        Style::default().fg(SURFACE_GREEN)
    } else if sample.starts_with('-') {
        Style::default().fg(SURFACE_RED)
    } else {
        Style::default().fg(SURFACE_GRAY)
    };
    let sample_width = content_width.saturating_sub(4).max(1);

    Some(
        crate::presentation::render_wrapped_display_line(sample, sample_width)
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                let guide = if index == 0 { "    " } else { "      " };
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(guide, Style::default().fg(SURFACE_GRAY)),
                ];
                spans.extend(render_inline_token_spans(
                    wrapped_line.as_str(),
                    sample_style,
                ));
                Line::from(spans)
            })
            .collect(),
    )
}

fn append_pending_input_preview_lines(
    lines: &mut Vec<Line<'static>>,
    pending_steers: &VecDeque<String>,
    pending_queue: &VecDeque<String>,
    width: u16,
    has_live_preview: bool,
) {
    const MAX_PENDING_PREVIEW_MESSAGES: usize = 3;

    if pending_steers.is_empty() && pending_queue.is_empty() {
        return;
    }

    if has_live_preview || lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::from(""));
    }

    let content_width = width.saturating_sub(6).max(1) as usize;
    let mut remaining_preview_budget = MAX_PENDING_PREVIEW_MESSAGES;
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
                    Style::default()
                        .fg(SURFACE_CYAN)
                        .add_modifier(Modifier::DIM),
                )
            })
            .collect::<Vec<_>>();
        let displayed = push_pending_input_lines(
            lines,
            &preview_items,
            content_width,
            "    ↳ ",
            remaining_preview_budget,
        );
        remaining_preview_budget = remaining_preview_budget.saturating_sub(displayed);
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
                        .fg(SURFACE_GRAY)
                        .add_modifier(Modifier::DIM | Modifier::ITALIC),
                )
            })
            .collect::<Vec<_>>();
        push_pending_input_lines(
            lines,
            &preview_items,
            content_width,
            "    ↳ ",
            remaining_preview_budget,
        );
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
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(title.to_owned(), Style::default().fg(SURFACE_GRAY)),
    ];
    if let Some(key_hint) = key_hint {
        spans.push(Span::styled(
            " (press ".to_owned(),
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(
            key_hint.to_owned(),
            Style::default()
                .fg(SURFACE_ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {suffix})"),
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::DIM),
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
            Style::default()
                .fg(SURFACE_GRAY)
                .add_modifier(Modifier::DIM),
        )]));
    }
}

fn push_pending_input_lines(
    lines: &mut Vec<Line<'static>>,
    messages: &[(&str, Style)],
    content_width: usize,
    first_prefix: &str,
    max_preview_messages: usize,
) -> usize {
    let displayed_messages = messages.len().min(max_preview_messages);
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

    let remaining_messages = messages.len().saturating_sub(displayed_messages);
    if remaining_messages > 0 {
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(
                format!("… +{remaining_messages} more"),
                Style::default()
                    .fg(SURFACE_GRAY)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    displayed_messages
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

#[cfg(test)]
fn startup_version_line() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

fn detect_available_skills(root: Option<&Path>) -> Vec<SkillEntry> {
    let mut seen_dirs = HashSet::new();
    let mut seen_names = HashSet::new();
    let mut skills = Vec::new();

    for source in installed_skill_search_roots(root) {
        let normalized_dir = source
            .directory
            .canonicalize()
            .unwrap_or_else(|_| source.directory.clone());
        if !seen_dirs.insert(normalized_dir) {
            continue;
        }

        for skill_dir in skill_dirs_in(source.directory.as_path()) {
            let folder_name = skill_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_owned());
            let skill = read_skill_metadata(
                folder_name,
                skill_dir.join("SKILL.md"),
                source.category_tag,
                source.search_label,
            );
            let name_key = skill.name.to_ascii_lowercase();
            if seen_names.insert(name_key) {
                skills.push(skill);
            }
        }
    }

    skills.sort_by(|left, right| {
        skill_source_priority(left.category_tag.as_str())
            .cmp(&skill_source_priority(right.category_tag.as_str()))
            .then_with(|| left.name.cmp(&right.name))
    });
    skills
}

fn detect_onboarding_optional_repo_skills(root: Option<&Path>) -> Vec<RepoOptionalSkill> {
    let repo_skills_root = root
        .map(|path| path.join("skills"))
        .unwrap_or_else(|| Path::new("skills").to_path_buf());
    let mut skills = skill_dirs_in(repo_skills_root.as_path())
        .into_iter()
        .filter_map(|skill_dir| {
            let skill_doc_path = skill_dir.join("SKILL.md");
            let contents = std::fs::read_to_string(&skill_doc_path).ok();
            if contents
                .as_deref()
                .is_some_and(repo_skill_onboarding_mode_is_bundled)
            {
                return None;
            }
            let relative_path = skill_dir
                .strip_prefix(&repo_skills_root)
                .ok()?
                .to_path_buf();
            let folder_name = skill_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".to_owned());
            let skill = read_skill_metadata(folder_name, skill_doc_path, "[Repo]", "repo");
            Some(RepoOptionalSkill {
                name: skill.name,
                description: skill.description,
                source_dir: skill_dir,
                install_relative_path: relative_path,
            })
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| {
        left.install_relative_path
            .cmp(&right.install_relative_path)
            .then_with(|| left.name.cmp(&right.name))
    });
    skills
}

fn repo_skill_onboarding_mode_is_bundled(contents: &str) -> bool {
    let Some(raw_mode) = parse_skill_frontmatter_value(contents, "onboarding_mode") else {
        return false;
    };
    let normalized = raw_mode
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "bundled" | "mandatory" | "hidden" | "always_on"
    )
}

struct SkillSearchRoot {
    directory: std::path::PathBuf,
    category_tag: &'static str,
    search_label: &'static str,
}

fn installed_skill_search_roots(root: Option<&Path>) -> Vec<SkillSearchRoot> {
    let mut roots = Vec::new();

    if let Some(root) = root {
        roots.push(SkillSearchRoot {
            directory: root.join(".loong").join("skills"),
            category_tag: "[Skill]",
            search_label: "workspace",
        });
    }

    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        roots.push(SkillSearchRoot {
            directory: std::path::PathBuf::from(codex_home).join("skills"),
            category_tag: "[Skill]",
            search_label: "global",
        });
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        roots.push(SkillSearchRoot {
            directory: home.join(".codex").join("skills"),
            category_tag: "[Skill]",
            search_label: "global",
        });
        roots.push(SkillSearchRoot {
            directory: home.join(".agents").join("skills"),
            category_tag: "[Skill]",
            search_label: "agent",
        });
    }

    roots
}

fn skill_dirs_in(skills_dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };

    let mut skill_dirs = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("SKILL.md").is_file() {
            skill_dirs.push(path);
            continue;
        }
        let Ok(children) = std::fs::read_dir(path) else {
            continue;
        };
        skill_dirs.extend(
            children
                .filter_map(|child| child.ok())
                .filter(|child| child.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                .map(|child| child.path())
                .filter(|child| child.join("SKILL.md").is_file()),
        );
    }
    skill_dirs
}

fn skill_source_priority(category_tag: &str) -> u8 {
    match category_tag {
        "[Repo]" => 0,
        "[Skill]" => 1,
        _ => 2,
    }
}

fn read_skill_metadata(
    folder_name: String,
    skill_doc_path: std::path::PathBuf,
    category_tag: &'static str,
    search_label: &'static str,
) -> SkillEntry {
    let Ok(contents) = std::fs::read_to_string(skill_doc_path) else {
        return SkillEntry {
            name: folder_name.clone(),
            description: "available skill".to_owned(),
            search_terms: build_skill_search_terms(
                folder_name.as_str(),
                folder_name.as_str(),
                search_label,
            ),
            category_tag: category_tag.to_owned(),
            source_alias: None,
        };
    };

    let name = parse_skill_frontmatter_value(contents.as_str(), "name")
        .filter(|value| !value.is_empty())
        .unwrap_or(folder_name.clone());
    let description = parse_skill_frontmatter_value(contents.as_str(), "description")
        .filter(|value| !value.is_empty())
        .or_else(|| fallback_skill_description(contents.as_str()))
        .unwrap_or_else(|| "available skill".to_owned());
    let search_terms = build_skill_search_terms(folder_name.as_str(), name.as_str(), search_label);
    let source_alias = (folder_name != name).then_some(folder_name);

    SkillEntry {
        name,
        description,
        search_terms,
        category_tag: category_tag.to_owned(),
        source_alias,
    }
}

fn build_skill_search_terms(folder_name: &str, name: &str, source_label: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for value in [folder_name, name, source_label] {
        if !terms.iter().any(|term| term == value) {
            terms.push(value.to_owned());
        }
        for segment in value.split(|ch: char| ch == '-' || ch == '_' || ch.is_whitespace()) {
            let trimmed = segment.trim();
            if trimmed.len() >= 2 && !terms.iter().any(|term| term == trimmed) {
                terms.push(trimmed.to_owned());
            }
        }
    }
    terms
}

fn parse_skill_frontmatter_value(contents: &str, key: &str) -> Option<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    let mut inside_frontmatter = false;
    let mut frontmatter_consumed = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            if !frontmatter_consumed {
                inside_frontmatter = !inside_frontmatter;
                if !inside_frontmatter {
                    frontmatter_consumed = true;
                }
            }
            continue;
        }

        if inside_frontmatter && let Some(value) = trimmed.strip_prefix(&format!("{key}:")) {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }

    None
}

fn fallback_skill_description(contents: &str) -> Option<String> {
    contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && *line != "---")
        .map(ToOwned::to_owned)
}

#[allow(dead_code)]
fn render_chat_surface_help_lines_with_width(width: usize) -> Vec<String> {
    render_chat_surface_help_lines_with_width_and_extension_commands(width, &[])
}

fn render_chat_surface_help_lines_with_width_and_extension_commands(
    width: usize,
    extension_commands: &[(String, String)],
) -> Vec<String> {
    let queue_restore_shortcut = queue_restore_shortcut_label();
    let mut slash_command_items = slash_command_specs()
        .iter()
        .map(|spec| TuiKeyValueSpec::Plain {
            key: spec.command.to_owned(),
            value: slash_command_help_value(spec),
        })
        .collect::<Vec<_>>();
    slash_command_items.push(TuiKeyValueSpec::Plain {
        key: "$skill-name <request>".to_owned(),
        value: "type an available skill invocation directly in the composer".to_owned(),
    });
    let extension_command_section =
        (!extension_commands.is_empty()).then(|| TuiSectionSpec::KeyValues {
            title: Some("extension commands".to_owned()),
            items: extension_commands
                .iter()
                .map(|(command, description)| TuiKeyValueSpec::Plain {
                    key: command.clone(),
                    value: description.clone(),
                })
                .collect(),
        });

    let message_spec = TuiMessageSpec {
        role: "help".to_owned(),
        caption: Some("chat surface".to_owned()),
        sections: {
            let mut sections = vec![TuiSectionSpec::KeyValues {
                title: Some("slash commands".to_owned()),
                items: slash_command_items,
            }];
            if let Some(extension_command_section) = extension_command_section {
                sections.push(extension_command_section);
            }
            sections.extend([
                TuiSectionSpec::Narrative {
                    title: Some("surface controls".to_owned()),
                    lines: {
                        let mut lines = vec![
                        "Use / or : from an empty composer to open the command palette.".to_owned(),
                        "Type $skill-name directly in the composer, then continue writing the rest of the request."
                            .to_owned(),
                        "When the inline $ suggestion popup is visible, Enter or Tab confirms the current skill."
                            .to_owned(),
                        "Use Ctrl+O to expand or collapse the latest compaction summary.".to_owned(),
                        ];
                        if !extension_commands.is_empty() {
                            lines.push(
                                "Runtime extension commands show up alongside built-ins whenever the command palette refreshes."
                                    .to_owned(),
                            );
                        }
                        lines
                    },
                },
                TuiSectionSpec::Narrative {
                    title: Some("keyboard".to_owned()),
                    lines: vec![
                        "Enter sends the current draft. Shift+Enter inserts a new line."
                            .to_owned(),
                        format!(
                            "Tab moves between composer and transcript. While a turn is running, Tab queues the current draft and {queue_restore_shortcut} restores the latest queued message."
                        ),
                        "PgUp / PgDn and Home / End scroll the transcript; printable keys return to the composer immediately."
                            .to_owned(),
                    ],
                },
                TuiSectionSpec::Callout {
                    tone: TuiCalloutTone::Info,
                    title: Some("mouse".to_owned()),
                    lines: vec![
                        "Mouse wheel scrolls the transcript where terminal alternate-scroll is supported."
                            .to_owned(),
                        "Native terminal drag-selection remains available by default.".to_owned(),
                    ],
                },
                TuiSectionSpec::Callout {
                    tone: TuiCalloutTone::Info,
                    title: Some("usage notes".to_owned()),
                    lines: vec![
                        "Type any non-command text to send a normal assistant turn.".to_owned(),
                        "Available skill names can be invoked directly with $skill-name."
                            .to_owned(),
                        "Use Ctrl+C to leave chat.".to_owned(),
                    ],
                },
            ]);
            sections
        },
        footer_lines: vec![
            "Send normal text to continue the transcript.".to_owned(),
            "Use /usage, /review, or /compact when you need to inspect or stabilize the current session."
                .to_owned(),
        ],
    };
    super::super::render_cli_chat_message_spec_with_width(&message_spec, width)
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
        .user_agent("loongclaw-chat-surface")
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

fn resize_reflow_required(
    previous_width: u16,
    previous_height: u16,
    next_width: u16,
    next_height: u16,
) -> bool {
    previous_width != next_width || previous_height != next_height
}

fn resize_live_rerender_ready(
    pending_live_resize_rerender: bool,
    since_last_resize: Option<Duration>,
) -> bool {
    pending_live_resize_rerender
        && since_last_resize
            .map(|elapsed| elapsed >= Duration::from_millis(70))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{App, Focus};
    use crate::chat::chat_surface::command_palette::{
        CommandAction, CommandPalette, SkillEntry, slash_command_specs,
    };
    use crate::chat::chat_surface::composer::Composer;
    use crate::chat::chat_surface::i18n::{I18nService, Language};
    use crate::chat::chat_surface::message_list::{
        MessageList, StartupEyeAnimation, StartupEyeFocus,
    };
    use crate::chat::chat_surface::onboarding::{
        StartupOnboardingController, startup_palette_eye_focus,
    };
    use crate::chat::chat_surface::utils::SURFACE_USER_MSG_BG;
    #[cfg(feature = "memory-sqlite")]
    use crate::chat::tests::{cleanup_chat_test_memory, init_chat_test_memory};
    use crate::test_support::ScopedEnv;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Modifier};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    fn blank_app() -> App {
        App {
            message_list: MessageList::new(),
            composer: Composer::new(),
            command_palette: CommandPalette::new(Language::En, Vec::new()),
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
            pending_render_cache: None,
            inline_skill_popup_active: false,
            last_render_width: 0,
            last_render_height: 0,
            last_transcript_area: Rect::default(),
            last_composer_area: Rect::default(),
            last_palette_area: Rect::default(),
            cwd: "/tmp/example".to_owned(),
            model: "gpt-test".to_owned(),
            title: None,
            i18n: I18nService::new(Language::En),
            startup_eye_feedback: None,
            startup_onboarding: StartupOnboardingController::new(None, None, Vec::new(), 0),
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn skill(name: &str) -> SkillEntry {
        SkillEntry {
            name: name.to_owned(),
            description: format!("{name} description"),
            search_terms: vec![name.to_owned()],
            category_tag: "[Skill]".to_owned(),
            source_alias: None,
        }
    }

    #[cfg(feature = "memory-sqlite")]
    #[test]
    fn reload_runtime_for_surface_turn_applies_transient_calibration_addendum() {
        let (config, _memory_config, sqlite_path) = init_chat_test_memory("surface-addendum");
        let options = crate::chat::CliChatOptions::default();
        let runtime = crate::chat::initialize_cli_turn_runtime_with_loaded_config(
            std::path::PathBuf::from("/tmp/loong.toml"),
            config,
            Some("surface-addendum"),
            &options,
            "cli-chat-surface-addendum-test",
            crate::chat::CliSessionRequirement::RequireExplicit,
            false,
        )
        .expect("runtime");

        let refreshed = super::reload_runtime_for_surface_turn(
            &runtime,
            Some("Ask one short calibration question first.".to_owned()),
        );

        assert!(
            refreshed
                .config
                .cli
                .system_prompt_addendum
                .as_deref()
                .is_some_and(|value| value.contains("Ask one short calibration question first."))
        );
        assert!(
            refreshed
                .config
                .cli
                .system_prompt
                .contains("Ask one short calibration question first.")
        );
        assert!(runtime.config.cli.system_prompt_addendum.is_none());

        cleanup_chat_test_memory(&sqlite_path);
    }

    #[test]
    fn resize_reflow_tracks_width_and_height_changes() {
        assert!(super::resize_reflow_required(80, 24, 72, 24));
        assert!(super::resize_reflow_required(80, 24, 80, 32));
        assert!(!super::resize_reflow_required(80, 24, 80, 24));
    }

    #[test]
    fn resize_live_rerender_waits_for_quiet_window() {
        assert!(!super::resize_live_rerender_ready(false, None));
        assert!(super::resize_live_rerender_ready(true, None));
        assert!(!super::resize_live_rerender_ready(
            true,
            Some(Duration::from_millis(32))
        ));
        assert!(super::resize_live_rerender_ready(
            true,
            Some(Duration::from_millis(70))
        ));
    }

    #[test]
    fn pending_tool_animation_frames_cycle_between_dim_and_bright_states() {
        let early = super::pending_tool_animation_frame_for_elapsed(Duration::from_millis(0));
        let bright = super::pending_tool_animation_frame_for_elapsed(Duration::from_millis(360));

        assert_ne!(early, bright);
        assert_eq!(
            super::PENDING_TOOL_LABEL_COLORS[early],
            super::SURFACE_DIM_GRAY
        );
        assert_eq!(
            super::PENDING_TOOL_LABEL_COLORS[bright],
            super::Color::White
        );
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
        let cwd = std::env::current_dir()
            .expect("current dir")
            .join("nested")
            .join("session-tail-for-footer-test");
        let cwd = cwd.to_string_lossy();
        let line = super::build_status_footer_line(cwd.as_ref(), "gpt-5.4", 32);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(crate::presentation::display_width(&rendered), 32);
        assert!(rendered.contains("gpt-5.4"));
        assert!(rendered.contains("…"));
        assert!(rendered.contains("footer-test"));
        assert_eq!(rendered.chars().next(), cwd.chars().next());
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
        let path = std::env::current_dir()
            .expect("current dir")
            .join("worktrees")
            .join("project-name")
            .join("session");
        let path = path.to_string_lossy();
        let truncated = super::truncate_middle_for_width(path.as_ref(), 20);

        assert_eq!(truncated.chars().next(), path.chars().next());
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
    fn startup_version_line_is_product_only() {
        let version = super::startup_version_line();

        assert_eq!(version, format!("v{}", env!("CARGO_PKG_VERSION")));
        assert!(!version.contains(" · "));
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
        assert!(rendered.contains("queued ×12"));
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
        assert!(rendered.contains("restore ×12"));
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
    fn footer_keeps_one_breathing_row_when_transcript_fills_available_height() {
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

        assert_eq!(
            footer_row,
            lines
                .len()
                .saturating_sub(super::FOOTER_BOTTOM_BREATHING_HEIGHT as usize + 1)
        );
        assert!(lines.last().is_some_and(|line| line.trim().is_empty()));
    }

    #[test]
    fn footer_content_uses_left_indent_when_space_allows() {
        let backend = TestBackend::new(50, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.message_list.add_assistant_message("hello".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let footer_line = lines
            .iter()
            .find(|line| line.contains("/tmp/example"))
            .expect("footer line");

        assert!(footer_line.starts_with("  /tmp/example"));
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
    fn composer_and_footer_do_not_jump_up_after_pending_turn_finishes() {
        let backend = TestBackend::new(60, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["streamed reply line".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw pending");
        let pending_lines = buffer_lines(&terminal);
        let pending_composer_row = pending_lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("pending composer row");
        let pending_footer_row = pending_lines
            .iter()
            .position(|line| line.contains("/tmp/example"))
            .expect("pending footer row");

        app.pending_turn = false;
        app.turn_start = None;
        if let Ok(mut lines) = app.live_lines.lock() {
            lines.clear();
        }
        app.message_list
            .add_assistant_message("streamed reply line".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw complete");
        let settled_lines = buffer_lines(&terminal);
        let settled_composer_row = settled_lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("settled composer row");
        let settled_footer_row = settled_lines
            .iter()
            .position(|line| line.contains("/tmp/example"))
            .expect("settled footer row");

        assert!(settled_composer_row >= pending_composer_row);
        assert!(settled_footer_row >= pending_footer_row);
    }

    #[test]
    fn spinner_stays_adjacent_to_composer_below_pending_content() {
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
        let spinner_row = lines
            .iter()
            .position(|line| line.contains("..."))
            .expect("spinner row");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");

        assert!(preview_row < spinner_row);
        assert_eq!(composer_row, spinner_row + 2);
    }

    #[test]
    fn split_surface_command_preserves_arguments() {
        assert_eq!(
            super::split_surface_command("/copy explicit text"),
            ("/copy", "explicit text")
        );
        assert_eq!(super::split_surface_command("  /diff  "), ("/diff", ""));
    }

    #[test]
    fn staging_commands_populate_composer_drafts() {
        let mut app = blank_app();
        app.message_list
            .add_assistant_message("existing answer".to_owned());

        super::stage_simplify_prompt(&mut app, "").expect("simplify stage");
        assert!(app.composer.text().contains("existing answer"));
        assert!(app.composer.text().contains("simplify"));

        super::stage_plan_prompt(&mut app, "the rollout").expect("plan stage");
        assert!(app.composer.text().contains("the rollout"));
    }

    #[test]
    fn export_filename_components_are_safe() {
        assert_eq!(super::safe_file_component("abc-DEF_123"), "abc-DEF_123");
        assert_eq!(super::safe_file_component("a/b:c"), "a-b-c");
    }

    #[test]
    fn help_lines_match_chat_surface_controls() {
        let rendered = super::render_chat_surface_help_lines_with_width(80).join("\n");

        assert!(rendered.contains("Shift+Enter inserts a new line"));
        assert!(rendered.contains("Use / or : from an empty composer"));
        assert!(rendered.contains("Type $skill-name directly in the composer"));
        assert!(rendered.contains("printable keys return"));
        assert!(rendered.contains("Native terminal drag-selection remains available"));
        assert!(!rendered.contains("coming soon"));
        assert!(!rendered.contains("A trailing \\\\ keeps composing"));
        assert!(!rendered.contains("control deck"));
        assert!(!rendered.contains("Esc from an empty composer"));
    }

    #[test]
    fn help_lines_can_list_extension_commands() {
        let rendered = super::render_chat_surface_help_lines_with_width_and_extension_commands(
            80,
            &[(
                "/hello-ext".to_owned(),
                "say hello from the extension".to_owned(),
            )],
        )
        .join("\n");

        assert!(rendered.contains("extension commands"));
        assert!(rendered.contains("/hello-ext"));
        assert!(rendered.contains("say hello from the extension"));
        assert!(rendered.contains("Runtime extension commands show up alongside built-ins"));
    }

    #[test]
    fn slash_usage_lines_note_runtime_extension_refresh() {
        let rendered = super::render_slash_command_usage_lines_with_width_and_extension_commands(
            80,
            &[(
                "/hello-ext".to_owned(),
                "say hello from the extension".to_owned(),
            )],
        )
        .join("\n");

        assert!(rendered.contains("Runtime extension commands join this deck automatically"));
    }

    #[test]
    fn slash_usage_and_detail_cards_are_enabled_without_placeholder_copy() {
        let usage = super::render_slash_command_usage_lines_with_width(90).join("\n");
        assert!(usage.contains("Every command stays visible"));
        assert!(!usage.contains("coming soon"));
        assert!(!usage.contains("placeholder"));
        assert!(!usage.contains("not wired"));

        let share_spec = slash_command_specs()
            .iter()
            .find(|spec| spec.command == "/share")
            .expect("/share spec");
        let detail = super::render_slash_command_detail_lines_with_width(share_spec, 90).join("\n");
        assert!(detail.contains("enabled"));
        assert!(detail.contains("/share is available"));
        assert!(detail.contains("write a local transcript artifact"));
        assert!(!detail.contains("coming soon"));
        assert!(!detail.contains("placeholder"));
        assert!(!detail.contains("not wired"));
    }

    #[test]
    fn slash_usage_and_detail_cards_surface_registry_aliases() {
        let usage = super::render_slash_command_usage_lines_with_width(90).join("\n");
        assert!(usage.contains("aliases: /mission"));
        assert!(usage.contains("aliases: /workers"));

        let mission_spec = super::find_slash_command_spec("/mission").expect("/mission alias spec");
        let detail =
            super::render_slash_command_detail_lines_with_width(mission_spec, 90).join("\n");
        assert!(detail.contains("aliases"));
        assert!(detail.contains("alias: /mission"));
    }

    #[test]
    fn command_palette_keeps_slash_query_visible_in_composer() {
        let backend = TestBackend::new(60, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        super::open_command_palette_with_query(&mut app, "/");

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");

        assert!(lines.contains(" › /"));
    }

    #[test]
    fn command_palette_backspace_can_delete_leading_slash_and_close_deck() {
        let mut app = blank_app();
        super::open_command_palette_with_query(&mut app, "/");

        let action = super::handle_command_palette_key(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );

        assert!(action.is_none());
        assert_eq!(app.focus, Focus::Composer);
        assert!(app.composer.is_empty());
    }

    #[test]
    fn command_palette_backspace_updates_query_text_in_composer() {
        let mut app = blank_app();
        super::open_command_palette_with_query(&mut app, "/review");

        let action = super::handle_command_palette_key(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );

        assert!(action.is_none());
        assert_eq!(app.focus, Focus::CommandPalette);
        assert_eq!(app.composer.text(), "/revie");
    }

    #[test]
    fn command_palette_run_action_clears_slash_query_after_selection() {
        let mut app = blank_app();
        super::open_command_palette_with_query(&mut app, "/experimental");

        let command =
            app.apply_palette_action(CommandAction::RunCommand("/experimental".to_owned()));

        assert_eq!(command.as_deref(), Some("/experimental"));
        assert!(app.composer.is_empty());
        assert!(!app.composer_follow_up_intent);
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn command_palette_keyboard_enter_returns_run_command_for_selected_entry() {
        let mut app = blank_app();
        super::open_command_palette_with_query(&mut app, "/experimental");

        let action = super::handle_command_palette_key(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        match action {
            Some(CommandAction::RunCommand(command)) => {
                assert_eq!(command, "/experimental");
            }
            other => panic!("expected run command, got {other:?}"),
        }
    }

    #[test]
    fn permissions_command_keeps_yolo_default_copy_simple() {
        let rendered = super::render_permissions_command_lines_with_width(80).join("\n");

        assert!(rendered.contains("YOLO by default"));
        assert!(rendered.contains("Hey yo, you only live once, take care."));
        assert!(rendered.contains("commands"));
        assert!(rendered.contains("enabled"));
        assert!(rendered.contains("not part of the happy path"));
        assert!(!rendered.contains("current policy"));
        assert!(!rendered.contains("shell allow"));
        assert!(!rendered.contains("shell deny"));
        assert!(!rendered.contains("file root"));
    }

    #[test]
    fn experimental_command_reports_enabled_surface_features() {
        let rendered = super::render_experimental_command_lines_with_width(80).join("\n");

        assert!(rendered.contains("streaming renderer"));
        assert!(rendered.contains("startup animation"));
        assert!(rendered.contains("resize smoothing"));
        assert!(rendered.contains("enabled"));
        assert!(!rendered.contains("disabled"));
        assert!(!rendered.contains("toggles remain config-driven"));
    }

    #[test]
    fn typing_dollar_keeps_focus_in_composer_while_inline_skill_popup_filters() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(
            Language::En,
            vec![skill("demo-skill"), skill("other-skill")],
        );

        assert!(
            app.composer
                .handle_key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('$'),
                    KeyModifiers::NONE,
                ))
                .is_none()
        );
        app.sync_inline_skill_popup();
        assert_eq!(app.focus, Focus::Composer);
        assert!(app.inline_skill_popup_active);
        assert_eq!(app.composer.text(), "$");

        assert!(
            app.composer
                .handle_key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('d'),
                    KeyModifiers::NONE,
                ))
                .is_none()
        );
        app.sync_inline_skill_popup();

        if let Some(action) = app
            .command_palette
            .handle_key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))
        {
            let _ = app.apply_palette_action(action);
        }
        assert_eq!(app.composer.text(), "$demo-skill ");
        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn typing_dollar_without_available_skills_keeps_plain_text_without_popup() {
        let mut app = blank_app();

        assert!(
            app.composer
                .handle_key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('$'),
                    KeyModifiers::NONE,
                ))
                .is_none()
        );
        app.sync_inline_skill_popup();

        assert_eq!(app.focus, Focus::Composer);
        assert!(!app.inline_skill_popup_active);
        assert_eq!(app.composer.text(), "$");
    }

    #[test]
    fn confirming_inline_skill_popup_with_no_matches_closes_popup_and_keeps_text() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$zzz".to_owned());
        app.sync_inline_skill_popup();

        assert!(app.inline_skill_popup_active);

        app.confirm_inline_skill_popup();

        assert_eq!(app.composer.text(), "$zzz");
        assert_eq!(app.focus, Focus::Composer);
        assert!(!app.inline_skill_popup_active);
    }

    #[test]
    fn read_skill_metadata_prefers_frontmatter_name_and_description() {
        let skill = super::read_skill_metadata(
            "folder-fallback".to_owned(),
            std::path::PathBuf::from("/tmp/nonexistent")
                .with_file_name("skill.md")
                .with_extension("tmp"),
            "[Repo]",
            "repo",
        );
        assert_eq!(skill.name, "folder-fallback");
        assert_eq!(skill.description, "available skill");
        assert_eq!(skill.category_tag, "[Repo]");

        let contents = r#"---
name: actual-skill
description: "actual description"
---

# Skill
"#;
        let dir = std::env::temp_dir().join(format!(
            "loong-chat-skill-meta-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("SKILL.md");
        std::fs::write(&path, contents).expect("write");

        let skill = super::read_skill_metadata(
            "folder-fallback".to_owned(),
            path.clone(),
            "[Repo]",
            "repo",
        );
        assert_eq!(skill.name, "actual-skill");
        assert_eq!(skill.description, "actual description");
        assert!(skill.search_terms.iter().any(|term| term == "repo"));

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn detect_available_skills_reads_installed_skill_metadata_from_codex_home() {
        let mut env = ScopedEnv::new();
        let root = std::env::temp_dir().join(format!(
            "loong-chat-skills-root-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        env.set("CODEX_HOME", &root);
        let skills_dir = root.join("skills");
        std::fs::create_dir_all(&skills_dir).expect("mkdir skills");

        let alpha_dir = skills_dir.join("alpha");
        std::fs::create_dir_all(&alpha_dir).expect("mkdir alpha");
        std::fs::write(
            alpha_dir.join("SKILL.md"),
            "---\nname: alpha-skill\ndescription: alpha description\n---\n",
        )
        .expect("write alpha");

        let beta_dir = skills_dir.join("beta");
        std::fs::create_dir_all(&beta_dir).expect("mkdir beta");
        std::fs::write(
            beta_dir.join("SKILL.md"),
            "# Beta\nbeta fallback description\n",
        )
        .expect("write beta");

        let skills = super::detect_available_skills(Some(root.as_path()));

        let alpha = skills
            .iter()
            .find(|skill| skill.name == "alpha-skill")
            .expect("alpha skill");
        assert_eq!(alpha.description, "alpha description");
        assert_eq!(alpha.category_tag, "[Skill]");
        assert!(alpha.search_terms.iter().any(|term| term == "alpha"));

        let beta = skills
            .iter()
            .find(|skill| skill.name == "beta")
            .expect("beta skill");
        assert_eq!(beta.description, "beta fallback description");
        assert_eq!(beta.category_tag, "[Skill]");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn detect_onboarding_optional_repo_skills_reads_repo_skill_metadata_from_workspace() {
        let root = std::env::temp_dir().join(format!(
            "loong-chat-optional-skills-root-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let skills_dir = root.join("skills").join("anthropic-office");
        std::fs::create_dir_all(&skills_dir).expect("mkdir skills");

        let alpha_dir = skills_dir.join("alpha");
        std::fs::create_dir_all(&alpha_dir).expect("mkdir alpha");
        std::fs::write(
            alpha_dir.join("SKILL.md"),
            "---\nname: alpha-skill\ndescription: alpha description\n---\n",
        )
        .expect("write alpha");

        let skills = super::detect_onboarding_optional_repo_skills(Some(root.as_path()));
        let alpha = skills
            .iter()
            .find(|skill| skill.name == "alpha-skill")
            .expect("alpha skill");
        assert_eq!(alpha.description, "alpha description");
        assert_eq!(
            alpha.install_relative_path,
            std::path::PathBuf::from("anthropic-office").join("alpha")
        );
        assert_eq!(alpha.source_dir, alpha_dir);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn detect_onboarding_optional_repo_skills_skips_bundled_repo_skills() {
        let root = std::env::temp_dir().join(format!(
            "loong-chat-optional-skills-filter-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let skills_dir = root.join("skills");
        std::fs::create_dir_all(&skills_dir).expect("mkdir skills");

        let optional_dir = skills_dir.join("optional");
        std::fs::create_dir_all(&optional_dir).expect("mkdir optional");
        std::fs::write(
            optional_dir.join("SKILL.md"),
            "---\nname: optional-skill\ndescription: optional description\nonboarding_mode: optional\n---\n",
        )
        .expect("write optional skill");

        let bundled_dir = skills_dir.join("bundled");
        std::fs::create_dir_all(&bundled_dir).expect("mkdir bundled");
        std::fs::write(
            bundled_dir.join("SKILL.md"),
            "---\nname: bundled-skill\ndescription: bundled description\nonboarding_mode: bundled\n---\n",
        )
        .expect("write bundled skill");

        let skills = super::detect_onboarding_optional_repo_skills(Some(root.as_path()));

        assert!(skills.iter().any(|skill| skill.name == "optional-skill"));
        assert!(!skills.iter().any(|skill| skill.name == "bundled-skill"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn build_skill_search_terms_includes_folder_name_and_source_segments() {
        let terms = super::build_skill_search_terms("babysit-pr", "PR Babysitter", "repo");

        assert!(terms.iter().any(|term| term == "babysit-pr"));
        assert!(terms.iter().any(|term| term == "babysit"));
        assert!(terms.iter().any(|term| term == "pr"));
        assert!(terms.iter().any(|term| term == "PR Babysitter"));
        assert!(terms.iter().any(|term| term == "Babysitter"));
        assert!(terms.iter().any(|term| term == "repo"));
    }

    #[test]
    fn confirming_inline_skill_popup_keeps_focus_in_composer() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$dem".to_owned());
        app.sync_inline_skill_popup();

        assert!(app.inline_skill_popup_active);

        app.confirm_inline_skill_popup();

        assert_eq!(app.composer.text(), "$demo-skill ");
        assert_eq!(app.focus, Focus::Composer);
        assert!(!app.inline_skill_popup_active);
    }

    #[test]
    fn tab_confirms_inline_skill_popup_through_shared_key_handler() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$dem".to_owned());
        app.sync_inline_skill_popup();

        assert!(
            app.handle_inline_skill_popup_key(crossterm::event::KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::NONE,
            ))
        );

        assert_eq!(app.composer.text(), "$demo-skill ");
        assert_eq!(app.focus, Focus::Composer);
        assert!(!app.inline_skill_popup_active);
    }

    #[test]
    fn confirming_inline_skill_keeps_surrounding_text_stable() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("please $dem now".to_owned());
        for _ in 0..4 {
            let _ = app.composer.handle_key(crossterm::event::KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::NONE,
            ));
        }
        app.sync_inline_skill_popup();

        app.confirm_inline_skill_popup();

        assert_eq!(app.composer.text(), "please $demo-skill now");
    }

    #[test]
    fn confirming_inline_skill_works_with_cursor_inside_token_middle() {
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$demo now".to_owned());
        for _ in 0..4 {
            let _ = app.composer.handle_key(crossterm::event::KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::NONE,
            ));
        }
        app.sync_inline_skill_popup();

        app.confirm_inline_skill_popup();

        assert_eq!(app.composer.text(), "$demo-skill now");
    }

    #[test]
    fn inline_skill_popup_mouse_click_works_while_composer_keeps_focus() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$dem".to_owned());
        app.sync_inline_skill_popup();

        terminal.draw(|f| app.render(f)).expect("draw");
        let palette_row = app.last_palette_area.y;
        let palette_col = app.last_palette_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            palette_col,
            palette_row,
        ));

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "$demo-skill ");
    }

    #[test]
    fn inline_skill_popup_mouse_scroll_updates_selection_while_composer_stays_focused() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(
            Language::En,
            vec![skill("demo-skill"), skill("other-skill")],
        );
        app.composer.set_input("$".to_owned());
        app.sync_inline_skill_popup();

        terminal.draw(|f| app.render(f)).expect("draw");
        let palette_row = app.last_palette_area.y;
        let palette_col = app.last_palette_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, palette_col, palette_row));
        app.confirm_inline_skill_popup();

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "$other-skill ");
    }

    #[test]
    fn mouse_scroll_routes_to_transcript_even_with_a_draft() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..14 {
            app.message_list
                .add_assistant_message(format!("line-{idx}"));
        }
        app.composer.set_input("draft".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let before = buffer_lines(&terminal).join("\n");

        let transcript_row = app.last_transcript_area.y.saturating_add(1);
        let transcript_col = app.last_transcript_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::ScrollUp,
            transcript_col,
            transcript_row,
        ));

        terminal.draw(|f| app.render(f)).expect("draw after scroll");
        let after = buffer_lines(&terminal).join("\n");

        assert!(app.message_list.scroll_offset > 0);
        assert_ne!(before, after);
        assert_eq!(app.focus, Focus::MessageList);
    }

    #[test]
    fn history_browse_mode_hides_composer_and_follow_footer_when_scrolled_up() {
        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..14 {
            app.message_list
                .add_assistant_message(format!("line-{idx}"));
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        ));
        terminal.draw(|f| app.render(f)).expect("draw off tail");
        let lines = buffer_lines(&terminal).join("\n");

        assert!(!lines.contains("PgDn / End"));
        assert!(!lines.contains("/tmp/example"));
        assert!(!lines.contains(" › "));
    }

    #[test]
    fn footer_returns_to_status_line_when_tail_is_restored() {
        let backend = TestBackend::new(50, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..14 {
            app.message_list
                .add_assistant_message(format!("line-{idx}"));
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        ));
        terminal.draw(|f| app.render(f)).expect("draw off tail");
        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        terminal
            .draw(|f| app.render(f))
            .expect("draw tail restored");
        let lines = buffer_lines(&terminal).join("\n");

        assert!(lines.contains("/tmp/example"));
        assert!(!lines.contains("PgDn / End"));
    }

    #[test]
    fn transcript_typing_restores_tail_before_editing_composer() {
        let mut app = blank_app();
        for idx in 0..14 {
            app.message_list
                .add_assistant_message(format!("line-{idx}"));
        }
        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        ));
        app.focus = Focus::MessageList;

        let submitted = super::route_transcript_key_to_composer(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        );

        assert!(submitted.is_none());
        assert!(app.message_list.is_following_tail());
        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "a");
    }

    #[test]
    fn mouse_scroll_over_palette_changes_selection_without_scrolling_transcript() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..10 {
            app.message_list
                .add_assistant_message(format!("line-{idx}"));
        }
        app.message_list.scroll_offset = 4;
        app.command_palette.show_commands(":");
        app.focus = Focus::CommandPalette;

        terminal.draw(|f| app.render(f)).expect("draw");
        let palette_row = app.last_palette_area.y.saturating_add(1);
        let palette_col = app.last_palette_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, palette_col, palette_row));

        assert_eq!(app.message_list.scroll_offset, 4);
        match app
            .command_palette
            .handle_key(crossterm::event::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )) {
            Some(CommandAction::RunCommand(command)) if command == "/permissions" => {}
            other => {
                panic!("expected palette mouse scroll to land on /permissions, got {other:?}")
            }
        }
    }

    #[test]
    fn mouse_clicking_skill_palette_inserts_into_composer() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.command_palette.show_skills("$demo");
        app.focus = Focus::CommandPalette;

        terminal.draw(|f| app.render(f)).expect("draw");
        let palette_row = app.last_palette_area.y;
        let palette_col = app.last_palette_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            palette_col,
            palette_row,
        ));

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.take_input(), "$demo-skill ");
    }

    #[test]
    fn mouse_clicking_composer_restores_focus() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.focus = Focus::MessageList;

        terminal.draw(|f| app.render(f)).expect("draw");
        let composer_row = app.last_composer_area.y;
        let composer_col = app.last_composer_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            composer_col,
            composer_row,
        ));

        assert_eq!(app.focus, Focus::Composer);
    }

    #[test]
    fn transcript_click_closes_inline_skill_popup_after_focus_change() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$dem".to_owned());
        app.sync_inline_skill_popup();
        app.message_list.add_assistant_message("line-0".to_owned());
        app.message_list.add_assistant_message("line-1".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let transcript_row = app.last_transcript_area.y.saturating_add(1);
        let transcript_col = app.last_transcript_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            transcript_col,
            transcript_row,
        ));

        assert_eq!(app.focus, Focus::MessageList);
        assert!(!app.inline_skill_popup_active);
    }

    #[test]
    fn composer_click_reopens_inline_skill_popup_after_transcript_focus() {
        let backend = TestBackend::new(50, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.command_palette = CommandPalette::new(Language::En, vec![skill("demo-skill")]);
        app.composer.set_input("$dem".to_owned());
        app.focus = Focus::MessageList;
        app.message_list.add_assistant_message("line-0".to_owned());
        app.sync_inline_skill_popup();

        terminal.draw(|f| app.render(f)).expect("draw");
        let composer_row = app.last_composer_area.y;
        let composer_col = app.last_composer_area.x.saturating_add(1);
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            composer_col,
            composer_row,
        ));

        assert_eq!(app.focus, Focus::Composer);
        assert!(app.inline_skill_popup_active);
    }

    #[test]
    fn startup_eye_animation_defaults_to_ambient_when_shell_is_idle() {
        let app = blank_app();

        assert_eq!(
            app.startup_eye_animation(),
            StartupEyeAnimation::Focus(StartupEyeFocus::DownLeft)
        );
    }

    #[test]
    fn startup_eye_animation_tracks_composer_and_palette_focus() {
        let mut app = blank_app();
        app.composer.set_input("hello".to_owned());
        assert_eq!(
            app.startup_eye_animation(),
            StartupEyeAnimation::Focus(StartupEyeFocus::DownCenter)
        );

        app.focus = Focus::CommandPalette;
        app.command_palette.show_commands("/");
        assert_eq!(
            app.startup_eye_animation(),
            StartupEyeAnimation::Thinking(StartupEyeFocus::DownLeft)
        );

        app.command_palette.show_skills("$");
        assert_eq!(
            app.startup_eye_animation(),
            StartupEyeAnimation::Focus(StartupEyeFocus::DownLeft)
        );
    }

    #[test]
    fn startup_eye_animation_tracks_palette_selection_depth() {
        assert_eq!(startup_palette_eye_focus(0, 9), StartupEyeFocus::DownLeft);
        assert_eq!(startup_palette_eye_focus(4, 9), StartupEyeFocus::DownCenter);
        assert_eq!(startup_palette_eye_focus(8, 9), StartupEyeFocus::DownRight);
    }

    #[test]
    fn startup_eye_animation_prefers_transient_feedback() {
        let mut app = blank_app();
        app.set_startup_eye_feedback(
            StartupEyeAnimation::Confirm(StartupEyeFocus::DownCenter),
            Duration::from_millis(250),
        );

        assert_eq!(
            app.startup_eye_animation(),
            StartupEyeAnimation::Confirm(StartupEyeFocus::DownCenter)
        );
    }

    #[test]
    fn startup_tip_leaves_blank_row_before_composer_separator() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_startup_header_with_tips(
            "0.1.0".to_owned(),
            "fallback".to_owned(),
            vec![
                ("Skills".to_owned(), vec!["0".to_owned()]),
                ("MCP".to_owned(), vec!["1".to_owned()]),
            ],
            vec!["rotating tip".to_owned()],
        );

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal);
        let composer_separator_row = app.last_composer_area.y.saturating_sub(1) as usize;
        let blank_row_before_separator = composer_separator_row.saturating_sub(1);

        assert!(lines.iter().any(|line| line.contains("rotating tip")));
        assert!(
            lines
                .get(blank_row_before_separator)
                .is_some_and(|line| line.trim().is_empty())
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
            vec![("MCP".to_owned(), vec!["0".to_owned()])],
        );
        app.message_list.add_user_message("hi".to_owned());
        app.message_list.add_assistant_message("hello".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        let lines = buffer_lines(&terminal).join("\n");
        assert!(lines.contains("0.1.0"));
        assert!(lines.contains("MCP (0)"));
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
            I18nService::new(Language::En).spinner_verbs(),
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
    fn pending_preview_styles_tool_activity_without_flattening_it_into_plain_text() {
        let lines = super::build_pending_lines(
            Some(std::time::Instant::now()),
            &[
                "• Called read_file · working".to_owned(),
                "  ↳ stderr 1 lines · 42 bytes".to_owned(),
                "    - denied".to_owned(),
            ],
            1,
            I18nService::new(Language::En).spinner_verbs(),
            &std::collections::VecDeque::new(),
            &std::collections::VecDeque::new(),
            72,
        );

        let called_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.as_ref() == "Called ")
            })
            .expect("called line");
        let called_label = called_line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "Called ")
            .expect("called label");
        assert!(
            super::PENDING_TOOL_LABEL_COLORS.contains(
                &called_label
                    .style
                    .fg
                    .expect("called label should have an animated foreground"),
            )
        );
        assert!(
            called_label
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );

        let stderr_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.as_ref() == "stderr ")
            })
            .expect("stderr line");
        let stderr_label = stderr_line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "stderr ")
            .expect("stderr label");
        assert_eq!(stderr_label.style.fg, Some(super::SURFACE_RED));

        let sample_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .contains("- denied")
            })
            .expect("sample line");
        let sample_span = sample_line
            .spans
            .iter()
            .find(|span| {
                let content = span.content.as_ref();
                content == "-" || content == "denied"
            })
            .expect("sample span");
        assert_eq!(sample_span.style.fg, Some(super::SURFACE_RED));
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
    fn pending_preview_keeps_blank_row_between_live_lines_and_spinner() {
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

        assert_eq!(spinner_row, preview_row + 2);
        assert!(lines[preview_row + 1].trim().is_empty());
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
            crate::chat::chat_surface::utils::SURFACE_GRAY
        );
        assert_eq!(buf[(2, visible_row)].fg, ratatui::style::Color::White);
    }

    #[test]
    fn pending_preview_key_value_lines_use_structured_styles() {
        let backend = TestBackend::new(70, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["- streaming renderer: enabled".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let text_lines = buffer_lines(&terminal);
        let row = text_lines
            .iter()
            .position(|line| line.contains("streaming renderer"))
            .expect("key value row") as u16;
        let key_col = text_lines[row as usize]
            .find("streaming renderer:")
            .expect("key col") as u16;
        let value_col = text_lines[row as usize].find("enabled").expect("value col") as u16;
        let buf = terminal.backend().buffer();

        assert_eq!(buf[(key_col, row)].fg, super::SURFACE_ACCENT);
        assert_eq!(buf[(value_col, row)].fg, ratatui::style::Color::White);
    }

    #[test]
    fn pending_preview_highlights_inline_control_tokens() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["Use /language or type $skill now.".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let text_lines = buffer_lines(&terminal);
        let row = text_lines
            .iter()
            .position(|line| line.contains("/language"))
            .expect("instruction row") as u16;
        let slash_col = text_lines[row as usize]
            .find("/language")
            .expect("slash col") as u16;
        let skill_col = text_lines[row as usize].find("$skill").expect("skill col") as u16;
        let buf = terminal.backend().buffer();

        assert_eq!(buf[(slash_col, row)].fg, super::SURFACE_ACCENT);
        assert_eq!(buf[(skill_col, row)].fg, super::SURFACE_ACCENT);
    }

    #[test]
    fn pending_tool_sample_neutral_lines_use_readable_gray() {
        let lines =
            super::render_pending_tool_sample_line("    context line", 40).expect("sample lines");
        let sample_spans = lines[0]
            .spans
            .iter()
            .filter(|span| {
                let content = span.content.as_ref();
                content.contains("context") || content.contains("line")
            })
            .collect::<Vec<_>>();

        assert!(!sample_spans.is_empty(), "sample spans");
        assert!(
            sample_spans
                .iter()
                .all(|span| span.style.fg == Some(super::SURFACE_GRAY))
        );
    }

    #[test]
    fn pending_tool_sample_lines_highlight_inline_control_tokens() {
        let lines = super::render_pending_tool_sample_line("    use /review after Ctrl+C", 48)
            .expect("sample lines");
        let slash_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("/review"))
            .expect("slash span");
        let ctrl_c_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("Ctrl+C"))
            .expect("ctrl c span");

        assert_eq!(slash_span.style.fg, Some(super::SURFACE_ACCENT));
        assert!(slash_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(ctrl_c_span.style.fg, Some(super::SURFACE_CYAN));
        assert!(ctrl_c_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn pending_tool_child_body_uses_readable_gray() {
        let (_label, body_style) = super::pending_tool_child_styles("request");
        assert_eq!(body_style.fg, Some(super::SURFACE_GRAY));
    }

    #[test]
    fn pending_tool_child_lines_highlight_inline_control_tokens() {
        let backend = TestBackend::new(90, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_user_message("hi".to_owned());
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["↳ request use /permissions before $skill".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        let text_lines = buffer_lines(&terminal);
        let row = text_lines
            .iter()
            .position(|line| line.contains("/permissions"))
            .expect("request row") as u16;
        let slash_col = text_lines[row as usize]
            .find("/permissions")
            .expect("slash col") as u16;
        let skill_col = text_lines[row as usize].find("$skill").expect("skill col") as u16;
        let buf = terminal.backend().buffer();

        assert_eq!(buf[(slash_col, row)].fg, super::SURFACE_ACCENT);
        assert_eq!(buf[(skill_col, row)].fg, super::SURFACE_ACCENT);
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
        assert!(rendered.contains("reply-2"));
        assert!(!rendered.contains("reply-3"));
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
    fn transcript_navigation_key_helper_keeps_printable_keys_for_composer() {
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE,)
        ));
        assert!(super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Home, KeyModifiers::NONE,)
        ));
        assert!(!super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE,)
        ));
        assert!(!super::is_transcript_navigation_key(
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE,)
        ));
    }

    #[test]
    fn transcript_focus_text_keys_enter_composer_immediately() {
        let mut app = blank_app();
        app.focus = Focus::MessageList;

        let submitted = super::route_transcript_key_to_composer(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE),
        );

        assert!(submitted.is_none());
        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "你");
    }

    #[test]
    fn paste_event_always_restores_composer_focus_and_inserts_text() {
        let mut app = blank_app();
        app.focus = Focus::MessageList;

        super::paste_into_composer(&mut app, "alpha\r\nbeta");

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "alpha\nbeta");
        assert!(!app.composer_follow_up_intent);
    }

    #[test]
    fn paste_event_marks_pending_draft_as_follow_up() {
        let mut app = blank_app();
        app.focus = Focus::CommandPalette;
        app.pending_turn = true;

        super::paste_into_composer(&mut app, "queued follow-up");

        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.composer.text(), "queued follow-up");
        assert!(app.composer_follow_up_intent);
    }

    #[test]
    fn transcript_focus_enter_submits_existing_draft() {
        let mut app = blank_app();
        app.focus = Focus::MessageList;
        app.composer.set_input("send me".to_owned());

        let submitted = super::route_transcript_key_to_composer(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert_eq!(submitted.as_deref(), Some("send me"));
        assert_eq!(app.focus, Focus::Composer);
        assert!(app.composer.is_empty());
    }

    #[test]
    fn transcript_focus_capture_helper_rejects_navigation_and_modified_keys() {
        assert!(super::should_focus_composer_for_transcript_key(
            crossterm::event::KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE,)
        ));
        assert!(super::should_focus_composer_for_transcript_key(
            crossterm::event::KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE,)
        ));
        assert!(super::should_focus_composer_for_transcript_key(
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE,)
        ));
        assert!(!super::should_focus_composer_for_transcript_key(
            crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE,)
        ));
        assert!(!super::should_focus_composer_for_transcript_key(
            crossterm::event::KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL,)
        ));
    }

    #[test]
    fn composer_routes_arrow_and_page_scroll_even_with_a_draft() {
        let mut app = blank_app();
        app.composer.set_input("draft".to_owned());

        assert!(super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::Up, KeyModifiers::NONE,)
        ));
        assert!(super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::Down, KeyModifiers::NONE,)
        ));
        assert!(super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE,)
        ));
        assert!(super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE,)
        ));
        assert!(!super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE,)
        ));
        assert!(!super::should_route_composer_key_to_transcript(
            &app,
            crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE,)
        ));
    }

    #[test]
    fn submitted_message_is_not_treated_as_follow_up_after_pending_turn_finishes() {
        let mut app = blank_app();
        app.composer_follow_up_intent = true;

        assert!(!super::submitted_message_is_follow_up(&app, "follow up"));

        app.pending_turn = true;
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
    fn width_resize_keeps_provider_error_and_footer_visible() {
        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.message_list.add_assistant_message(
            "[provider_error] provider returned status 401 for model `gpt-5.4` on attempt 1/3: {\"code\":\"INVALID_API_KEY\",\"message\":\"Invalid API key\"} | provider_failover={\"reason\":\"auth_rejected\",\"stage\":\"status_failure\",\"model\":\"gpt-5.4\",\"attempt\":1,\"max_attempts\":3,\"status_code\":401}".to_owned(),
        );

        terminal.draw(|f| app.render(f)).expect("draw");
        terminal.backend_mut().resize(28, 18);
        terminal.draw(|f| app.render(f)).expect("draw");

        let lines = buffer_lines(&terminal);
        let provider_row = lines
            .iter()
            .position(|line| line.contains("provider error"))
            .expect("provider error row");
        let detail_row = lines
            .iter()
            .position(|line| line.contains("INVALID_API_KEY"))
            .expect("provider error detail row");
        let footer_row = lines
            .iter()
            .position(|line| line.contains("gpt-test"))
            .expect("footer row");

        assert!(provider_row < detail_row);
        assert!(detail_row < footer_row);
        assert!(footer_row > detail_row);
        assert!(lines.iter().any(|line| line.contains("401")));
    }

    #[test]
    fn width_resize_keeps_pending_restore_footer_and_previews_visible() {
        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        app.pending_steers
            .push_back("nudge the current answer toward the root cause".to_owned());
        app.pending_queue
            .push_back("after that, summarize the diff and keep the footer visible".to_owned());

        terminal.draw(|f| app.render(f)).expect("draw");
        terminal.backend_mut().resize(34, 18);
        terminal.draw(|f| app.render(f)).expect("draw");

        let lines = buffer_lines(&terminal);
        let steer_row = lines
            .iter()
            .position(|line| line.contains("root cause"))
            .expect("steer preview row");
        let queue_header_row = lines
            .iter()
            .position(|line| line.contains("Queued follow-up messages"))
            .expect("queued header row");
        let queued_row = lines
            .iter()
            .enumerate()
            .skip(queue_header_row + 1)
            .find_map(|(idx, line)| line.contains("↳").then_some(idx))
            .expect("queued preview row");
        let composer_row = lines
            .iter()
            .position(|line| line.contains("›"))
            .expect("composer row");
        let footer_row = lines
            .iter()
            .position(|line| line.contains("Option + Up") || line.contains("Alt + Up"))
            .expect("restore footer row");

        assert!(steer_row < queue_header_row);
        assert!(queue_header_row < queued_row);
        assert!(queued_row < composer_row);
        assert!(composer_row < footer_row);
        assert!(lines[queued_row].contains("↳"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Option + Up") || line.contains("Alt + Up"))
        );
    }

    #[test]
    fn off_tail_pending_resize_and_end_restore_tail_without_losing_state() {
        let backend = TestBackend::new(48, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = blank_app();
        for idx in 0..18 {
            app.message_list.add_assistant_message(format!(
                "line-{idx} keeps transcript stable while pending preview and resize interact"
            ));
        }

        terminal.draw(|f| app.render(f)).expect("draw");
        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE,
        ));
        app.pending_turn = true;
        app.turn_start = Some(std::time::Instant::now());
        if let Ok(mut lines) = app.live_lines.lock() {
            *lines = vec!["streamed preview line".to_owned()];
        }

        terminal.draw(|f| app.render(f)).expect("draw off tail");
        let off_tail_lines = buffer_lines(&terminal).join("\n");
        assert!(!off_tail_lines.contains("PgDn / End"));
        assert!(off_tail_lines.contains("streamed preview line"));

        app.message_list
            .add_assistant_message("new-tail-line after scroll".to_owned());
        terminal.backend_mut().resize(34, 18);
        terminal.draw(|f| app.render(f)).expect("draw resized");
        let resized_lines = buffer_lines(&terminal).join("\n");
        assert!(!resized_lines.contains("PgDn / End"));
        assert!(resized_lines.contains("streamed preview line"));

        app.message_list.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        terminal.draw(|f| app.render(f)).expect("draw restored");
        let restored_lines = buffer_lines(&terminal).join("\n");

        assert!(restored_lines.contains("new-tail-line after scroll"));
        assert!(restored_lines.contains("streamed preview line"));
        assert!(!restored_lines.contains("PgDn / End"));
        assert_eq!(app.message_list.scroll_offset, 0);
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
            row_has_background(&terminal, user_row - 1, SURFACE_USER_MSG_BG),
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
        let pending_row = find_row(&terminal, "...")
            .or_else(|| find_row(&terminal, "中"))
            .unwrap_or(0);

        assert!(row_has_background(
            &terminal,
            user_row + 1,
            SURFACE_USER_MSG_BG
        ));
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

        assert!(row_has_background(
            &terminal,
            user_row - 1,
            SURFACE_USER_MSG_BG
        ));
        assert!(preview_row > user_row);
        assert!(preview_row < composer_row);
    }
}
