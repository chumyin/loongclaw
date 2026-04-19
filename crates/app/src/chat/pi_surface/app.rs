use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
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
    pub cwd: String,
    pub model: String,
}

impl App {
    pub fn new(
        runtime: &CliTurnRuntime,
        options: &CliChatOptions,
        render_width: usize,
    ) -> CliResult<Self> {
        let mut app = Self {
            message_list: MessageList::new(),
            composer: Composer::new(),
            command_palette: CommandPalette::new(),
            focus: Focus::Composer,
            pending_turn: false,
            turn_start: None,
            live_lines: Arc::new(StdMutex::new(Vec::new())),
            pending_task: None,
            cwd: format_cwd(runtime),
            model: runtime.config.provider.model.clone(),
        };

        let (version, tutorial, sections) =
            build_pi_startup_content(runtime, options, render_width);
        app.message_list
            .add_startup_header(version, tutorial, sections);

        Ok(app)
    }

    pub fn render(&mut self, f: &mut Frame) {
        let size = f.area();
        let pending_lines = if self.pending_turn {
            build_pending_lines(self.turn_start, &self.live_lines, size.width)
        } else {
            Vec::new()
        };
        let pending_height = pending_lines.len() as u16;
        let palette_height = if matches!(self.focus, Focus::CommandPalette) {
            self.command_palette.desired_height() as u16
        } else {
            0
        };
        let fixed_height = pending_height
            + 1
            + self.composer.height()
            + if palette_height > 0 {
                1 + palette_height
            } else {
                0
            }
            + 1
            + 1;
        let available_transcript_height = size.height.saturating_sub(fixed_height).max(1);
        let transcript_height = self
            .message_list
            .rendered_line_count(size.width)
            .min(available_transcript_height as usize) as u16;

        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(transcript_height),
                Constraint::Length(pending_height),
                Constraint::Length(1),
                Constraint::Length(self.composer.height()),
                Constraint::Length(if palette_height > 0 { 1 } else { 0 }),
                Constraint::Length(palette_height),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(size);

        self.message_list.render(f, main_layout[0]);

        if self.pending_turn {
            f.render_widget(Paragraph::new(pending_lines), main_layout[1]);
        }

        let line_color = PI_COTTON_CANDY;
        f.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(line_color)),
            main_layout[2],
        );

        self.composer
            .render(f, main_layout[3], matches!(self.focus, Focus::Composer));
        if matches!(self.focus, Focus::Composer) {
            let (x, y) = self.composer.cursor_position(main_layout[3]);
            f.set_cursor_position((x, y));
        }

        if palette_height > 0 {
            f.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(line_color)),
                main_layout[4],
            );
            self.command_palette.render(f, main_layout[5]);
        }

        f.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(line_color)),
            main_layout[6],
        );

        let spaces = size
            .width
            .saturating_sub(self.cwd.len() as u16 + self.model.len() as u16);
        let footer_line = Line::from(vec![
            Span::styled(&self.cwd, Style::default().fg(PI_GRAY)),
            Span::raw(" ".repeat(spaces as usize)),
            Span::styled(&self.model, Style::default().fg(PI_GRAY)),
        ]);
        f.render_widget(Paragraph::new(footer_line), main_layout[7]);
    }
}

pub async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    runtime: CliTurnRuntime,
    options: CliChatOptions,
) -> CliResult<()> {
    let render_width = terminal
        .size()
        .map_err(|e| format!("failed to query terminal size: {e}"))?
        .width as usize;
    let mut app = App::new(&runtime, &options, render_width)?;

    loop {
        maybe_finalize_pending_turn(terminal, &mut app).await?;

        terminal
            .draw(|f| app.render(f))
            .map_err(|e| format!("draw error: {}", e))?;

        if event::poll(Duration::from_millis(40)).map_err(|e| format!("poll error: {}", e))? {
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
                        match app.focus {
                            Focus::Composer => {
                                if key.code == KeyCode::Char('/') && app.composer.is_empty() {
                                    app.command_palette.show("/");
                                    app.focus = Focus::CommandPalette;
                                } else if key.code == KeyCode::Tab {
                                    app.focus = Focus::MessageList;
                                } else if !(key.code == KeyCode::Enter
                                    && !key.modifiers.contains(KeyModifiers::SHIFT))
                                {
                                    let _ = app.composer.handle_key(key);
                                }
                            }
                            Focus::MessageList => {
                                if key.code == KeyCode::Char('/') && app.composer.is_empty() {
                                    app.command_palette.show("/");
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
                        if let Some(command) = pending_command {
                            if command == "/exit" {
                                break;
                            }
                            run_surface_command(terminal, &mut app, &runtime, &options, &command)
                                .await?;
                        }
                        continue;
                    }

                    let mut command_to_run = None;
                    let mut submitted_message = None;

                    match app.focus {
                        Focus::Composer => {
                            if key.code == KeyCode::Esc {
                                if !app.composer.is_empty() {
                                    app.composer.clear();
                                }
                            } else if key.code == KeyCode::Char('/') && app.composer.is_empty() {
                                app.command_palette.show("/");
                                app.focus = Focus::CommandPalette;
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

                        if msg.starts_with('/') {
                            app.command_palette.show(&msg);
                            app.focus = Focus::CommandPalette;
                            continue;
                        }

                        submit_user_turn(terminal, &mut app, &runtime, msg).await?;
                    } else if let Some(command) = command_to_run {
                        if command == "/exit" {
                            break;
                        }

                        run_surface_command(terminal, &mut app, &runtime, &options, &command)
                            .await?;
                    }
                }
                Event::Mouse(mouse_event) => {
                    app.message_list.handle_mouse(mouse_event);
                }
                Event::Resize(_, _) => {}
                _ => {}
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
    let width = current_render_width(terminal)?;
    app.message_list.add_user_message(input.clone());
    app.pending_turn = true;
    app.turn_start = Some(std::time::Instant::now());
    app.focus = Focus::Composer;
    clear_live_lines(&app.live_lines);

    terminal
        .draw(|f| app.render(f))
        .map_err(|e| format!("draw error: {}", e))?;

    app.pending_task = Some(spawn_pending_turn(
        runtime.clone(),
        input,
        width,
        app.live_lines.clone(),
    ));
    Ok(())
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
                "usage: /help | /status | /history | /compact | /fast_lane_summary | /safe_lane_summary | /turn_checkpoint_summary | /turn_checkpoint_repair | /sessions | /workers | /review | /mission | /exit",
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
    if let Some(primary) = sessions.first() {
        if let Some(details) = store.session_details(&primary.session_id, false)? {
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
    if let Some(primary) = workers.first() {
        if let Some(details) = store.session_details(&primary.session_id, true)? {
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
) -> CliResult<()> {
    let Some(handle) = app.pending_task.as_ref() else {
        return Ok(());
    };
    if !handle.is_finished() {
        return Ok(());
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
    clear_live_lines(&app.live_lines);
    app.focus = Focus::Composer;
    if super::super::build_cli_chat_approval_screen_spec(&assistant_text).is_some() {
        app.message_list.add_rendered_lines(
            super::super::render_cli_chat_assistant_lines_with_width(&assistant_text, width),
        );
    } else {
        app.message_list.add_assistant_message(assistant_text);
    }
    Ok(())
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
    render_width: usize,
    live_lines: Arc<StdMutex<Vec<String>>>,
) -> JoinHandle<CliResult<String>> {
    tokio::spawn(async move {
        let sink = Arc::new(move |lines: Vec<String>| {
            if let Ok(mut state) = live_lines.lock() {
                *state = lines;
            }
        });
        let observer =
            super::super::build_cli_chat_live_surface_observer_with_sink(render_width, sink);
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

fn build_pending_lines(
    turn_start: Option<std::time::Instant>,
    live_lines: &Arc<StdMutex<Vec<String>>>,
    width: u16,
) -> Vec<Line<'static>> {
    let start = turn_start.unwrap_or_else(std::time::Instant::now);
    let mut pending_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{} ", focus_ring_frame(start)),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}...", get_spinner_verb(start)),
                Style::default().fg(PI_GRAY),
            ),
        ]),
    ];

    let snapshot = live_lines
        .lock()
        .map(|state| state.clone())
        .unwrap_or_default();
    if snapshot.is_empty() {
        return pending_lines;
    }

    let wrap_width = width.saturating_sub(2).max(1) as usize;
    for preview_line in snapshot
        .iter()
        .filter(|line| !line.trim().is_empty())
        .take(3)
    {
        for wrapped in crate::presentation::render_wrapped_display_line(preview_line, wrap_width) {
            pending_lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(wrapped, Style::default().fg(PI_DIM_GRAY)),
            ]));
        }
    }

    pending_lines
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

    let tutorial = "escape interrupt · / commands · ctrl+o compaction".to_owned();
    let mut sections = vec![
        ("MCP".to_owned(), mcp_servers),
        ("Skills".to_owned(), skills),
    ];

    if options.acp_event_stream || runtime.explicit_acp_request {
        sections.push((
            "ACP".to_owned(),
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
