use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::chat_surface::markdown;
use super::chat_surface::utils::{
    REQUEST_COMMAND_KEYS, REQUEST_GLOB_KEYS, REQUEST_HIDDEN_KEYS, REQUEST_SCOPE_KEYS,
    REQUEST_TEXT_KEYS, compact_structured_preview, format_glob_summary,
    format_inspect_secondary_details, format_list_summary, format_read_summary,
    format_search_summary, is_glob_tool_name, is_list_tool_name, is_read_tool_name,
    is_run_tool_name, is_search_tool_name, normalized_tool_name, request_bool_from_candidates,
    request_numeric_from_candidates, request_path_from_candidates, request_string_from_candidates,
    shorten_display_path, split_inline_list_runs,
};
use super::*;

const CLI_CHAT_LIVE_PREVIEW_MIN_EMIT_CHARS: usize = 8;
const CLI_CHAT_LIVE_PREVIEW_MAX_EMIT_CHARS: usize = 48;
const CLI_CHAT_LIVE_PREVIEW_INITIAL_EMIT_CHARS: usize = 4;
const CLI_CHAT_LIVE_PREVIEW_MAX_BUFFER_CHARS: usize = 4096;
const CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS: usize = 1024;
const CLI_CHAT_LIVE_OUTPUT_MAX_BUFFER_CHARS: usize = 1536;
const CLI_CHAT_LIVE_OUTPUT_RENDER_MAX_LINES: usize = 4;
const CLI_CHAT_LIVE_DIFF_PREVIEW_MAX_LINES: usize = 6;
const CLI_CHAT_LIVE_PREVIEW_CATCH_UP_ENTER_VISUAL_LINE_GROWTH: usize = 2;
const CLI_CHAT_LIVE_PREVIEW_CATCH_UP_ENTER_MIN_VISUAL_LINES: usize = 4;
const CLI_CHAT_LIVE_PREVIEW_SMOOTH_MIN_INTERVAL_MS: u64 = 40;
const CLI_CHAT_LIVE_PREVIEW_CATCH_UP_MIN_INTERVAL_MS: u64 = 16;
pub(super) type CliChatLiveSurfaceSink = Arc<dyn Fn(Vec<String>) + Send + Sync>;
pub(super) type CliChatLiveSurfaceRerender = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliChatLiveSurfaceRenderMode {
    Card,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliChatLivePreviewEmitMode {
    Smooth,
    CatchUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveOutputView {
    pub text: String,
    pub total_bytes: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

impl CliChatLiveOutputView {
    fn new() -> Self {
        Self {
            text: String::new(),
            total_bytes: 0,
            total_lines: 0,
            truncated: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveFileChangeView {
    pub path: String,
    pub operation: ToolFileChangeKind,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveToolSnapshot {
    pub tool_call_id: String,
    pub name: Option<String>,
    pub request_summary: Option<String>,
    pub args: String,
    pub status: ConversationTurnToolState,
    pub detail: Option<String>,
    pub stdout: CliChatLiveOutputView,
    pub stderr: CliChatLiveOutputView,
    pub file_change: Option<CliChatLiveFileChangeView>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliChatLiveOutputPreviewMode {
    Head,
    Tail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveSnapshotPreview {
    text: String,
    blocks: Vec<CliChatLivePreviewBlock>,
}

impl CliChatLiveSnapshotPreview {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn new(text: String, blocks: Vec<CliChatLivePreviewBlock>) -> Self {
        Self { text, blocks }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn from_text(text: String) -> Self {
        Self {
            text,
            blocks: Vec::new(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn from_blocks(blocks: Vec<CliChatLivePreviewBlock>) -> Self {
        Self {
            text: String::new(),
            blocks,
        }
    }

    fn from_parts(
        text: Option<String>,
        blocks: Option<Vec<CliChatLivePreviewBlock>>,
    ) -> Option<Self> {
        (text.is_some() || blocks.is_some()).then(|| Self {
            text: text.unwrap_or_default(),
            blocks: blocks.unwrap_or_default(),
        })
    }

    fn text(&self) -> Option<&str> {
        (!self.text.trim().is_empty()).then_some(self.text.as_str())
    }

    fn blocks(&self) -> Option<&[CliChatLivePreviewBlock]> {
        (!self.blocks.is_empty()).then_some(self.blocks.as_slice())
    }

    fn effective_preview(&self) -> Option<CliChatLiveEffectivePreview<'_>> {
        let preview = self.text().unwrap_or_default();
        let blocks = self.blocks().unwrap_or(&[]);
        let effective_preview = cli_chat_live_effective_preview(preview, blocks, None);
        (!effective_preview.blocks.is_empty()).then_some(effective_preview)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveSurfaceSnapshot {
    pub phase: ConversationTurnPhase,
    pub provider_round: Option<usize>,
    pub lane: Option<ExecutionLane>,
    pub tool_call_count: usize,
    pub message_count: Option<usize>,
    pub estimated_tokens: Option<usize>,
    pub first_token_latency_ms: Option<u64>,
    pub preview: Option<CliChatLiveSnapshotPreview>,
    pub tools: Vec<CliChatLiveToolSnapshot>,
}

impl CliChatLiveSurfaceSnapshot {
    fn effective_preview(&self) -> Option<CliChatLiveEffectivePreview<'_>> {
        self.preview
            .as_ref()
            .and_then(CliChatLiveSnapshotPreview::effective_preview)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLiveToolState {
    pub tool_call_id: String,
    pub display_order: usize,
    pub name: Option<String>,
    pub request_summary: Option<String>,
    pub args: String,
    pub status: ConversationTurnToolState,
    pub detail: Option<String>,
    pub stdout: CliChatLiveOutputView,
    pub stderr: CliChatLiveOutputView,
    pub file_change: Option<CliChatLiveFileChangeView>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

impl CliChatLiveToolState {
    fn new(tool_call_id: String, display_order: usize) -> Self {
        let stdout = CliChatLiveOutputView::new();
        let stderr = CliChatLiveOutputView::new();

        Self {
            tool_call_id,
            display_order,
            name: None,
            request_summary: None,
            args: String::new(),
            status: ConversationTurnToolState::Running,
            detail: None,
            stdout,
            stderr,
            file_change: None,
            duration_ms: None,
            exit_code: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct CliChatLivePreviewCache {
    preview_buffer: String,
    preview_blocks: Vec<CliChatLivePreviewBlock>,
    preview_display_text: String,
    preview_display_char_count: usize,
    preview_parse_kind: CliChatLivePreviewBlockKind,
    preview_tag_carry: String,
    emit: CliChatLivePreviewEmitCache,
}

impl CliChatLivePreviewCache {
    fn clear(&mut self) {
        self.preview_buffer.clear();
        self.preview_blocks.clear();
        self.preview_display_text.clear();
        self.preview_display_char_count = 0;
        self.preview_parse_kind = CliChatLivePreviewBlockKind::Visible;
        self.preview_tag_carry.clear();
    }

    fn snapshot_preview(&self) -> Option<CliChatLiveSnapshotPreview> {
        let preview = self.effective_preview_or_reparse();
        let text = (!self.preview_display_text.trim().is_empty())
            .then(|| self.preview_display_text.clone())
            .or_else(|| {
                (!preview.display_text.trim().is_empty()).then(|| preview.display_text.clone())
            });
        let blocks = (!preview.blocks.is_empty()).then(|| preview.blocks.into_owned());
        CliChatLiveSnapshotPreview::from_parts(text, blocks)
    }

    fn effective_preview(&self) -> Option<CliChatLiveEffectivePreview<'_>> {
        if self.preview_blocks.is_empty() && self.preview_display_text.trim().is_empty() {
            return None;
        }

        let display_text = if self.preview_display_text.is_empty() {
            self.preview_blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<String>()
        } else {
            self.preview_display_text.clone()
        };
        let display_char_count = if self.preview_display_char_count > 0 || display_text.is_empty() {
            self.preview_display_char_count
        } else {
            display_text.chars().count()
        };
        let has_complete_tag_boundary = self.preview_tag_carry.is_empty()
            && cli_chat_live_preview_has_complete_tag_boundary(self.preview_buffer.as_str());

        Some(CliChatLiveEffectivePreview {
            blocks: Cow::Borrowed(self.preview_blocks.as_slice()),
            display_text,
            display_char_count,
            has_complete_tag_boundary,
            tag_carry_empty: self.preview_tag_carry.is_empty(),
        })
    }

    fn effective_preview_or_reparse(&self) -> CliChatLiveEffectivePreview<'_> {
        self.effective_preview().unwrap_or_else(|| {
            cli_chat_live_effective_preview(
                self.preview_buffer.as_str(),
                self.preview_blocks.as_slice(),
                Some(self.preview_tag_carry.as_str()),
            )
        })
    }

    fn analysis(&self, render_width: usize) -> CliChatLivePreviewAnalysis<'_> {
        let effective_preview = self.effective_preview_or_reparse();
        let emit_signals = effective_preview.emit_signals();
        let emit_stride = cli_chat_live_preview_emit_stride(render_width);
        let preview_char_count = effective_preview.display_char_count;
        let visual_line_count = cli_chat_live_preview_visual_line_count_from_blocks(
            Some(effective_preview.blocks.as_ref()),
            render_width,
        );
        let visual_line_growth = visual_line_count.saturating_sub(self.emit.visual_line_count);
        let unseen_chars = preview_char_count.saturating_sub(self.emit.chars_seen);

        CliChatLivePreviewAnalysis {
            emit_signals,
            emit_stride,
            preview_char_count,
            visual_line_count,
            visual_line_growth,
            unseen_chars,
            effective_preview,
        }
    }

    fn rebuild_from_buffer(&mut self) {
        let parsed = parse_live_preview_state(
            self.preview_buffer.as_str(),
            CliChatLivePreviewBlockKind::Visible,
        );
        self.preview_blocks = parsed.blocks;
        self.preview_display_text = parsed.display_text;
        self.preview_display_char_count = parsed.display_char_count;
        self.preview_parse_kind = parsed.current_kind;
        self.preview_tag_carry = parsed.carry;
    }

    fn append_chunk(&mut self, chunk: &str) {
        let mut fragment = std::mem::take(&mut self.preview_tag_carry);
        fragment.push_str(chunk);
        let parsed = parse_live_preview_state(fragment.as_str(), self.preview_parse_kind);

        self.preview_display_text
            .push_str(parsed.display_text.as_str());
        self.preview_display_char_count = self
            .preview_display_char_count
            .saturating_add(parsed.display_char_count);
        for block in parsed.blocks {
            if let Some(last) = self.preview_blocks.last_mut()
                && last.kind == block.kind
            {
                last.text.push_str(block.text.as_str());
            } else {
                self.preview_blocks.push(block);
            }
        }

        self.preview_parse_kind = parsed.current_kind;
        self.preview_tag_carry = parsed.carry;
    }

    fn append_delta(&mut self, chunk: &str, preview_char_limit: usize) {
        let truncated =
            append_cli_chat_live_buffer(&mut self.preview_buffer, chunk, preview_char_limit);
        if truncated {
            self.rebuild_from_buffer();
        } else {
            self.append_chunk(chunk);
        }
    }

    fn reset_emit_tracking(&mut self) {
        self.emit = CliChatLivePreviewEmitCache::default();
    }

    fn note_emit_elapsed(&mut self, elapsed_ms: Option<u64>) {
        self.emit.elapsed_ms = elapsed_ms;
    }

    fn record_emit(&mut self, snapshot: &CliChatLiveSurfaceSnapshot, render_width: usize) {
        if let Some(preview) = snapshot.effective_preview() {
            self.emit.chars_seen = preview.display_char_count;
            self.emit.visual_line_count = cli_chat_live_preview_visual_line_count_from_blocks(
                Some(preview.blocks.as_ref()),
                render_width,
            );
        } else {
            self.emit.chars_seen = 0;
            self.emit.visual_line_count = 0;
        }
    }

    fn should_emit(&self, render_width: usize, event_elapsed_ms: Option<u64>) -> bool {
        let preview_analysis = self.analysis(render_width);
        if preview_analysis.is_empty() {
            return false;
        }
        let emit_mode = preview_analysis.emit_mode();
        let cadence_ready = self.preview_emit_cadence_ready(emit_mode, event_elapsed_ms);

        if self.emit.chars_seen == 0 {
            return preview_analysis.initial_emit_ready();
        }

        if !cadence_ready {
            return false;
        }

        preview_analysis.update_emit_ready(emit_mode)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn emit_mode(&self, render_width: usize) -> CliChatLivePreviewEmitMode {
        let preview_analysis = self.analysis(render_width);
        if preview_analysis.is_empty() {
            return CliChatLivePreviewEmitMode::Smooth;
        }
        preview_analysis.emit_mode()
    }

    fn preview_emit_cadence_ready(
        &self,
        emit_mode: CliChatLivePreviewEmitMode,
        event_elapsed_ms: Option<u64>,
    ) -> bool {
        let Some(current_elapsed_ms) = event_elapsed_ms else {
            return true;
        };
        let Some(last_emit_elapsed_ms) = self.emit.elapsed_ms else {
            return true;
        };

        let min_interval_ms = match emit_mode {
            CliChatLivePreviewEmitMode::Smooth => CLI_CHAT_LIVE_PREVIEW_SMOOTH_MIN_INTERVAL_MS,
            CliChatLivePreviewEmitMode::CatchUp => CLI_CHAT_LIVE_PREVIEW_CATCH_UP_MIN_INTERVAL_MS,
        };

        current_elapsed_ms.saturating_sub(last_emit_elapsed_ms) >= min_interval_ms
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct CliChatLivePreviewEmitCache {
    chars_seen: usize,
    visual_line_count: usize,
    elapsed_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CliChatLiveSurfaceState {
    pub latest_phase_event: Option<ConversationTurnPhaseEvent>,
    pub first_token_latency_ms: Option<u64>,
    pub preview: CliChatLivePreviewCache,
    pub tool_states: BTreeMap<String, CliChatLiveToolState>,
    pub tool_call_index_map: BTreeMap<usize, String>,
    pub next_tool_display_order: usize,
    pub last_emitted_snapshot: Option<CliChatLiveSurfaceSnapshot>,
    pub last_emitted_lines: Option<Vec<String>>,
}

pub(super) struct CliChatLiveSurfaceObserver {
    render_width: Arc<AtomicUsize>,
    render_sink: CliChatLiveSurfaceSink,
    render_mode: CliChatLiveSurfaceRenderMode,
    state: StdMutex<CliChatLiveSurfaceState>,
}

pub(super) fn build_cli_chat_live_surface_observer(
    render_width: usize,
) -> ConversationTurnObserverHandle {
    let render_sink: CliChatLiveSurfaceSink = Arc::new(|lines| {
        print_rendered_cli_chat_lines(&lines);
    });
    build_cli_chat_live_surface_observer_with_sink(render_width, render_sink)
}

pub(super) fn build_cli_chat_live_surface_observer_with_sink(
    render_width: usize,
    render_sink: CliChatLiveSurfaceSink,
) -> ConversationTurnObserverHandle {
    build_cli_chat_live_surface_observer_with_dynamic_width_sink(
        Arc::new(AtomicUsize::new(render_width.max(1))),
        render_sink,
    )
}

pub(super) fn build_cli_chat_live_surface_observer_with_dynamic_width_sink(
    render_width: Arc<AtomicUsize>,
    render_sink: CliChatLiveSurfaceSink,
) -> ConversationTurnObserverHandle {
    let observer = CliChatLiveSurfaceObserver::new_with_mode(
        render_width,
        render_sink,
        CliChatLiveSurfaceRenderMode::Card,
    );
    Arc::new(observer)
}

#[allow(dead_code)]
pub(super) fn build_cli_chat_live_compact_observer_with_sink(
    render_width: usize,
    render_sink: CliChatLiveSurfaceSink,
) -> ConversationTurnObserverHandle {
    build_cli_chat_live_compact_observer_with_dynamic_width_sink(
        Arc::new(AtomicUsize::new(render_width.max(1))),
        render_sink,
    )
}

pub(super) fn build_cli_chat_live_compact_observer_with_dynamic_width_sink(
    render_width: Arc<AtomicUsize>,
    render_sink: CliChatLiveSurfaceSink,
) -> ConversationTurnObserverHandle {
    let observer = CliChatLiveSurfaceObserver::new_with_mode(
        render_width,
        render_sink,
        CliChatLiveSurfaceRenderMode::Compact,
    );
    Arc::new(observer)
}

pub(super) fn build_cli_chat_live_compact_observer_controller(
    render_width: Arc<AtomicUsize>,
    render_sink: CliChatLiveSurfaceSink,
) -> (ConversationTurnObserverHandle, CliChatLiveSurfaceRerender) {
    let observer = Arc::new(CliChatLiveSurfaceObserver::new_with_mode(
        render_width,
        render_sink,
        CliChatLiveSurfaceRenderMode::Compact,
    ));
    let rerender_observer = Arc::clone(&observer);
    let rerender: CliChatLiveSurfaceRerender = Arc::new(move || {
        rerender_observer.rerender_current_lines();
    });
    (observer as ConversationTurnObserverHandle, rerender)
}

impl CliChatLiveSurfaceObserver {
    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn new(render_width: usize, render_sink: CliChatLiveSurfaceSink) -> Self {
        Self::new_with_mode(
            Arc::new(AtomicUsize::new(render_width.max(1))),
            render_sink,
            CliChatLiveSurfaceRenderMode::Card,
        )
    }

    fn new_with_mode(
        render_width: Arc<AtomicUsize>,
        render_sink: CliChatLiveSurfaceSink,
        render_mode: CliChatLiveSurfaceRenderMode,
    ) -> Self {
        Self {
            render_width,
            render_sink,
            render_mode,
            state: StdMutex::new(CliChatLiveSurfaceState::default()),
        }
    }

    fn render_width(&self) -> usize {
        self.render_width.load(Ordering::Relaxed).max(1)
    }

    fn rerender_current_lines(&self) {
        let lines_to_render = {
            let mut state = self.lock_state();
            if state.latest_phase_event.is_none() {
                None
            } else {
                build_cli_chat_live_surface_snapshot(&state).and_then(|snapshot| {
                    let lines = match self.render_mode {
                        CliChatLiveSurfaceRenderMode::Card => {
                            render_cli_chat_live_surface_lines_with_width(
                                &snapshot,
                                self.render_width(),
                            )
                        }
                        CliChatLiveSurfaceRenderMode::Compact => {
                            render_cli_chat_live_compact_lines_with_width(
                                &snapshot,
                                self.render_width(),
                            )
                        }
                    };
                    if state.last_emitted_lines.as_ref() == Some(&lines) {
                        state.last_emitted_snapshot = Some(snapshot);
                        return None;
                    }
                    state.preview.record_emit(&snapshot, self.render_width());
                    state.last_emitted_snapshot = Some(snapshot);
                    state.last_emitted_lines = Some(lines.clone());
                    Some(lines)
                })
            }
        };

        if let Some(lines) = lines_to_render {
            (self.render_sink)(lines);
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, CliChatLiveSurfaceState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned_state) => poisoned_state.into_inner(),
        }
    }

    fn record_phase_event(&self, event: ConversationTurnPhaseEvent) {
        let lines_to_render = {
            let mut state = self.lock_state();
            if cli_chat_live_phase_starts_provider_request(event.phase) {
                reset_cli_chat_live_request_state(&mut state);
            }
            state.latest_phase_event = Some(event.clone());
            reconcile_cli_chat_live_tool_states_for_phase(&mut state.tool_states, event.phase);
            if !should_render_cli_chat_live_phase(event.phase) {
                None
            } else {
                self.prepare_live_surface_lines(&mut state)
            }
        };

        if let Some(lines) = lines_to_render {
            (self.render_sink)(lines);
        }
    }

    fn record_tool_event(&self, event: ConversationTurnToolEvent) {
        let lines_to_render = {
            let mut state = self.lock_state();
            let render_width = self.render_width();
            apply_cli_chat_live_tool_event(&mut state, &event, render_width);
            let current_phase = match state.latest_phase_event.as_ref() {
                Some(phase_event) => phase_event.phase,
                None => return,
            };
            if should_render_cli_chat_live_phase(current_phase) {
                self.prepare_live_surface_lines(&mut state)
            } else {
                None
            }
        };

        if let Some(lines) = lines_to_render {
            (self.render_sink)(lines);
        }
    }

    fn record_runtime_event(&self, event: ConversationTurnRuntimeEvent) {
        let lines_to_render = {
            let mut state = self.lock_state();
            let render_width = self.render_width();
            apply_cli_chat_live_runtime_event(&mut state, &event, render_width);
            let current_phase = match state.latest_phase_event.as_ref() {
                Some(phase_event) => phase_event.phase,
                None => return,
            };
            if should_render_cli_chat_live_phase(current_phase) {
                self.prepare_live_surface_lines(&mut state)
            } else {
                None
            }
        };

        if let Some(lines) = lines_to_render {
            (self.render_sink)(lines);
        }
    }

    fn record_streaming_token_event(&self, event: crate::acp::StreamingTokenEvent) {
        let lines_to_render = {
            let mut state = self.lock_state();
            let render_width = self.render_width();
            let current_phase = match state.latest_phase_event.as_ref() {
                Some(phase_event) => phase_event.phase,
                None => return,
            };

            let text_delta = event.delta.text;
            let tool_call_delta = event.delta.tool_call;
            let tool_call_index = event.index;
            let mut should_render = false;

            if let Some(text_delta) = text_delta {
                append_cli_chat_live_preview_delta(
                    &mut state,
                    text_delta.as_str(),
                    render_width,
                    event.elapsed_ms,
                );

                if (state.preview.should_emit(render_width, event.elapsed_ms)
                    || cli_chat_live_delta_has_commit_boundary(text_delta.as_str()))
                    && phase_supports_cli_chat_live_preview(current_phase)
                {
                    should_render = true;
                }
            }

            let tool_call_update = match (tool_call_delta, tool_call_index) {
                (Some(tool_call_delta), Some(index)) => Some((tool_call_delta, index)),
                (Some(_), None) | (None, Some(_)) | (None, None) => None,
            };

            if let Some((tool_call_delta, index)) = tool_call_update {
                update_cli_chat_live_tool_state(&mut state, index, &tool_call_delta, render_width);

                let render_tool_activity_now = event.event_type == "tool_call_start"
                    && current_phase == ConversationTurnPhase::RunningTools;
                if render_tool_activity_now {
                    should_render = true;
                }
            }

            if should_render {
                let rendered = self.prepare_live_surface_lines(&mut state);
                if rendered.is_some() {
                    state.preview.note_emit_elapsed(event.elapsed_ms);
                }
                rendered
            } else {
                None
            }
        };

        if let Some(lines) = lines_to_render {
            (self.render_sink)(lines);
        }
    }

    fn prepare_live_surface_lines(
        &self,
        state: &mut CliChatLiveSurfaceState,
    ) -> Option<Vec<String>> {
        let snapshot = build_cli_chat_live_surface_snapshot(state)?;
        if state.last_emitted_snapshot.as_ref() == Some(&snapshot) {
            return None;
        }

        let lines = match self.render_mode {
            CliChatLiveSurfaceRenderMode::Card => {
                render_cli_chat_live_surface_lines_with_width(&snapshot, self.render_width())
            }
            CliChatLiveSurfaceRenderMode::Compact => {
                render_cli_chat_live_compact_lines_with_width(&snapshot, self.render_width())
            }
        };
        state.preview.record_emit(&snapshot, self.render_width());
        state.last_emitted_snapshot = Some(snapshot);
        if state.last_emitted_lines.as_ref() == Some(&lines) {
            return None;
        }
        state.last_emitted_lines = Some(lines.clone());
        Some(lines)
    }
}

impl ConversationTurnObserver for CliChatLiveSurfaceObserver {
    fn on_phase(&self, event: ConversationTurnPhaseEvent) {
        self.record_phase_event(event);
    }

    fn on_tool(&self, event: ConversationTurnToolEvent) {
        self.record_tool_event(event);
    }

    fn on_runtime(&self, event: ConversationTurnRuntimeEvent) {
        self.record_runtime_event(event);
    }

    fn on_streaming_token(&self, event: crate::acp::StreamingTokenEvent) {
        self.record_streaming_token_event(event);
    }
}

pub(super) fn cli_chat_live_phase_starts_provider_request(phase: ConversationTurnPhase) -> bool {
    matches!(
        phase,
        ConversationTurnPhase::RequestingProvider
            | ConversationTurnPhase::RequestingFollowupProvider
    )
}

pub(super) fn reset_cli_chat_live_request_state(state: &mut CliChatLiveSurfaceState) {
    state.first_token_latency_ms = None;
    state.preview.clear();
    state.tool_states.clear();
    state.tool_call_index_map.clear();
    state.next_tool_display_order = 0;
    state.preview.reset_emit_tracking();
    state.last_emitted_snapshot = None;
    state.last_emitted_lines = None;
}

fn should_render_cli_chat_live_phase(phase: ConversationTurnPhase) -> bool {
    match phase {
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Failed => true,
        ConversationTurnPhase::ContextReady | ConversationTurnPhase::Completed => false,
    }
}

pub(super) fn phase_supports_cli_chat_live_preview(phase: ConversationTurnPhase) -> bool {
    match phase {
        ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RequestingFollowupProvider => true,
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed
        | ConversationTurnPhase::Failed => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CliChatLivePreviewEmitSignals {
    has_stable_suffix: bool,
    has_initial_phrase: bool,
    has_initial_structure: bool,
}

struct CliChatLivePreviewAnalysis<'a> {
    emit_signals: CliChatLivePreviewEmitSignals,
    emit_stride: usize,
    preview_char_count: usize,
    visual_line_count: usize,
    visual_line_growth: usize,
    unseen_chars: usize,
    #[allow(dead_code)]
    effective_preview: CliChatLiveEffectivePreview<'a>,
}

impl CliChatLivePreviewAnalysis<'_> {
    fn is_empty(&self) -> bool {
        self.preview_char_count == 0
    }

    fn emit_mode(&self) -> CliChatLivePreviewEmitMode {
        if self.visual_line_growth >= CLI_CHAT_LIVE_PREVIEW_CATCH_UP_ENTER_VISUAL_LINE_GROWTH
            || self.unseen_chars >= self.emit_stride.saturating_mul(2)
            || (self.visual_line_count >= CLI_CHAT_LIVE_PREVIEW_CATCH_UP_ENTER_MIN_VISUAL_LINES
                && self.unseen_chars >= self.emit_stride.saturating_div(2).max(1))
        {
            CliChatLivePreviewEmitMode::CatchUp
        } else {
            CliChatLivePreviewEmitMode::Smooth
        }
    }

    fn initial_emit_ready(&self) -> bool {
        (self.preview_char_count >= CLI_CHAT_LIVE_PREVIEW_INITIAL_EMIT_CHARS
            && self.emit_signals.has_stable_suffix)
            || (self.preview_char_count
                >= CLI_CHAT_LIVE_PREVIEW_INITIAL_EMIT_CHARS.saturating_mul(2)
                && self.emit_signals.has_initial_phrase)
            || (self.preview_char_count >= CLI_CHAT_LIVE_PREVIEW_INITIAL_EMIT_CHARS
                && self.emit_signals.has_initial_structure)
            || (self.visual_line_count >= 2
                && self.preview_char_count
                    >= CLI_CHAT_LIVE_PREVIEW_INITIAL_EMIT_CHARS.saturating_add(1))
            || self.preview_char_count >= self.emit_stride
    }

    fn update_emit_ready(&self, emit_mode: CliChatLivePreviewEmitMode) -> bool {
        match emit_mode {
            CliChatLivePreviewEmitMode::Smooth => {
                if self.visual_line_growth
                    >= CLI_CHAT_LIVE_PREVIEW_CATCH_UP_ENTER_VISUAL_LINE_GROWTH
                    && self.unseen_chars >= self.emit_stride.saturating_div(2).max(1)
                {
                    return true;
                }
                (self.unseen_chars >= self.emit_stride && self.emit_signals.has_stable_suffix)
                    || self.unseen_chars >= self.emit_stride.saturating_mul(2)
            }
            CliChatLivePreviewEmitMode::CatchUp => {
                self.visual_line_growth >= 1
                    || (self.emit_signals.has_stable_suffix
                        && self.unseen_chars >= self.emit_stride.saturating_div(2).max(1))
                    || self.unseen_chars >= self.emit_stride
            }
        }
    }
}

fn cli_chat_live_preview_emit_stride(render_width: usize) -> usize {
    render_width.clamp(
        CLI_CHAT_LIVE_PREVIEW_MIN_EMIT_CHARS,
        CLI_CHAT_LIVE_PREVIEW_MAX_EMIT_CHARS,
    )
}

fn cli_chat_live_delta_has_commit_boundary(text_delta: &str) -> bool {
    text_delta.contains('\n')
        || text_delta.contains("<think>")
        || text_delta.contains("</think>")
        || text_delta.contains("```")
}

fn cli_chat_live_preview_has_stable_suffix(preview: &str) -> bool {
    if preview.is_empty() {
        return false;
    }

    if preview.ends_with('\n') || preview.ends_with(' ') || preview.ends_with('\t') {
        return true;
    }

    let trimmed = preview.trim_end_matches([' ', '\t']);
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.ends_with("```") && trimmed.matches("```").count().is_multiple_of(2) {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("<think>") || lower.ends_with("</think>") {
        return true;
    }

    let Some(last_char) = trimmed.chars().last() else {
        return false;
    };

    last_char.is_ascii_punctuation()
        || matches!(
            last_char,
            '，' | '。' | '！' | '？' | '、' | '；' | '：' | '）' | '】' | '」' | '』'
        )
        || live_preview_is_cjk(last_char)
}

fn cli_chat_live_preview_has_complete_tag_boundary(preview: &str) -> bool {
    let trimmed = preview.trim_end_matches([' ', '\t']);
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    lower.ends_with("<think>") || lower.ends_with("</think>")
}

fn cli_chat_live_preview_has_initial_phrase_boundary(preview: &str) -> bool {
    let mut saw_token = false;
    let mut saw_separator_after_token = false;

    for character in preview.trim().chars() {
        if character.is_whitespace() {
            if saw_token {
                saw_separator_after_token = true;
            }
            continue;
        }

        if saw_separator_after_token {
            return true;
        }

        saw_token = true;
    }

    false
}

fn cli_chat_live_preview_has_initial_structure(preview: &str) -> bool {
    let trimmed = preview.trim();
    !trimmed.is_empty()
        && (split_inline_list_runs(trimmed).is_some()
            || render_live_preview_key_value_lines(trimmed, usize::MAX / 2).is_some()
            || starts_with_ordered_list_marker(trimmed)
            || starts_with_unordered_list_marker(trimmed)
            || looks_like_markdown_table_row(trimmed)
            || trimmed.starts_with('#')
            || trimmed.starts_with('>')
            || trimmed.starts_with("```"))
}

fn starts_with_ordered_list_marker(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    let mut saw_digit = false;
    while chars.peek().is_some_and(|ch| ch.is_ascii_digit()) {
        saw_digit = true;
        let _ = chars.next();
    }
    if !saw_digit {
        return false;
    }
    matches!(chars.next(), Some('.' | ')')) && matches!(chars.next(), Some(' '))
}

fn starts_with_unordered_list_marker(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("• ")
}

fn looks_like_markdown_table_row(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
}

struct CliChatLiveEffectivePreview<'a> {
    blocks: Cow<'a, [CliChatLivePreviewBlock]>,
    display_text: String,
    display_char_count: usize,
    has_complete_tag_boundary: bool,
    tag_carry_empty: bool,
}

impl CliChatLiveEffectivePreview<'_> {
    fn emit_signals(&self) -> CliChatLivePreviewEmitSignals {
        let has_stable_suffix = if self.tag_carry_empty {
            cli_chat_live_preview_has_stable_suffix(self.display_text.as_str())
                || self.has_complete_tag_boundary
        } else {
            false
        };

        CliChatLivePreviewEmitSignals {
            has_stable_suffix,
            has_initial_phrase: cli_chat_live_preview_has_initial_phrase_boundary(
                self.display_text.as_str(),
            ),
            has_initial_structure: cli_chat_live_preview_has_initial_structure(
                self.display_text.as_str(),
            ),
        }
    }
}

fn cli_chat_live_effective_preview<'a>(
    preview: &'a str,
    blocks: &'a [CliChatLivePreviewBlock],
    tag_carry: Option<&str>,
) -> CliChatLiveEffectivePreview<'a> {
    let parsed_preview = (!blocks.is_empty() || preview.trim().is_empty()).then_some(
        CliChatLiveParsedPreviewState {
            blocks: blocks.to_vec(),
            current_kind: CliChatLivePreviewBlockKind::Visible,
            carry: String::new(),
            display_text: match blocks {
                [] => preview.to_owned(),
                [block] => block.text.clone(),
                _ => blocks
                    .iter()
                    .map(|block| block.text.as_str())
                    .collect::<String>(),
            },
            display_char_count: match blocks {
                [] => preview.chars().count(),
                [block] => block.text.chars().count(),
                _ => blocks.iter().map(|block| block.text.chars().count()).sum(),
            },
        },
    );
    let parsed_preview = parsed_preview
        .unwrap_or_else(|| parse_live_preview_state(preview, CliChatLivePreviewBlockKind::Visible));
    let blocks = if !blocks.is_empty() || preview.trim().is_empty() {
        Cow::Borrowed(blocks)
    } else {
        Cow::Owned(parsed_preview.blocks.clone())
    };
    let display_text = parsed_preview.display_text;
    let tag_carry_empty = tag_carry.is_none_or(str::is_empty);
    let has_complete_tag_boundary =
        tag_carry_empty && cli_chat_live_preview_has_complete_tag_boundary(preview);

    CliChatLiveEffectivePreview {
        blocks,
        display_text,
        display_char_count: parsed_preview.display_char_count,
        has_complete_tag_boundary,
        tag_carry_empty,
    }
}

fn cli_chat_live_preview_visual_line_count_from_blocks(
    blocks: Option<&[CliChatLivePreviewBlock]>,
    render_width: usize,
) -> usize {
    let Some(blocks) = blocks else {
        return 0;
    };
    if blocks.is_empty() {
        return 0;
    }

    let wrap_width = render_width.saturating_sub(2).max(1);
    let mut line_count = 0usize;

    for (index, block) in blocks.iter().enumerate() {
        let rendered_count =
            render_live_preview_segment_lines(block.text.as_str(), wrap_width).len();
        if rendered_count == 0 {
            continue;
        }
        if index > 0 && line_count > 0 {
            line_count = line_count.saturating_add(1);
        }
        line_count = line_count.saturating_add(rendered_count);
    }

    line_count
}

pub(super) fn cli_chat_live_preview_char_limit(render_width: usize) -> usize {
    let _ = render_width;
    CLI_CHAT_LIVE_PREVIEW_MAX_BUFFER_CHARS
}

pub(super) fn append_cli_chat_live_preview_delta(
    state: &mut CliChatLiveSurfaceState,
    chunk: &str,
    render_width: usize,
    elapsed_ms: Option<u64>,
) {
    if state.first_token_latency_ms.is_none() {
        state.first_token_latency_ms = elapsed_ms;
    }

    let preview_char_limit = cli_chat_live_preview_char_limit(render_width);
    state.preview.append_delta(chunk, preview_char_limit);
}

fn cli_chat_live_tool_args_char_limit(render_width: usize) -> usize {
    let _ = render_width;
    CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS
}

pub(super) fn append_cli_chat_live_buffer(
    buffer: &mut String,
    chunk: &str,
    char_limit: usize,
) -> bool {
    buffer.push_str(chunk);
    trim_cli_chat_live_buffer(buffer, char_limit)
}

fn trim_cli_chat_live_buffer(buffer: &mut String, char_limit: usize) -> bool {
    let current_char_count = buffer.chars().count();
    if current_char_count <= char_limit {
        return false;
    }

    let retained_char_count = char_limit.saturating_sub(1);
    let skipped_char_count = current_char_count.saturating_sub(retained_char_count);
    let trimmed_tail = buffer.chars().skip(skipped_char_count).collect::<String>();

    buffer.clear();
    buffer.push('…');
    buffer.push_str(trimmed_tail.as_str());
    true
}

pub(super) fn truncate_cli_chat_live_text(value: &str, char_limit: usize) -> String {
    let mut truncated = value.to_owned();
    trim_cli_chat_live_buffer(&mut truncated, char_limit);
    truncated
}

fn cli_chat_live_output_char_limit(render_width: usize) -> usize {
    let _ = render_width;
    CLI_CHAT_LIVE_OUTPUT_MAX_BUFFER_CHARS
}

fn cli_chat_live_line_count(value: &str) -> usize {
    if value.is_empty() {
        return 0;
    }

    let line_break_count = value.chars().filter(|ch| *ch == '\n').count();
    line_break_count.saturating_add(1)
}

fn push_cli_chat_live_output_lines(
    lines: &mut Vec<String>,
    tool_snapshot: &CliChatLiveToolSnapshot,
    label: &str,
    output: &CliChatLiveOutputView,
) {
    if output.text.trim().is_empty() {
        return;
    }

    let preview_mode = cli_chat_live_output_preview_mode(tool_snapshot, label);
    let max_lines = CLI_CHAT_LIVE_OUTPUT_RENDER_MAX_LINES;
    lines.push(format!(
        "  ↳ {label} {} lines · {} bytes",
        output.total_lines, output.total_bytes
    ));

    let mut output_lines = output.text.lines().collect::<Vec<_>>();
    let omitted_count = output_lines.len().saturating_sub(max_lines);
    if omitted_count > 0 {
        output_lines = match preview_mode {
            CliChatLiveOutputPreviewMode::Head => {
                output_lines.into_iter().take(max_lines).collect()
            }
            CliChatLiveOutputPreviewMode::Tail => {
                output_lines.into_iter().skip(omitted_count).collect()
            }
        };
    }

    if omitted_count > 0 && preview_mode == CliChatLiveOutputPreviewMode::Tail {
        lines.push(format!("    … +{omitted_count} earlier lines"));
    }

    for output_line in output_lines {
        let rendered_line =
            truncate_cli_chat_live_text(output_line, CLI_CHAT_LIVE_OUTPUT_MAX_BUFFER_CHARS);
        lines.push(format!("    {rendered_line}"));
    }

    if omitted_count > 0 && preview_mode == CliChatLiveOutputPreviewMode::Head {
        lines.push(format!("    … +{omitted_count} more lines"));
    }

    if output.truncated {
        lines.push("    … live output truncated".to_owned());
    }
}

fn cli_chat_live_output_preview_mode(
    tool_snapshot: &CliChatLiveToolSnapshot,
    label: &str,
) -> CliChatLiveOutputPreviewMode {
    if label == "stderr" {
        return CliChatLiveOutputPreviewMode::Tail;
    }

    let Some(name) = tool_snapshot.name.as_deref() else {
        return CliChatLiveOutputPreviewMode::Tail;
    };
    let normalized_name = normalized_tool_name(name);

    if is_run_tool_name(normalized_name.as_str()) {
        return CliChatLiveOutputPreviewMode::Tail;
    }

    if is_read_tool_name(normalized_name.as_str())
        || is_search_tool_name(normalized_name.as_str())
        || is_list_tool_name(normalized_name.as_str())
        || is_glob_tool_name(normalized_name.as_str())
    {
        return CliChatLiveOutputPreviewMode::Head;
    }

    CliChatLiveOutputPreviewMode::Tail
}

pub(super) fn cli_chat_live_pending_tool_call_id(index: usize) -> String {
    format!("pending-stream-tool-{index}")
}

pub(super) fn ensure_cli_chat_live_tool_state<'a>(
    state: &'a mut CliChatLiveSurfaceState,
    tool_call_id: &str,
) -> &'a mut CliChatLiveToolState {
    let tool_call_key = tool_call_id.to_owned();
    let entry = state.tool_states.entry(tool_call_key.clone());

    match entry {
        std::collections::btree_map::Entry::Occupied(occupied_entry) => occupied_entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(vacant_entry) => {
            let display_order = state.next_tool_display_order;
            let tool_state = CliChatLiveToolState::new(tool_call_key, display_order);
            state.next_tool_display_order = state.next_tool_display_order.saturating_add(1);
            vacant_entry.insert(tool_state)
        }
    }
}

pub(super) fn merge_cli_chat_live_pending_tool_state(
    state: &mut CliChatLiveSurfaceState,
    pending_tool_call_id: &str,
    tool_call_id: &str,
) {
    if pending_tool_call_id == tool_call_id {
        return;
    }

    let pending_state = match state.tool_states.remove(pending_tool_call_id) {
        Some(pending_state) => pending_state,
        None => return,
    };
    let target_state = ensure_cli_chat_live_tool_state(state, tool_call_id);

    if target_state.name.is_none() {
        target_state.name = pending_state.name;
    }
    if target_state.args.is_empty() {
        target_state.args = pending_state.args;
    }
    if target_state.detail.is_none() {
        target_state.detail = pending_state.detail;
    }
    if target_state.status == ConversationTurnToolState::Running {
        target_state.status = pending_state.status;
    }
}

pub(super) fn update_cli_chat_live_tool_state(
    state: &mut CliChatLiveSurfaceState,
    index: usize,
    delta: &crate::acp::ToolCallDelta,
    render_width: usize,
) {
    let pending_tool_call_id = cli_chat_live_pending_tool_call_id(index);
    let tool_call_id = delta.id.clone().unwrap_or_else(|| {
        state
            .tool_call_index_map
            .get(&index)
            .cloned()
            .unwrap_or_else(|| pending_tool_call_id.clone())
    });
    let args_char_limit = cli_chat_live_tool_args_char_limit(render_width);

    state
        .tool_call_index_map
        .insert(index, tool_call_id.clone());
    merge_cli_chat_live_pending_tool_state(
        state,
        pending_tool_call_id.as_str(),
        tool_call_id.as_str(),
    );

    let tool_state = ensure_cli_chat_live_tool_state(state, tool_call_id.as_str());
    tool_state.status = ConversationTurnToolState::Running;
    tool_state.detail = None;

    if let Some(name) = delta.name.as_ref() {
        tool_state.name = Some(name.clone());
    }

    if let Some(args) = delta.args.as_ref() {
        append_cli_chat_live_buffer(&mut tool_state.args, args.as_str(), args_char_limit);
    }
}

pub(super) fn apply_cli_chat_live_tool_event(
    state: &mut CliChatLiveSurfaceState,
    event: &ConversationTurnToolEvent,
    render_width: usize,
) {
    let tool_state = ensure_cli_chat_live_tool_state(state, event.tool_call_id.as_str());
    let detail_char_limit = cli_chat_live_tool_args_char_limit(render_width);

    tool_state.name = Some(event.tool_name.clone());
    if let Some(request_summary) = event.request_summary.as_deref() {
        let truncated_summary =
            truncate_cli_chat_live_text(request_summary, CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS);
        tool_state.request_summary = Some(truncated_summary);
    }
    tool_state.status = event.state;
    tool_state.detail = event
        .detail
        .as_deref()
        .map(|detail| truncate_cli_chat_live_text(detail, detail_char_limit));
}

pub(super) fn apply_cli_chat_live_runtime_event(
    state: &mut CliChatLiveSurfaceState,
    event: &ConversationTurnRuntimeEvent,
    render_width: usize,
) {
    let tool_state = ensure_cli_chat_live_tool_state(state, event.tool_call_id.as_str());

    match &event.event {
        ToolRuntimeEvent::OutputDelta(delta) => {
            let output_char_limit = cli_chat_live_output_char_limit(render_width);
            let target_output = match delta.stream {
                ToolRuntimeStream::Stdout => &mut tool_state.stdout,
                ToolRuntimeStream::Stderr => &mut tool_state.stderr,
            };
            let chunk = delta.chunk.as_str();
            let fallback_line_count = cli_chat_live_line_count(chunk);
            let total_lines = if delta.total_lines == 0 {
                fallback_line_count
            } else {
                delta.total_lines
            };

            target_output.total_bytes = delta.total_bytes;
            target_output.total_lines = total_lines;
            target_output.truncated = delta.truncated;
            append_cli_chat_live_buffer(&mut target_output.text, chunk, output_char_limit);
        }
        ToolRuntimeEvent::FileChangePreview(file_change) => {
            let preview = file_change.preview.as_deref();
            let preview = preview.map(|preview| {
                let preview_limit = cli_chat_live_output_char_limit(render_width);
                truncate_cli_chat_live_text(preview, preview_limit)
            });
            let file_change_view = CliChatLiveFileChangeView {
                path: file_change.path.clone(),
                operation: file_change.kind,
                added_lines: file_change.added_lines,
                removed_lines: file_change.removed_lines,
                preview,
            };
            tool_state.file_change = Some(file_change_view);
        }
        ToolRuntimeEvent::CommandMetrics(metrics) => {
            tool_state.duration_ms = Some(metrics.duration_ms);
            tool_state.exit_code = metrics.exit_code;
        }
    }
}

pub(super) fn reconcile_cli_chat_live_tool_states_for_phase(
    tool_states: &mut BTreeMap<String, CliChatLiveToolState>,
    phase: ConversationTurnPhase,
) {
    let fallback_status = match phase {
        ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed => Some(ConversationTurnToolState::Completed),
        ConversationTurnPhase::Failed => Some(ConversationTurnToolState::Interrupted),
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools => None,
    };
    let Some(fallback_status) = fallback_status else {
        return;
    };

    for tool_state in tool_states.values_mut() {
        if tool_state.status != ConversationTurnToolState::Running {
            continue;
        }

        tool_state.status = fallback_status;
        if fallback_status == ConversationTurnToolState::Interrupted && tool_state.detail.is_none()
        {
            tool_state.detail =
                Some("turn failed before a terminal tool result was recorded".to_owned());
        }
    }
}

pub(super) fn build_cli_chat_live_surface_snapshot(
    state: &CliChatLiveSurfaceState,
) -> Option<CliChatLiveSurfaceSnapshot> {
    let phase_event = state.latest_phase_event.as_ref()?;
    let preview = state.preview.snapshot_preview();
    let tools = build_cli_chat_live_tool_snapshots(&state.tool_states);

    Some(CliChatLiveSurfaceSnapshot {
        phase: phase_event.phase,
        provider_round: phase_event.provider_round,
        lane: phase_event.lane,
        tool_call_count: phase_event.tool_call_count,
        message_count: phase_event.message_count,
        estimated_tokens: phase_event.estimated_tokens,
        first_token_latency_ms: state.first_token_latency_ms,
        preview,
        tools,
    })
}

fn build_cli_chat_live_tool_snapshots(
    tool_states: &BTreeMap<String, CliChatLiveToolState>,
) -> Vec<CliChatLiveToolSnapshot> {
    let mut ordered_states = tool_states.values().collect::<Vec<_>>();
    ordered_states.sort_by_key(|tool_state| tool_state.display_order);

    let mut snapshots = Vec::with_capacity(ordered_states.len());
    for tool_state in ordered_states {
        let snapshot = CliChatLiveToolSnapshot {
            tool_call_id: tool_state.tool_call_id.clone(),
            name: tool_state
                .name
                .as_deref()
                .map(crate::tools::user_visible_tool_name),
            request_summary: tool_state.request_summary.clone(),
            args: tool_state.args.clone(),
            status: tool_state.status,
            detail: tool_state.detail.clone(),
            stdout: tool_state.stdout.clone(),
            stderr: tool_state.stderr.clone(),
            file_change: tool_state.file_change.clone(),
            duration_ms: tool_state.duration_ms,
            exit_code: tool_state.exit_code,
        };
        snapshots.push(snapshot);
    }

    snapshots
}

pub(super) fn format_cli_chat_live_tool_activity_lines(
    tool_snapshots: &[CliChatLiveToolSnapshot],
) -> Vec<String> {
    let mut lines = Vec::new();

    for tool_snapshot in tool_snapshots {
        let name = tool_snapshot.name.as_deref().unwrap_or("pending");
        let tool_line = format_cli_chat_live_tool_headline(tool_snapshot, name);
        lines.push(tool_line);

        let primary_request_line = format_cli_chat_live_primary_request_line(tool_snapshot, name);
        let suppress_generic_request_lines = primary_request_line.is_some();
        if let Some(primary_request_line) = primary_request_line {
            lines.push(primary_request_line);
        }
        if let Some(secondary_request_line) =
            format_cli_chat_live_secondary_request_line(tool_snapshot, name)
        {
            lines.push(secondary_request_line);
        }

        let request_preview = tool_snapshot
            .request_summary
            .as_deref()
            .map(format_cli_chat_live_structured_preview);
        let args_preview = (!tool_snapshot.args.is_empty())
            .then(|| format_cli_chat_live_structured_preview(tool_snapshot.args.as_str()));

        if !suppress_generic_request_lines {
            if let Some(request_preview) = request_preview.as_deref() {
                let request_line = if args_preview.as_deref() == Some(request_preview) {
                    format!("  ↳ request {request_preview}")
                } else if tool_snapshot.request_summary.as_deref() == Some(request_preview) {
                    format!("  ↳ {request_preview}")
                } else {
                    format!("  ↳ request {request_preview}")
                };
                lines.push(request_line);
            }

            if let Some(args_preview) = args_preview.as_deref()
                && request_preview.as_deref() != Some(args_preview)
            {
                let args_line = format!("  ↳ args {args_preview}");
                lines.push(args_line);
            }
        }

        push_cli_chat_live_output_lines(&mut lines, tool_snapshot, "stdout", &tool_snapshot.stdout);
        push_cli_chat_live_output_lines(&mut lines, tool_snapshot, "stderr", &tool_snapshot.stderr);

        if let Some(file_change) = tool_snapshot.file_change.as_ref() {
            let operation = match file_change.operation {
                ToolFileChangeKind::Create => "create",
                ToolFileChangeKind::Overwrite => "overwrite",
                ToolFileChangeKind::Edit => "edit",
            };
            let path = truncate_cli_chat_live_text(
                file_change.path.as_str(),
                CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS,
            );
            let summary_line = format!(
                "  ↳ file {operation} {path} (+{} / -{})",
                file_change.added_lines, file_change.removed_lines,
            );
            lines.push(summary_line);

            if let Some(preview) = file_change.preview.as_deref() {
                let preview_lines = preview.lines().take(CLI_CHAT_LIVE_DIFF_PREVIEW_MAX_LINES);
                for preview_line in preview_lines {
                    let preview_line = truncate_cli_chat_live_text(
                        preview_line,
                        CLI_CHAT_LIVE_OUTPUT_MAX_BUFFER_CHARS,
                    );
                    lines.push(format!("  {preview_line}"));
                }
            }
        }

        if let Some(duration_ms) = tool_snapshot.duration_ms {
            let metrics_line = if let Some(exit_code) = tool_snapshot.exit_code {
                format!("  ↳ metrics {duration_ms}ms · exit={exit_code}")
            } else {
                format!("  ↳ metrics {duration_ms}ms")
            };
            lines.push(metrics_line);
        }
    }

    lines
}

fn format_cli_chat_live_tool_headline(
    tool_snapshot: &CliChatLiveToolSnapshot,
    name: &str,
) -> String {
    let prefix = match tool_snapshot.status {
        ConversationTurnToolState::Running => "• Called",
        ConversationTurnToolState::Completed
        | ConversationTurnToolState::Failed
        | ConversationTurnToolState::Interrupted => "• Closed",
        ConversationTurnToolState::NeedsApproval => "• Approval",
        ConversationTurnToolState::Denied => "• Denied",
    };

    if let Some(detail) = tool_snapshot.detail.as_deref() {
        format!("{prefix} {name} · {detail}")
    } else {
        format!("{prefix} {name}")
    }
}

fn format_cli_chat_live_structured_preview(text: &str) -> String {
    compact_structured_preview(text, 3).unwrap_or_else(|| {
        truncate_cli_chat_live_text(text, CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS)
    })
}

fn format_cli_chat_live_primary_request_line(
    tool_snapshot: &CliChatLiveToolSnapshot,
    name: &str,
) -> Option<String> {
    let normalized_name = normalized_tool_name(name);

    if is_read_tool_name(normalized_name.as_str())
        && let Some(path) = cli_chat_live_read_request_display(tool_snapshot)
    {
        return Some(format!("  ↳ Read {path}"));
    }

    if is_run_tool_name(normalized_name.as_str())
        && let Some(command) = cli_chat_live_request_command(tool_snapshot)
    {
        let command =
            truncate_cli_chat_live_text(command.as_str(), CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS);
        return Some(format!("  ↳ Command {command}"));
    }

    if is_search_tool_name(normalized_name.as_str())
        && let Some(summary) = cli_chat_live_search_request_display(tool_snapshot)
    {
        return Some(format!("  ↳ Search {summary}"));
    }

    if is_list_tool_name(normalized_name.as_str())
        && let Some(summary) = cli_chat_live_list_request_display(tool_snapshot)
    {
        return Some(format!("  ↳ List {summary}"));
    }

    if is_glob_tool_name(normalized_name.as_str())
        && let Some(summary) = cli_chat_live_glob_request_display(tool_snapshot)
    {
        return Some(format!("  ↳ Glob {summary}"));
    }

    None
}

fn format_cli_chat_live_secondary_request_line(
    tool_snapshot: &CliChatLiveToolSnapshot,
    name: &str,
) -> Option<String> {
    let normalized_name = normalized_tool_name(name);

    if is_search_tool_name(normalized_name.as_str())
        && let Some(scope) = cli_chat_live_request_string_field(tool_snapshot, REQUEST_SCOPE_KEYS)
    {
        let scope =
            truncate_cli_chat_live_text(scope.as_str(), CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS);
        return Some(format!("  ↳ scope {scope}"));
    }

    if (is_list_tool_name(normalized_name.as_str()) || is_glob_tool_name(normalized_name.as_str()))
        && let Some(details) = format_inspect_secondary_details(
            cli_chat_live_request_number(tool_snapshot, "depth"),
            cli_chat_live_request_bool(tool_snapshot, REQUEST_HIDDEN_KEYS),
        )
    {
        return Some(format!("  ↳ {details}"));
    }

    None
}

fn cli_chat_live_read_request_display(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    let path = cli_chat_live_request_path(tool_snapshot)?;
    Some(format_read_summary(
        path.as_str(),
        cli_chat_live_request_number(tool_snapshot, "offset"),
        cli_chat_live_request_number(tool_snapshot, "limit"),
    ))
}

fn cli_chat_live_request_command(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    cli_chat_live_request_string_field(tool_snapshot, REQUEST_COMMAND_KEYS)
}

fn cli_chat_live_search_request_display(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    let query = cli_chat_live_request_string_field(tool_snapshot, REQUEST_TEXT_KEYS)?;
    let query = truncate_cli_chat_live_text(query.as_str(), 48);
    let path = cli_chat_live_request_path(tool_snapshot)
        .map(|path| truncate_cli_chat_live_text(shorten_display_path(path.as_str()).as_str(), 40));
    Some(format_search_summary(
        query.as_str(),
        path.as_deref(),
        cli_chat_live_request_number(tool_snapshot, "limit"),
    ))
}

fn cli_chat_live_glob_request_display(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    let pattern = cli_chat_live_request_string_field(tool_snapshot, REQUEST_GLOB_KEYS)?;
    let pattern = truncate_cli_chat_live_text(pattern.as_str(), 48);
    let path = cli_chat_live_request_path(tool_snapshot)
        .map(|path| truncate_cli_chat_live_text(shorten_display_path(path.as_str()).as_str(), 40));
    Some(format_glob_summary(
        pattern.as_str(),
        path.as_deref(),
        cli_chat_live_request_number(tool_snapshot, "limit"),
    ))
}

fn cli_chat_live_list_request_display(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    let path = cli_chat_live_request_path(tool_snapshot).map(|path| {
        truncate_cli_chat_live_text(shorten_display_path(path.as_str()).as_str(), 40)
    })?;
    Some(format_list_summary(
        path.as_str(),
        cli_chat_live_request_number(tool_snapshot, "limit"),
    ))
}

fn cli_chat_live_request_path(tool_snapshot: &CliChatLiveToolSnapshot) -> Option<String> {
    request_path_from_candidates(cli_chat_live_request_candidates(tool_snapshot).as_slice())
}

fn cli_chat_live_request_string_field(
    tool_snapshot: &CliChatLiveToolSnapshot,
    keys: &[&str],
) -> Option<String> {
    request_string_from_candidates(
        cli_chat_live_request_candidates(tool_snapshot).as_slice(),
        keys,
    )
}

fn cli_chat_live_request_number(tool_snapshot: &CliChatLiveToolSnapshot, key: &str) -> Option<u64> {
    request_numeric_from_candidates(
        cli_chat_live_request_candidates(tool_snapshot).as_slice(),
        key,
    )
}

fn cli_chat_live_request_bool(
    tool_snapshot: &CliChatLiveToolSnapshot,
    keys: &[&str],
) -> Option<bool> {
    request_bool_from_candidates(
        cli_chat_live_request_candidates(tool_snapshot).as_slice(),
        keys,
    )
}

fn cli_chat_live_request_candidates(tool_snapshot: &CliChatLiveToolSnapshot) -> [Option<&str>; 2] {
    [
        (!tool_snapshot.args.is_empty()).then_some(tool_snapshot.args.as_str()),
        tool_snapshot.request_summary.as_deref(),
    ]
}

pub(super) fn render_cli_chat_live_surface_lines_with_width(
    snapshot: &CliChatLiveSurfaceSnapshot,
    width: usize,
) -> Vec<String> {
    let body_width = cli_chat_card_inner_width(width);
    let message_spec = build_cli_chat_live_surface_message_spec(snapshot, body_width);
    let body_lines = render_tui_message_body_spec(&message_spec, body_width);
    let title = build_cli_chat_live_surface_card_title(snapshot);
    render_cli_chat_card_lines(title.as_str(), &body_lines, width)
}

pub(super) fn render_cli_chat_live_compact_lines_with_width(
    snapshot: &CliChatLiveSurfaceSnapshot,
    width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let wrap_width = width.saturating_sub(2).max(1);

    if let Some(preview) = snapshot.effective_preview() {
        lines.extend(render_live_preview_lines_from_blocks(
            preview.blocks.as_ref(),
            wrap_width,
        ));
    }

    if !snapshot.tools.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        for raw_line in format_cli_chat_live_tool_activity_lines(snapshot.tools.as_slice()) {
            for wrapped in
                crate::presentation::render_wrapped_display_line(raw_line.as_str(), wrap_width)
            {
                lines.push(wrapped);
            }
        }
    }

    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum CliChatLivePreviewBlockKind {
    #[default]
    Visible,
    Thinking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CliChatLivePreviewBlock {
    kind: CliChatLivePreviewBlockKind,
    text: String,
}

struct CliChatLiveParsedPreviewState {
    blocks: Vec<CliChatLivePreviewBlock>,
    current_kind: CliChatLivePreviewBlockKind,
    carry: String,
    display_text: String,
    display_char_count: usize,
}

fn parse_live_preview_state(
    preview: &str,
    initial_kind: CliChatLivePreviewBlockKind,
) -> CliChatLiveParsedPreviewState {
    let open_tag = "<think>";
    let close_tag = "</think>";
    let lower = preview.to_ascii_lowercase();
    let mut current = String::new();
    let mut idx = 0usize;
    let mut current_kind = initial_kind;
    let mut blocks = Vec::new();
    let mut display_text = String::new();
    let mut display_char_count = 0usize;

    let flush_block = |blocks: &mut Vec<CliChatLivePreviewBlock>,
                       current: &mut String,
                       display_text: &mut String,
                       display_char_count: &mut usize,
                       current_kind: CliChatLivePreviewBlockKind| {
        if current.trim().is_empty() {
            current.clear();
            return;
        }

        let text = std::mem::take(current);
        display_text.push_str(text.as_str());
        *display_char_count = display_char_count.saturating_add(text.chars().count());
        if let Some(last) = blocks.last_mut()
            && last.kind == current_kind
        {
            last.text.push_str(text.as_str());
            return;
        }

        blocks.push(CliChatLivePreviewBlock {
            kind: current_kind,
            text,
        });
    };

    while idx < preview.len() {
        let remaining = &lower[idx..];
        if remaining.starts_with(open_tag) {
            flush_block(
                &mut blocks,
                &mut current,
                &mut display_text,
                &mut display_char_count,
                current_kind,
            );
            current_kind = CliChatLivePreviewBlockKind::Thinking;
            idx += open_tag.len();
            continue;
        }
        if remaining.starts_with(close_tag) {
            flush_block(
                &mut blocks,
                &mut current,
                &mut display_text,
                &mut display_char_count,
                current_kind,
            );
            current_kind = CliChatLivePreviewBlockKind::Visible;
            idx += close_tag.len();
            continue;
        }
        if open_tag.starts_with(remaining) || close_tag.starts_with(remaining) {
            flush_block(
                &mut blocks,
                &mut current,
                &mut display_text,
                &mut display_char_count,
                current_kind,
            );
            return CliChatLiveParsedPreviewState {
                blocks,
                current_kind,
                carry: preview[idx..].to_owned(),
                display_text,
                display_char_count,
            };
        }

        let Some(ch) = preview[idx..].chars().next() else {
            break;
        };
        current.push(ch);
        idx += ch.len_utf8();
    }

    flush_block(
        &mut blocks,
        &mut current,
        &mut display_text,
        &mut display_char_count,
        current_kind,
    );
    CliChatLiveParsedPreviewState {
        blocks,
        current_kind,
        carry: String::new(),
        display_text,
        display_char_count,
    }
}

fn render_live_preview_lines_from_blocks(
    blocks: &[CliChatLivePreviewBlock],
    wrap_width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let wrap_width = wrap_width.max(1);

    for (index, block) in blocks.iter().enumerate() {
        let rendered = render_live_preview_segment_lines(block.text.as_str(), wrap_width);
        if rendered.is_empty() {
            continue;
        }
        if index > 0 && !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(rendered);
    }

    lines
}

fn render_live_preview_segment_lines(segment: &str, wrap_width: usize) -> Vec<String> {
    let normalized = sanitize_live_preview_text(segment);
    if !normalized.contains('\n')
        && let Some(key_value_lines) =
            render_live_preview_key_value_lines(normalized.trim_end(), wrap_width)
    {
        return key_value_lines;
    }
    if let Some(split_lines) = split_inline_list_runs(normalized.as_str()) {
        let mut rendered = Vec::new();
        for line in split_lines {
            rendered.extend(crate::presentation::render_wrapped_display_line(
                line.as_str(),
                wrap_width,
            ));
        }
        trim_outer_blank_lines(&mut rendered);
        return rendered;
    }
    if let Some(structured_lines) =
        render_live_preview_structured_lines(normalized.as_str(), wrap_width)
    {
        return structured_lines;
    }
    let mut rendered = Vec::new();
    let mut paragraph_buffer = String::new();

    let flush_paragraph = |rendered: &mut Vec<String>, paragraph_buffer: &mut String| {
        if paragraph_buffer.trim().is_empty() {
            paragraph_buffer.clear();
            return;
        }
        rendered.extend(crate::presentation::render_wrapped_display_line(
            paragraph_buffer.trim(),
            wrap_width,
        ));
        paragraph_buffer.clear();
    };

    let normalized_lines = normalized.lines().collect::<Vec<_>>();
    for line in normalized_lines {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            flush_paragraph(&mut rendered, &mut paragraph_buffer);
            if rendered
                .last()
                .map(|line: &String| !line.trim().is_empty())
                .unwrap_or(true)
            {
                rendered.push(String::new());
            }
            continue;
        }

        if let Some(split_bullets) = split_inline_list_runs(trimmed) {
            flush_paragraph(&mut rendered, &mut paragraph_buffer);
            for bullet_line in split_bullets {
                rendered.extend(crate::presentation::render_wrapped_display_line(
                    bullet_line.as_str(),
                    wrap_width,
                ));
            }
            continue;
        }

        if let Some(key_value_lines) = render_live_preview_key_value_lines(trimmed, wrap_width) {
            flush_paragraph(&mut rendered, &mut paragraph_buffer);
            rendered.extend(key_value_lines);
            continue;
        }

        if is_reflowable_live_preview_line(trimmed) {
            if !paragraph_buffer.is_empty() {
                paragraph_buffer.push_str(live_preview_paragraph_joiner(
                    paragraph_buffer.as_str(),
                    trimmed,
                ));
            }
            paragraph_buffer.push_str(trimmed.trim());
            continue;
        }

        flush_paragraph(&mut rendered, &mut paragraph_buffer);
        rendered.extend(crate::presentation::render_wrapped_display_line(
            trimmed, wrap_width,
        ));
    }

    flush_paragraph(&mut rendered, &mut paragraph_buffer);
    trim_outer_blank_lines(&mut rendered);
    rendered
}

fn is_reflowable_live_preview_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with('#')
        && !trimmed.starts_with('>')
        && !trimmed.starts_with("```")
        && !trimmed.ends_with(':')
        && !starts_with_unordered_list_marker(trimmed)
        && !starts_with_ordered_list_marker(trimmed)
        && !looks_like_markdown_table_row(trimmed)
        && !looks_like_live_preview_logfmt_line(trimmed)
        && !looks_like_live_preview_code_line(trimmed)
        && render_live_preview_key_value_lines(trimmed, usize::MAX / 2).is_none()
        && split_inline_list_runs(trimmed).is_none()
        && (trimmed.contains(char::is_whitespace) || live_preview_contains_cjk(trimmed))
}

fn looks_like_live_preview_logfmt_line(line: &str) -> bool {
    line.split_whitespace()
        .filter(|token| {
            let (key, value) = token.split_once('=').unwrap_or(("", ""));
            !key.is_empty() && !value.is_empty()
        })
        .nth(1)
        .is_some()
}

fn looks_like_live_preview_code_line(line: &str) -> bool {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();

    lower.starts_with("import ")
        || lower.starts_with("const ")
        || lower.starts_with("func ")
        || lower.starts_with("return ")
        || lower.starts_with("let ")
        || lower.starts_with("var ")
        || lower.starts_with("fn ")
        || lower.starts_with("class ")
        || lower.starts_with("if ")
        || lower.starts_with("for ")
        || lower.starts_with("while ")
        || trimmed.contains(" = ")
        || trimmed.ends_with('{')
        || trimmed == "}"
        || trimmed == ")"
}

fn live_preview_paragraph_joiner(current: &str, next: &str) -> &'static str {
    if live_preview_contains_cjk(current) || live_preview_contains_cjk(next) {
        ""
    } else {
        " "
    }
}

fn live_preview_contains_cjk(text: &str) -> bool {
    text.chars().any(live_preview_is_cjk)
}

fn render_live_preview_key_value_lines(line: &str, wrap_width: usize) -> Option<Vec<String>> {
    let body = line.trim_start().strip_prefix("- ")?;
    let (key, value) = body.split_once(": ")?;
    let prefix = format!("- {key}: ");
    let prefix_width = crate::presentation::display_width(prefix.as_str());
    let value_width = wrap_width.saturating_sub(prefix_width).max(1);
    let wrapped = crate::presentation::render_wrapped_display_line(value.trim(), value_width);

    Some(
        wrapped
            .into_iter()
            .enumerate()
            .map(|(index, wrapped_line)| {
                if index == 0 {
                    format!("{prefix}{wrapped_line}")
                } else {
                    format!("{}{}", " ".repeat(prefix_width), wrapped_line)
                }
            })
            .collect(),
    )
}

fn render_live_preview_structured_lines(segment: &str, wrap_width: usize) -> Option<Vec<String>> {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }

    if let Some(diff_body) = live_preview_diff_body(trimmed) {
        let mut rendered = Vec::new();
        for plain in live_preview_render_raw_diff_lines(diff_body.as_str()) {
            for wrapped in wrap_live_preview_diff_line(plain.as_str(), wrap_width) {
                rendered.push(wrapped);
            }
        }
        trim_outer_blank_lines(&mut rendered);
        return Some(rendered);
    }

    if let Some(diff_body) = live_preview_provisional_diff_body(trimmed) {
        let diff_lines = if diff_body.trim().is_empty() {
            vec!["  diff preview…".to_owned()]
        } else {
            live_preview_render_raw_diff_lines(diff_body.as_str())
        };
        let mut rendered = Vec::new();
        for plain in diff_lines {
            for wrapped in wrap_live_preview_diff_line(plain.as_str(), wrap_width) {
                rendered.push(wrapped);
            }
        }
        trim_outer_blank_lines(&mut rendered);
        return Some(rendered);
    }

    if live_preview_contains_markdown_table(trimmed) {
        let mut rendered = markdown::render_markdown_to_lines_with_width(trimmed, Some(wrap_width))
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        trim_outer_blank_lines(&mut rendered);
        return Some(rendered);
    }

    if let Some(rendered) = render_live_preview_provisional_table(trimmed, wrap_width) {
        return Some(rendered);
    }

    if live_preview_contains_structured_markdown(trimmed) {
        let mut rendered = markdown::render_markdown_to_lines_with_width(trimmed, Some(wrap_width))
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        trim_outer_blank_lines(&mut rendered);
        return Some(rendered);
    }

    None
}
fn live_preview_provisional_diff_body(segment: &str) -> Option<String> {
    let mut lines = segment.lines();
    let first = lines.next()?.trim();
    let lower = first.to_ascii_lowercase();
    let is_partial_diff_fence = first.starts_with("```")
        && first != "```"
        && ("```diff".starts_with(lower.as_str()) || "```patch".starts_with(lower.as_str()));
    if is_partial_diff_fence {
        return Some(lines.collect::<Vec<_>>().join("\n"));
    }

    let mut plus_line_count = 0usize;
    let mut minus_line_count = 0usize;
    let mut structural_line_count = 0usize;
    for line in segment.lines().map(str::trim_start) {
        if line.starts_with("+ ") {
            plus_line_count = plus_line_count.saturating_add(1);
        } else if line.starts_with("- ") {
            minus_line_count = minus_line_count.saturating_add(1);
        } else if line.starts_with("@@")
            || line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
        {
            structural_line_count = structural_line_count.saturating_add(1);
        }
    }

    let has_diff_pair = plus_line_count > 0 && minus_line_count > 0;
    let has_structured_diff = structural_line_count > 0
        && plus_line_count
            .saturating_add(minus_line_count)
            .saturating_add(structural_line_count)
            >= 2;
    (has_diff_pair || has_structured_diff).then(|| segment.to_owned())
}

fn live_preview_diff_body(segment: &str) -> Option<String> {
    let mut lines = segment.lines();
    let fence = lines.next()?.trim();
    if !matches!(fence, "```diff" | "```patch") {
        return None;
    }

    let mut body = lines.collect::<Vec<_>>();
    if body.last().is_some_and(|line| line.trim() == "```") {
        body.pop();
    }
    Some(body.join("\n"))
}

fn render_live_preview_provisional_table(segment: &str, wrap_width: usize) -> Option<Vec<String>> {
    let mut rows = Vec::new();
    let mut saw_table_like_line = false;

    for line in segment
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if live_preview_is_markdown_table_separator(line) {
            saw_table_like_line = true;
            continue;
        }
        if !live_preview_is_provisional_table_row(line) {
            return None;
        }
        saw_table_like_line = true;
        rows.push(parse_live_preview_table_cells(line));
    }

    if !saw_table_like_line || rows.is_empty() {
        return None;
    }

    let markdown_table = provisional_table_rows_to_markdown(rows.as_slice());
    let mut rendered = markdown::render_markdown_to_lines_with_width(
        markdown_table.as_str(),
        Some(wrap_width.max(1)),
    )
    .into_iter()
    .map(|line| {
        line.spans
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
    })
    .collect::<Vec<_>>();
    trim_outer_blank_lines(&mut rendered);
    Some(rendered)
}

fn live_preview_is_provisional_table_row(line: &str) -> bool {
    line.starts_with('|') && line.matches('|').count() >= 2
}

fn parse_live_preview_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect::<Vec<_>>()
}

fn provisional_table_rows_to_markdown(rows: &[Vec<String>]) -> String {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let mut normalized_rows = rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row.resize(column_count, String::new());
            row
        })
        .collect::<Vec<_>>();

    if normalized_rows.is_empty() {
        normalized_rows.push(vec![String::new(); column_count]);
    }

    let mut lines = Vec::with_capacity(normalized_rows.len().saturating_add(1));
    if let Some(header) = normalized_rows.first() {
        lines.push(format_provisional_markdown_row(header.as_slice()));
    }
    lines.push(format_provisional_markdown_separator(column_count));
    for row in normalized_rows.iter().skip(1) {
        lines.push(format_provisional_markdown_row(row.as_slice()));
    }
    lines.join("\n")
}

fn format_provisional_markdown_row(row: &[String]) -> String {
    let cells = row
        .iter()
        .map(|cell| escape_provisional_markdown_cell(cell))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("| {cells} |")
}

fn format_provisional_markdown_separator(column_count: usize) -> String {
    let columns = std::iter::repeat_n("---", column_count)
        .collect::<Vec<_>>()
        .join(" | ");
    format!("| {columns} |")
}

fn escape_provisional_markdown_cell(cell: &str) -> String {
    cell.replace('\\', "\\\\")
        .replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn live_preview_render_raw_diff_lines(diff: &str) -> Vec<String> {
    let lines = diff
        .lines()
        .map(|line| {
            if line.starts_with('+')
                || line.starts_with('-')
                || line.starts_with("@@")
                || line.starts_with("diff ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
            {
                line.to_owned()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        vec!["  (empty diff)".to_owned()]
    } else {
        lines
    }
}

fn wrap_live_preview_diff_line(line: &str, wrap_width: usize) -> Vec<String> {
    for prefix in ["+ ", "- ", "@@ ", "+++ ", "--- ", "diff ", "index "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let continuation = " ".repeat(crate::presentation::display_width(prefix));
            return wrap_with_prefix(prefix, continuation.as_str(), rest, wrap_width);
        }
    }

    crate::presentation::render_wrapped_display_line(line, wrap_width)
}

fn wrap_with_prefix(
    prefix: &str,
    continuation_prefix: &str,
    body: &str,
    wrap_width: usize,
) -> Vec<String> {
    let prefix_width = crate::presentation::display_width(prefix);
    let continuation_width = crate::presentation::display_width(continuation_prefix);
    let first_width = wrap_width.saturating_sub(prefix_width).max(1);
    let continuation_body_width = wrap_width.saturating_sub(continuation_width).max(1);

    let wrapped_body = crate::presentation::render_wrapped_display_line(body, first_width);
    let mut rendered = Vec::new();
    for (index, line) in wrapped_body.into_iter().enumerate() {
        if index == 0 {
            rendered.push(format!("{prefix}{line}"));
        } else {
            for continuation_line in crate::presentation::render_wrapped_display_line(
                line.as_str(),
                continuation_body_width,
            ) {
                rendered.push(format!("{continuation_prefix}{continuation_line}"));
            }
        }
    }
    rendered
}

fn live_preview_contains_markdown_table(segment: &str) -> bool {
    let lines = segment.lines().map(str::trim).collect::<Vec<_>>();
    lines.len() >= 2
        && lines.windows(2).any(|pair| match pair {
            [first, second] => {
                live_preview_is_markdown_table_row(first)
                    && live_preview_is_markdown_table_separator(second)
            }
            _ => false,
        })
}

fn live_preview_contains_structured_markdown(segment: &str) -> bool {
    let lines = segment.lines().map(str::trim).collect::<Vec<_>>();
    if lines.is_empty() {
        return false;
    }

    lines.iter().any(|line| {
        line.starts_with("```")
            || line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || line.starts_with("> ")
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with("1. ")
            || line.starts_with("2. ")
            || line.starts_with("3. ")
    })
}

fn live_preview_is_markdown_table_row(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 3
}

fn live_preview_is_markdown_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !live_preview_is_markdown_table_row(trimmed) {
        return false;
    }

    trimmed.trim_matches('|').split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
    })
}

fn sanitize_live_preview_text(segment: &str) -> String {
    let normalized = segment.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = trim_unstable_preview_suffix(normalized.as_str());
    collapse_preview_blank_runs(trimmed.as_str())
}

fn trim_unstable_preview_suffix(segment: &str) -> String {
    if segment.is_empty() || segment.ends_with('\n') {
        return segment.to_owned();
    }

    let trimmed = segment.trim_end_matches([' ', '\t']);
    if trimmed.is_empty() {
        return String::new();
    }

    let lower = trimmed.to_ascii_lowercase();
    if "<think>".starts_with(lower.as_str())
        || "</think>".starts_with(lower.as_str())
        || lower.ends_with('<')
        || lower.ends_with("</")
    {
        return String::new();
    }

    if trimmed.ends_with("```") && trimmed.matches("```").count() % 2 == 1 {
        return trimmed
            .rsplit_once('\n')
            .map(|(head, _)| head.to_owned())
            .unwrap_or_default();
    }

    trimmed.to_owned()
}

fn collapse_preview_blank_runs(segment: &str) -> String {
    let mut normalized = Vec::new();
    let mut last_was_blank = false;
    for line in segment.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_was_blank {
            continue;
        }
        last_was_blank = is_blank;
        normalized.push(line);
    }
    normalized.join("\n")
}

fn live_preview_is_cjk(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3040}'..='\u{30FF}').contains(&ch)
        || ('\u{AC00}'..='\u{D7AF}').contains(&ch)
}

fn trim_outer_blank_lines(lines: &mut Vec<String>) {
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
}

fn build_cli_chat_live_surface_message_spec(
    snapshot: &CliChatLiveSurfaceSnapshot,
    body_width: usize,
) -> TuiMessageSpec {
    let phase_tone = cli_chat_live_surface_tone(snapshot.phase);
    let phase_title = cli_chat_live_surface_title(snapshot.phase);
    let phase_detail = cli_chat_live_surface_detail(snapshot);
    let phase_section = TuiSectionSpec::Callout {
        tone: phase_tone,
        title: Some(phase_title.to_owned()),
        lines: vec![phase_detail],
    };
    let pipeline_items = build_cli_chat_live_pipeline_items(snapshot);
    let pipeline_section = TuiSectionSpec::Checklist {
        title: Some("turn pipeline".to_owned()),
        items: pipeline_items,
    };
    let status_items = build_cli_chat_live_status_items(snapshot);
    let mut sections = vec![phase_section, pipeline_section];

    if !status_items.is_empty() {
        let status_section = TuiSectionSpec::KeyValues {
            title: Some("status".to_owned()),
            items: status_items,
        };
        sections.push(status_section);
    }

    if let Some(preview_section) = build_cli_chat_live_preview_section(snapshot, body_width) {
        sections.push(preview_section);
    }

    if let Some(tool_section) = build_cli_chat_live_tool_section(snapshot) {
        sections.push(tool_section);
    }

    TuiMessageSpec {
        role: config::CLI_COMMAND_NAME.to_owned(),
        caption: Some("live".to_owned()),
        sections,
        footer_lines: vec![
            "Streaming turn state · /status runtime · /compact checkpoint".to_owned(),
        ],
    }
}

fn build_cli_chat_live_surface_card_title(snapshot: &CliChatLiveSurfaceSnapshot) -> String {
    let mut segments = vec![build_cli_chat_message_card_title(
        config::CLI_COMMAND_NAME,
        Some("live"),
    )];

    if let Some(provider_round) = snapshot.provider_round {
        segments.push(format!("round {provider_round}"));
    }

    if let Some(message_count) = snapshot.message_count {
        segments.push(format!("{message_count} msgs"));
    }

    if let Some(estimated_tokens) = snapshot.estimated_tokens {
        segments.push(format!("~{estimated_tokens} tok"));
    }

    if let Some(first_token_latency_ms) = snapshot.first_token_latency_ms {
        segments.push(format!("ttft {first_token_latency_ms}ms"));
    }

    segments.join(" · ")
}

fn cli_chat_live_surface_tone(phase: ConversationTurnPhase) -> TuiCalloutTone {
    match phase {
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply => TuiCalloutTone::Info,
        ConversationTurnPhase::Completed => TuiCalloutTone::Success,
        ConversationTurnPhase::Failed => TuiCalloutTone::Warning,
    }
}

fn cli_chat_live_surface_title(phase: ConversationTurnPhase) -> &'static str {
    match phase {
        ConversationTurnPhase::Preparing => "assembling context",
        ConversationTurnPhase::ContextReady => "context ready",
        ConversationTurnPhase::RequestingProvider => "querying model",
        ConversationTurnPhase::RunningTools => "running tools",
        ConversationTurnPhase::RequestingFollowupProvider => "requesting follow-up",
        ConversationTurnPhase::FinalizingReply => "finalizing reply",
        ConversationTurnPhase::Completed => "reply ready",
        ConversationTurnPhase::Failed => "turn failed",
    }
}

fn cli_chat_live_surface_detail(snapshot: &CliChatLiveSurfaceSnapshot) -> String {
    match snapshot.phase {
        ConversationTurnPhase::Preparing => {
            "Building the session context and preparing the next provider turn.".to_owned()
        }
        ConversationTurnPhase::ContextReady => {
            "Context is ready for the next provider round.".to_owned()
        }
        ConversationTurnPhase::RequestingProvider => {
            let provider_round = snapshot.provider_round.unwrap_or(1);
            if let Some(first_token_latency_ms) = snapshot.first_token_latency_ms {
                return format!(
                    "Provider round {provider_round} started streaming after {first_token_latency_ms} ms."
                );
            }

            format!("Requesting provider round {provider_round} and waiting for the reply.")
        }
        ConversationTurnPhase::RunningTools => {
            let lane_label = snapshot
                .lane
                .map(format_cli_chat_live_lane)
                .unwrap_or_else(|| "-".to_owned());
            format!(
                "Executing {} tool call(s) in the {lane_label} lane.",
                snapshot.tool_call_count
            )
        }
        ConversationTurnPhase::RequestingFollowupProvider => {
            let provider_round = snapshot.provider_round.unwrap_or(1);
            if let Some(first_token_latency_ms) = snapshot.first_token_latency_ms {
                return format!(
                    "Follow-up provider round {provider_round} started streaming after {first_token_latency_ms} ms."
                );
            }

            format!("Sending tool results back for provider round {provider_round}.")
        }
        ConversationTurnPhase::FinalizingReply => {
            "Persisting the assistant reply and finishing after-turn work.".to_owned()
        }
        ConversationTurnPhase::Completed => "The assistant reply is ready.".to_owned(),
        ConversationTurnPhase::Failed => {
            "The turn failed before a stable reply could be finalized.".to_owned()
        }
    }
}

fn build_cli_chat_live_pipeline_items(
    snapshot: &CliChatLiveSurfaceSnapshot,
) -> Vec<TuiChecklistItemSpec> {
    let prepare_item = TuiChecklistItemSpec {
        status: cli_chat_live_prepare_status(snapshot.phase),
        label: "prepare context".to_owned(),
        detail: cli_chat_live_prepare_detail(snapshot.phase),
    };
    let model_item = TuiChecklistItemSpec {
        status: cli_chat_live_model_status(snapshot.phase),
        label: "call model".to_owned(),
        detail: cli_chat_live_model_detail(snapshot),
    };
    let tools_item = TuiChecklistItemSpec {
        status: cli_chat_live_tools_status(snapshot),
        label: "run tools".to_owned(),
        detail: cli_chat_live_tools_detail(snapshot),
    };
    let finalize_item = TuiChecklistItemSpec {
        status: cli_chat_live_finalize_status(snapshot.phase),
        label: "finalize reply".to_owned(),
        detail: cli_chat_live_finalize_detail(snapshot.phase),
    };

    vec![prepare_item, model_item, tools_item, finalize_item]
}

fn cli_chat_live_prepare_status(phase: ConversationTurnPhase) -> TuiChecklistStatus {
    match phase {
        ConversationTurnPhase::Preparing => TuiChecklistStatus::Warn,
        ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed
        | ConversationTurnPhase::Failed => TuiChecklistStatus::Pass,
    }
}

fn cli_chat_live_prepare_detail(phase: ConversationTurnPhase) -> String {
    match phase {
        ConversationTurnPhase::Preparing => "assembling the next turn context".to_owned(),
        ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed
        | ConversationTurnPhase::Failed => "context assembled".to_owned(),
    }
}

fn cli_chat_live_model_status(phase: ConversationTurnPhase) -> TuiChecklistStatus {
    match phase {
        ConversationTurnPhase::Preparing | ConversationTurnPhase::ContextReady => {
            TuiChecklistStatus::Warn
        }
        ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RequestingFollowupProvider => TuiChecklistStatus::Warn,
        ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed => TuiChecklistStatus::Pass,
        ConversationTurnPhase::Failed => TuiChecklistStatus::Fail,
    }
}

fn cli_chat_live_model_detail(snapshot: &CliChatLiveSurfaceSnapshot) -> String {
    match snapshot.phase {
        ConversationTurnPhase::Preparing => "waiting for a provider round".to_owned(),
        ConversationTurnPhase::ContextReady => "provider request is about to start".to_owned(),
        ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RequestingFollowupProvider => {
            let provider_round = snapshot.provider_round.unwrap_or(1);
            if let Some(first_token_latency_ms) = snapshot.first_token_latency_ms {
                return format!("first token in {first_token_latency_ms} ms");
            }

            format!("provider round {provider_round} in progress")
        }
        ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed => "provider reply resolved".to_owned(),
        ConversationTurnPhase::Failed => "provider step did not finish cleanly".to_owned(),
    }
}

fn cli_chat_live_tools_status(snapshot: &CliChatLiveSurfaceSnapshot) -> TuiChecklistStatus {
    let tools_needed = snapshot.tool_call_count > 0;
    if !tools_needed {
        return match snapshot.phase {
            ConversationTurnPhase::FinalizingReply
            | ConversationTurnPhase::Completed
            | ConversationTurnPhase::Failed => TuiChecklistStatus::Pass,
            ConversationTurnPhase::Preparing
            | ConversationTurnPhase::ContextReady
            | ConversationTurnPhase::RequestingProvider
            | ConversationTurnPhase::RunningTools
            | ConversationTurnPhase::RequestingFollowupProvider => TuiChecklistStatus::Warn,
        };
    }

    match snapshot.phase {
        ConversationTurnPhase::RunningTools => TuiChecklistStatus::Warn,
        ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed => TuiChecklistStatus::Pass,
        ConversationTurnPhase::Failed => TuiChecklistStatus::Fail,
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider => TuiChecklistStatus::Warn,
    }
}

fn cli_chat_live_tools_detail(snapshot: &CliChatLiveSurfaceSnapshot) -> String {
    let tools_needed = snapshot.tool_call_count > 0;
    if !tools_needed {
        return match snapshot.phase {
            ConversationTurnPhase::FinalizingReply | ConversationTurnPhase::Completed => {
                "no tool calls were needed for this turn".to_owned()
            }
            ConversationTurnPhase::Failed => "no tool step was completed".to_owned(),
            ConversationTurnPhase::Preparing
            | ConversationTurnPhase::ContextReady
            | ConversationTurnPhase::RequestingProvider
            | ConversationTurnPhase::RunningTools
            | ConversationTurnPhase::RequestingFollowupProvider => {
                "waiting to see whether tools are needed".to_owned()
            }
        };
    }

    let lane_label = snapshot
        .lane
        .map(format_cli_chat_live_lane)
        .unwrap_or_else(|| "-".to_owned());
    match snapshot.phase {
        ConversationTurnPhase::RunningTools => {
            format!(
                "{} tool call(s) currently running in the {lane_label} lane",
                snapshot.tool_call_count
            )
        }
        ConversationTurnPhase::RequestingFollowupProvider
        | ConversationTurnPhase::FinalizingReply
        | ConversationTurnPhase::Completed => {
            format!(
                "{} tool call(s) finished in the {lane_label} lane",
                snapshot.tool_call_count
            )
        }
        ConversationTurnPhase::Failed => {
            format!(
                "{} tool call(s) did not converge cleanly",
                snapshot.tool_call_count
            )
        }
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider => {
            format!(
                "{} tool call(s) are queued if the provider asks for them",
                snapshot.tool_call_count
            )
        }
    }
}

fn cli_chat_live_finalize_status(phase: ConversationTurnPhase) -> TuiChecklistStatus {
    match phase {
        ConversationTurnPhase::FinalizingReply => TuiChecklistStatus::Warn,
        ConversationTurnPhase::Completed => TuiChecklistStatus::Pass,
        ConversationTurnPhase::Failed => TuiChecklistStatus::Fail,
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider => TuiChecklistStatus::Warn,
    }
}

fn cli_chat_live_finalize_detail(phase: ConversationTurnPhase) -> String {
    match phase {
        ConversationTurnPhase::FinalizingReply => {
            "persisting reply state and final runtime side effects".to_owned()
        }
        ConversationTurnPhase::Completed => "reply finalized".to_owned(),
        ConversationTurnPhase::Failed => "reply finalization did not complete".to_owned(),
        ConversationTurnPhase::Preparing
        | ConversationTurnPhase::ContextReady
        | ConversationTurnPhase::RequestingProvider
        | ConversationTurnPhase::RunningTools
        | ConversationTurnPhase::RequestingFollowupProvider => {
            "waiting for a final reply".to_owned()
        }
    }
}

fn build_cli_chat_live_status_items(snapshot: &CliChatLiveSurfaceSnapshot) -> Vec<TuiKeyValueSpec> {
    let mut items = Vec::new();

    items.push(TuiKeyValueSpec::Plain {
        key: "phase".to_owned(),
        value: snapshot.phase.as_str().to_owned(),
    });

    if let Some(provider_round) = snapshot.provider_round {
        items.push(TuiKeyValueSpec::Plain {
            key: "round".to_owned(),
            value: provider_round.to_string(),
        });
    }

    if let Some(lane) = snapshot.lane {
        items.push(TuiKeyValueSpec::Plain {
            key: "lane".to_owned(),
            value: format_cli_chat_live_lane(lane),
        });
    }

    if snapshot.tool_call_count > 0 {
        items.push(TuiKeyValueSpec::Plain {
            key: "tool calls".to_owned(),
            value: snapshot.tool_call_count.to_string(),
        });
    }

    if let Some(message_count) = snapshot.message_count {
        items.push(TuiKeyValueSpec::Plain {
            key: "context messages".to_owned(),
            value: message_count.to_string(),
        });
    }

    if let Some(estimated_tokens) = snapshot.estimated_tokens {
        items.push(TuiKeyValueSpec::Plain {
            key: "estimated tokens".to_owned(),
            value: estimated_tokens.to_string(),
        });
    }

    if let Some(first_token_latency_ms) = snapshot.first_token_latency_ms {
        items.push(TuiKeyValueSpec::Plain {
            key: "first token".to_owned(),
            value: format!("{first_token_latency_ms} ms"),
        });
    }

    items
}

fn format_cli_chat_live_lane(lane: ExecutionLane) -> String {
    match lane {
        ExecutionLane::Fast => "fast".to_owned(),
        ExecutionLane::Safe => "safe".to_owned(),
    }
}

fn build_cli_chat_live_preview_section(
    snapshot: &CliChatLiveSurfaceSnapshot,
    body_width: usize,
) -> Option<TuiSectionSpec> {
    let preview = snapshot.effective_preview()?;
    let preview_lines = render_live_preview_lines_from_blocks(preview.blocks.as_ref(), body_width);

    if preview_lines.is_empty() {
        return None;
    }

    if let Some(language) = live_preview_preformatted_language_from_blocks(preview.blocks.as_ref())
    {
        return Some(TuiSectionSpec::Preformatted {
            title: Some("draft preview".to_owned()),
            language: Some(language.to_owned()),
            lines: preview_lines,
        });
    }

    Some(TuiSectionSpec::Narrative {
        title: Some("draft preview".to_owned()),
        lines: preview_lines,
    })
}

fn live_preview_preformatted_language_from_blocks(
    blocks: &[CliChatLivePreviewBlock],
) -> Option<&'static str> {
    let blocks = blocks
        .iter()
        .filter(|block| !block.text.trim().is_empty())
        .collect::<Vec<_>>();
    let [block] = blocks.as_slice() else {
        return None;
    };
    let sanitized = sanitize_live_preview_text(block.text.as_str());
    live_preview_diff_body(sanitized.trim()).map(|_| "diff")
}

fn build_cli_chat_live_tool_section(
    snapshot: &CliChatLiveSurfaceSnapshot,
) -> Option<TuiSectionSpec> {
    if snapshot.tools.is_empty() {
        return None;
    }

    let lines = format_cli_chat_live_tool_activity_lines(snapshot.tools.as_slice());

    Some(TuiSectionSpec::Narrative {
        title: Some("tool activity".to_owned()),
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CliChatLiveFileChangeView, CliChatLiveOutputView, CliChatLivePreviewBlock,
        CliChatLivePreviewBlockKind, CliChatLiveSnapshotPreview, CliChatLiveSurfaceSink,
        CliChatLiveSurfaceSnapshot, CliChatLiveToolSnapshot,
        build_cli_chat_live_compact_observer_controller, build_cli_chat_live_surface_snapshot,
        render_cli_chat_live_compact_lines_with_width,
        render_cli_chat_live_surface_lines_with_width, render_live_preview_segment_lines,
    };
    use crate::conversation::{
        ConversationTurnPhase, ConversationTurnPhaseEvent, ConversationTurnToolState, ExecutionLane,
    };
    use crate::tools::runtime_events::ToolFileChangeKind;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn assert_uniform_display_width(lines: &[String]) {
        let Some(first_width) = lines
            .first()
            .map(|line| crate::presentation::display_width(line))
        else {
            return;
        };

        for line in lines {
            assert_eq!(
                crate::presentation::display_width(line),
                first_width,
                "table line has a different display width: {line:?}"
            );
        }
    }

    fn empty_output() -> CliChatLiveOutputView {
        CliChatLiveOutputView {
            text: String::new(),
            total_bytes: 0,
            total_lines: 0,
            truncated: false,
        }
    }

    #[test]
    fn compact_render_shows_preview_without_card_chrome() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(4),
            estimated_tokens: Some(1200),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "Hello there\nHow are you?".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);
        let joined = lines.join("\n");

        assert!(joined.contains("Hello there"));
        assert!(joined.contains("How are you?"));
        assert!(!joined.contains("╭─"));
        assert!(!joined.contains("turn pipeline"));
    }

    #[test]
    fn compact_render_includes_tool_activity_summary_without_card_chrome() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Safe),
            tool_call_count: 1,
            message_count: Some(6),
            estimated_tokens: Some(1800),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-1".to_owned(),
                name: Some("read_file".to_owned()),
                request_summary: Some("Read src/main.rs".to_owned()),
                args: "{\"path\":\"src/main.rs\"}".to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: Some(12),
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 60);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called read_file · working"));
        assert!(joined.contains("↳ Read src/main.rs"));
        assert!(!joined.contains("↳ args path=src/main.rs"));
        assert!(!joined.contains("↳ request"));
        assert!(joined.contains("↳ metrics 12ms"));
        assert!(!joined.contains("╭─"));
        assert!(!joined.contains("tool activity]"));
    }

    #[test]
    fn compact_render_compacts_structured_request_and_args_previews() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-2".to_owned(),
                name: Some("search".to_owned()),
                request_summary: Some(
                    "{\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
                ),
                args: "{\"query\":\"rust\",\"limit\":5,\"scope\":\"repo\"}".to_owned(),
                status: ConversationTurnToolState::Running,
                detail: None,
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 64);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called search"));
        assert!(joined.contains("↳ Search \"rust\" · limit 5"));
        assert!(joined.contains("↳ scope repo"));
        assert!(!joined.contains("↳ args"));
        assert!(!joined.contains("↳ request"));
        assert!(!joined.contains("↳ args query=rust"));
    }

    #[test]
    fn compact_render_promotes_command_request_into_primary_preview_line() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-bash".to_owned(),
                name: Some("bash".to_owned()),
                request_summary: None,
                args: "{\"cmd\":\"cargo test --workspace --all-features\"}".to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 72);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called bash · working"));
        assert!(joined.contains("↳ Command cargo test --workspace --all-features"));
        assert!(!joined.contains("↳ args"));
    }

    #[test]
    fn compact_render_promotes_search_request_into_primary_preview_line() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-search".to_owned(),
                name: Some("grep".to_owned()),
                request_summary: None,
                args: "{\"query\":\"稳定|wenjian|robust|stable\",\"path\":\"~/chat\",\"limit\":5}"
                    .to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called grep · working"));
        assert!(joined.contains("↳ Search \"稳定|wenjian|robust|stable\" in ~/chat · limit 5"));
        assert!(!joined.contains("↳ args"));
    }

    #[test]
    fn compact_render_promotes_glob_request_into_primary_preview_line() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-glob".to_owned(),
                name: Some("find_files".to_owned()),
                request_summary: None,
                args: "{\"glob\":\"src/**/*.rs\",\"path\":\"~/chat\",\"limit\":5,\"depth\":3,\"includeHidden\":true}"
                    .to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called find_files · working"));
        assert!(joined.contains("↳ Glob src/**/*.rs in ~/chat · limit 5"));
        assert!(joined.contains("↳ depth 3 · hidden on"));
    }

    #[test]
    fn compact_render_promotes_list_request_into_primary_preview_line() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-list".to_owned(),
                name: Some("list_directory".to_owned()),
                request_summary: None,
                args: "{\"path\":\"~/chat/.omx\",\"limit\":20,\"depth\":2,\"includeHidden\":true}"
                    .to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: empty_output(),
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);
        let joined = lines.join("\n");

        assert!(joined.contains("• Called list_directory · working"));
        assert!(joined.contains("↳ List ~/chat/.omx · limit 20"));
        assert!(joined.contains("↳ depth 2 · hidden on"));
    }

    #[test]
    fn run_tool_output_preview_prefers_tail_lines() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-run".to_owned(),
                name: Some("bash".to_owned()),
                request_summary: None,
                args: "{\"cmd\":\"cargo test\"}".to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: CliChatLiveOutputView {
                    text: "first\nsecond\nthird\nfourth\nfifth".to_owned(),
                    total_bytes: 30,
                    total_lines: 5,
                    truncated: false,
                },
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let joined = render_cli_chat_live_compact_lines_with_width(&snapshot, 72).join("\n");

        assert!(joined.contains("↳ stdout 5 lines · 30 bytes"));
        assert!(joined.contains("… +1 earlier lines"));
        assert!(!joined.contains("    first"));
        assert!(joined.contains("    second"));
        assert!(joined.contains("    fifth"));
    }

    #[test]
    fn inspect_tool_output_preview_prefers_head_lines() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-search".to_owned(),
                name: Some("grep".to_owned()),
                request_summary: None,
                args: "{\"query\":\"stable\",\"path\":\"~/chat\"}".to_owned(),
                status: ConversationTurnToolState::Running,
                detail: Some("working".to_owned()),
                stdout: CliChatLiveOutputView {
                    text: "match-one\nmatch-two\nmatch-three\nmatch-four\nmatch-five".to_owned(),
                    total_bytes: 48,
                    total_lines: 5,
                    truncated: false,
                },
                stderr: empty_output(),
                file_change: None,
                duration_ms: None,
                exit_code: None,
            }],
        };

        let joined = render_cli_chat_live_compact_lines_with_width(&snapshot, 72).join("\n");

        assert!(joined.contains("↳ stdout 5 lines · 48 bytes"));
        assert!(joined.contains("    match-one"));
        assert!(joined.contains("    match-four"));
        assert!(!joined.contains("    match-five"));
        assert!(joined.contains("… +1 more lines"));
    }

    #[test]
    fn compact_render_compacts_stderr_and_file_children() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 1,
            message_count: Some(3),
            estimated_tokens: Some(900),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![CliChatLiveToolSnapshot {
                tool_call_id: "call-3".to_owned(),
                name: Some("exec".to_owned()),
                request_summary: None,
                args: String::new(),
                status: ConversationTurnToolState::Completed,
                detail: Some("ok".to_owned()),
                stdout: empty_output(),
                stderr: CliChatLiveOutputView {
                    text: "permission denied".to_owned(),
                    total_bytes: 17,
                    total_lines: 1,
                    truncated: false,
                },
                file_change: Some(CliChatLiveFileChangeView {
                    path: "src/lib.rs".to_owned(),
                    operation: ToolFileChangeKind::Edit,
                    added_lines: 2,
                    removed_lines: 1,
                    preview: None,
                }),
                duration_ms: Some(42),
                exit_code: Some(0),
            }],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 64);
        let joined = lines.join("\n");

        assert!(joined.contains("• Closed exec · ok"));
        assert!(joined.contains("↳ stderr 1 lines · 17 bytes"));
        assert!(joined.contains("permission denied"));
        assert!(joined.contains("↳ file edit src/lib.rs (+2 / -1)"));
        assert!(joined.contains("↳ metrics 42ms · exit=0"));
    }

    #[test]
    fn compact_render_surfaces_approval_and_denied_status_lines() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RunningTools,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Safe),
            tool_call_count: 2,
            message_count: Some(5),
            estimated_tokens: Some(700),
            first_token_latency_ms: None,
            preview: None,
            tools: vec![
                CliChatLiveToolSnapshot {
                    tool_call_id: "call-a".to_owned(),
                    name: Some("search".to_owned()),
                    request_summary: None,
                    args: String::new(),
                    status: ConversationTurnToolState::NeedsApproval,
                    detail: Some("operator confirmation required".to_owned()),
                    stdout: empty_output(),
                    stderr: empty_output(),
                    file_change: None,
                    duration_ms: None,
                    exit_code: None,
                },
                CliChatLiveToolSnapshot {
                    tool_call_id: "call-b".to_owned(),
                    name: Some("search".to_owned()),
                    request_summary: None,
                    args: String::new(),
                    status: ConversationTurnToolState::Denied,
                    detail: Some("blocked".to_owned()),
                    stdout: empty_output(),
                    stderr: empty_output(),
                    file_change: None,
                    duration_ms: None,
                    exit_code: None,
                },
            ],
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 64);
        let joined = lines.join("\n");

        assert!(joined.contains("• Approval search · operator confirmation required"));
        assert!(joined.contains("• Denied search · blocked"));
    }

    #[test]
    fn compact_render_splits_think_blocks_into_reasoning_and_visible_reply() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(2),
            estimated_tokens: Some(512),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "<think>quiet reasoning\nsecond line</think>Hello there".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 50);
        let joined = lines.join("\n");

        assert!(joined.contains("quiet reasoning"));
        assert!(joined.contains("second line"));
        assert!(joined.contains("Hello there"));
        assert!(!joined.contains("<think>"));
        assert!(!joined.contains("</think>"));
    }

    #[test]
    fn compact_render_collapses_outer_and_repeated_blank_lines() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(2),
            estimated_tokens: Some(512),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "\n\n<think>reasoning line</think>\n\n\nvisible reply\n\n".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 50);
        assert_eq!(
            lines,
            vec![
                "reasoning line".to_owned(),
                String::new(),
                "visible reply".to_owned()
            ]
        );
    }

    #[test]
    fn compact_render_hides_partial_think_tag_prefixes() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(128),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text("<thi".to_owned())),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);

        assert!(lines.is_empty());
    }

    #[test]
    fn compact_render_keeps_incomplete_trailing_paragraph_literal_while_streaming() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "hello\nworld from stream".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);

        assert_eq!(
            lines,
            vec!["hello".to_owned(), "world from stream".to_owned()]
        );
    }

    #[test]
    fn compact_render_drops_trailing_unclosed_code_fence_suffix() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "hello world\n```".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);

        assert_eq!(lines, vec!["hello world".to_owned()]);
    }

    #[test]
    fn compact_render_keeps_visible_text_when_partial_closing_think_tag_arrives() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "visible answer</t".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 80);

        assert_eq!(lines, vec!["visible answer".to_owned()]);
    }

    #[test]
    fn compact_render_structures_markdown_tables_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "| 指标 | 数值 |\n| --- | --- |\n| 覆盖率 | 68% |\n| 平均响应时间 | 220ms |"
                    .to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);
        let joined = lines.join("\n");

        assert!(joined.contains("┌"));
        assert!(joined.contains("覆盖率"));
        assert!(joined.contains("220ms"));
        assert!(!joined.contains("| --- |"));
    }

    #[test]
    fn compact_render_structures_provisional_markdown_tables_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "| Name | Value |\n| A | 1 |\n| B | 2 |".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 32);

        assert_eq!(
            lines,
            vec![
                "┌──────┬───────┐".to_owned(),
                "│ Name │ Value │".to_owned(),
                "├──────┼───────┤".to_owned(),
                "│ A    │ 1     │".to_owned(),
                "│ B    │ 2     │".to_owned(),
                "└──────┴───────┘".to_owned(),
            ]
        );
        assert_uniform_display_width(lines.as_slice());
    }

    #[test]
    fn surface_render_structures_markdown_tables_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "| 指标 | 数值 |\n| --- | --- |\n| 覆盖率 | 68% |\n| 平均响应时间 | 220ms |"
                    .to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_surface_lines_with_width(&snapshot, 72);
        let joined = lines.join("\n");

        assert!(joined.contains("draft preview"));
        assert!(joined.contains("┌"));
        assert!(joined.contains("覆盖率"));
        assert!(joined.contains("220ms"));
        assert!(!joined.contains("| --- |"));
    }

    #[test]
    fn compact_render_structures_fenced_diff_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "```diff\n- old value\n+ new value\n```".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 50);
        let joined = lines.join("\n");

        assert!(joined.contains("- old value"));
        assert!(joined.contains("+ new value"));
        assert!(!joined.contains("```diff"));
    }

    #[test]
    fn compact_render_structures_provisional_diff_fence_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "```di\n- old value\n+ new value".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 50);
        let joined = lines.join("\n");

        assert!(joined.contains("- old value"), "{joined}");
        assert!(joined.contains("+ new value"), "{joined}");
        assert!(!joined.contains("```di"), "{joined}");
    }

    #[test]
    fn surface_render_structures_fenced_diff_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "```diff\n- old value\n+ new value\n```".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_surface_lines_with_width(&snapshot, 72);
        let joined = lines.join("\n");

        assert!(joined.contains("draft preview"), "{joined}");
        assert!(joined.contains("- old value"), "{joined}");
        assert!(joined.contains("+ new value"), "{joined}");
        assert!(!joined.contains("```diff"), "{joined}");
    }

    #[test]
    fn compact_render_structures_fenced_code_block_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "```bash\nnpm install\nnpm test\n```".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);
        let joined = lines.join("\n");

        assert!(joined.contains("```bash"));
        assert!(joined.contains("npm install"));
        assert!(joined.contains("npm test"));
        assert!(joined.contains("```"));
    }

    #[test]
    fn compact_render_structures_markdown_list_in_preview() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::RequestingProvider,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(256),
            first_token_latency_ms: None,
            preview: Some(CliChatLiveSnapshotPreview::from_text(
                "## 本周进展\n- 修复崩溃\n- 提升性能".to_owned(),
            )),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);
        let joined = lines.join("\n");

        assert!(joined.contains("## 本周进展"));
        assert!(joined.contains("• 修复崩溃"));
        assert!(joined.contains("• 提升性能"));
    }

    #[test]
    fn preview_emit_waits_for_a_stable_initial_boundary() {
        let mut state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "hel".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!state.preview.should_emit(40, Some(3)));

        state.preview.preview_buffer.push(' ');

        assert!(state.preview.should_emit(40, Some(4)));
    }

    #[test]
    fn preview_emit_allows_a_readable_initial_phrase() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "Draft response".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(72, Some(42)));
    }

    #[test]
    fn preview_emit_forces_progress_after_large_unstable_burst() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "averylongunstablesuffixwithoutbreaks".to_owned(),
                emit: super::CliChatLivePreviewEmitCache {
                    chars_seen: 8,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(8, Some(24)));
    }

    #[test]
    fn preview_emit_uses_visual_line_pressure_for_wrapped_cjk_text() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "渲染表格边界".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(8, Some(5)));
    }

    #[test]
    fn preview_emit_allows_initial_key_value_structure_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "- mode: enabled".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_allows_initial_inline_list_structure_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "1. inspect 2. fix".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_allows_initial_markdown_heading_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "## progress update".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_allows_initial_unordered_list_structure_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "- inspect the repo".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_allows_initial_markdown_table_row_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "| Name | Value |".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_uses_display_text_even_when_raw_counter_is_zero() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_blocks: vec![CliChatLivePreviewBlock {
                    kind: CliChatLivePreviewBlockKind::Visible,
                    text: "hello world".to_owned(),
                }],
                preview_display_text: "hello world".to_owned(),
                preview_display_char_count: "hello world".chars().count(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_does_not_count_raw_think_tag_chars_without_visible_text() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "<think>".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn effective_preview_display_text_prefers_preparsed_blocks_without_tags() {
        let blocks = vec![
            CliChatLivePreviewBlock {
                kind: CliChatLivePreviewBlockKind::Visible,
                text: "intro".to_owned(),
            },
            CliChatLivePreviewBlock {
                kind: CliChatLivePreviewBlockKind::Thinking,
                text: "reasoning".to_owned(),
            },
            CliChatLivePreviewBlock {
                kind: CliChatLivePreviewBlockKind::Visible,
                text: "answer".to_owned(),
            },
        ];

        let effective = super::cli_chat_live_effective_preview(
            "intro<think>reasoning</think>answer",
            blocks.as_slice(),
            Some(""),
        );

        assert_eq!(effective.display_text.as_str(), "introreasoninganswer");
        assert_eq!(
            effective.display_char_count,
            "introreasoninganswer".chars().count()
        );
        assert_eq!(effective.blocks.len(), 3);
        assert!(effective.tag_carry_empty);
        assert!(!effective.has_complete_tag_boundary);
    }

    #[test]
    fn effective_preview_marks_partial_tag_carry_as_unstable() {
        let effective = super::cli_chat_live_effective_preview("intro<th", &[], Some("<th"));

        assert_eq!(effective.display_text.as_str(), "intro");
        assert_eq!(effective.display_char_count, "intro".chars().count());
        assert!(!effective.tag_carry_empty);
        assert!(!effective.has_complete_tag_boundary);
    }

    #[test]
    fn effective_preview_from_state_reuses_cached_blocks_and_display_text() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "intro<think>reasoning</think>answer".to_owned(),
                preview_blocks: vec![
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Visible,
                        text: "intro".to_owned(),
                    },
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Thinking,
                        text: "reasoning".to_owned(),
                    },
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Visible,
                        text: "answer".to_owned(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = state.preview.effective_preview_or_reparse();

        assert_eq!(effective.blocks.len(), 3);
        assert_eq!(effective.display_text.as_str(), "introreasoninganswer");
        assert_eq!(
            effective.display_char_count,
            "introreasoninganswer".chars().count()
        );
        assert!(effective.tag_carry_empty);
        assert!(!effective.has_complete_tag_boundary);
    }

    #[test]
    fn effective_preview_from_state_falls_back_to_raw_buffer_when_cache_is_empty() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "intro<think>reasoning</think>answer".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = state.preview.effective_preview_or_reparse();

        assert_eq!(effective.blocks.len(), 3);
        assert_eq!(effective.display_text.as_str(), "introreasoninganswer");
        assert_eq!(
            effective.display_char_count,
            "introreasoninganswer".chars().count()
        );
    }

    #[test]
    fn preview_cache_append_chunk_updates_display_text_and_blocks() {
        let mut preview = super::CliChatLivePreviewCache::default();

        preview.append_chunk("intro<th");
        preview.append_chunk("ink>reason");
        preview.append_chunk("ing</thin");
        preview.append_chunk("k>answer");

        assert_eq!(preview.preview_display_text, "introreasoninganswer");
        assert_eq!(
            preview.preview_display_char_count,
            "introreasoninganswer".chars().count()
        );
        assert_eq!(preview.preview_blocks.len(), 3);
        assert_eq!(preview.preview_blocks[0].text, "intro");
        assert_eq!(preview.preview_blocks[1].text, "reasoning");
        assert_eq!(preview.preview_blocks[2].text, "answer");
        assert!(preview.preview_tag_carry.is_empty());
    }

    #[test]
    fn preview_cache_append_delta_rebuilds_after_truncation() {
        let mut preview = super::CliChatLivePreviewCache::default();
        let chunk = "x".repeat(super::CLI_CHAT_LIVE_PREVIEW_MAX_BUFFER_CHARS + 32);

        preview.append_delta(chunk.as_str(), 80);

        assert_eq!(preview.preview_blocks.len(), 1);
        assert!(preview.preview_blocks[0].text.starts_with('…'));
        assert_eq!(preview.preview_display_text, preview.preview_blocks[0].text);
        assert_eq!(
            preview.preview_display_char_count,
            preview.preview_blocks[0].text.chars().count()
        );
    }

    #[test]
    fn preview_emit_allows_initial_markdown_quote_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "> quoted progress".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_allows_initial_code_fence_earlier() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "```rust".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(80, Some(12)));
    }

    #[test]
    fn preview_emit_ignores_partial_think_tag_for_initial_phrase_boundary() {
        let mut state = super::CliChatLiveSurfaceState::default();

        super::append_cli_chat_live_preview_delta(&mut state, "hello <th", 80, Some(9));

        assert!(!state.preview.should_emit(80, Some(9)));

        super::append_cli_chat_live_preview_delta(&mut state, "ink>world", 80, Some(10));

        assert!(state.preview.should_emit(80, Some(10)));
    }

    #[test]
    fn append_preview_delta_updates_preview_blocks_and_first_token_latency() {
        let mut state = super::CliChatLiveSurfaceState::default();

        super::append_cli_chat_live_preview_delta(&mut state, "intro<think>", 80, Some(23));
        super::append_cli_chat_live_preview_delta(
            &mut state,
            "reasoning</think>answer",
            80,
            Some(24),
        );

        assert_eq!(state.first_token_latency_ms, Some(23));
        assert_eq!(state.preview.preview_display_text, "introreasoninganswer");
        assert_eq!(
            state.preview.preview_display_char_count,
            "introreasoninganswer".chars().count()
        );
        assert_eq!(state.preview.preview_blocks.len(), 3);
        assert_eq!(
            state.preview.preview_blocks[0].kind,
            CliChatLivePreviewBlockKind::Visible
        );
        assert_eq!(state.preview.preview_blocks[0].text, "intro");
        assert_eq!(
            state.preview.preview_blocks[1].kind,
            CliChatLivePreviewBlockKind::Thinking
        );
        assert_eq!(state.preview.preview_blocks[1].text, "reasoning");
        assert_eq!(
            state.preview.preview_blocks[2].kind,
            CliChatLivePreviewBlockKind::Visible
        );
        assert_eq!(state.preview.preview_blocks[2].text, "answer");
    }

    #[test]
    fn append_preview_delta_handles_split_think_tags_incrementally() {
        let mut state = super::CliChatLiveSurfaceState::default();

        super::append_cli_chat_live_preview_delta(&mut state, "intro<th", 80, Some(10));
        super::append_cli_chat_live_preview_delta(&mut state, "ink>reason", 80, Some(11));
        super::append_cli_chat_live_preview_delta(&mut state, "ing</thin", 80, Some(12));
        super::append_cli_chat_live_preview_delta(&mut state, "k>answer", 80, Some(13));

        assert_eq!(state.preview.preview_blocks.len(), 3);
        assert_eq!(state.preview.preview_display_text, "introreasoninganswer");
        assert_eq!(
            state.preview.preview_display_char_count,
            "introreasoninganswer".chars().count()
        );
        assert_eq!(state.preview.preview_blocks[0].text, "intro");
        assert_eq!(state.preview.preview_blocks[1].text, "reasoning");
        assert_eq!(state.preview.preview_blocks[2].text, "answer");
        assert!(state.preview.preview_tag_carry.is_empty());
        assert_eq!(
            state.preview.preview_parse_kind,
            CliChatLivePreviewBlockKind::Visible
        );
    }

    #[test]
    fn append_preview_delta_rebuilds_blocks_after_buffer_truncation() {
        let mut state = super::CliChatLiveSurfaceState::default();
        let chunk = "x".repeat(super::CLI_CHAT_LIVE_PREVIEW_MAX_BUFFER_CHARS + 32);

        super::append_cli_chat_live_preview_delta(&mut state, chunk.as_str(), 80, Some(10));

        assert_eq!(state.preview.preview_blocks.len(), 1);
        assert_eq!(
            state.preview.preview_display_text,
            state.preview.preview_blocks[0].text
        );
        assert_eq!(
            state.preview.preview_display_char_count,
            state.preview.preview_blocks[0].text.chars().count()
        );
        assert_eq!(
            state.preview.preview_blocks[0].kind,
            CliChatLivePreviewBlockKind::Visible
        );
        assert!(state.preview.preview_blocks[0].text.starts_with('…'));
        assert!(state.preview.preview_tag_carry.is_empty());
        assert_eq!(
            state.preview.preview_parse_kind,
            CliChatLivePreviewBlockKind::Visible
        );
    }

    #[test]
    fn preview_emit_mode_enters_catch_up_when_visual_backlog_grows() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer:
                    "line one wraps quickly\nline two wraps quickly\nline three wraps quickly"
                        .to_owned(),
                emit: super::CliChatLivePreviewEmitCache {
                    chars_seen: 8,
                    visual_line_count: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            state.preview.emit_mode(18),
            super::CliChatLivePreviewEmitMode::CatchUp
        );
    }

    #[test]
    fn preview_emit_mode_stays_smooth_for_small_stable_updates() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "hello world ".to_owned(),
                emit: super::CliChatLivePreviewEmitCache {
                    chars_seen: 8,
                    visual_line_count: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            state.preview.emit_mode(80),
            super::CliChatLivePreviewEmitMode::Smooth
        );
    }

    #[test]
    fn preview_emit_cadence_slows_smooth_mode() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "hello world this is a stable preview chunk with just enough size "
                    .to_owned(),
                emit: super::CliChatLivePreviewEmitCache {
                    chars_seen: 8,
                    visual_line_count: 1,
                    elapsed_ms: Some(100),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!state.preview.should_emit(80, Some(120)));
        assert!(state.preview.should_emit(80, Some(140)));
    }

    #[test]
    fn preview_emit_cadence_keeps_catch_up_faster_than_smooth() {
        let state = super::CliChatLiveSurfaceState {
            preview: super::CliChatLivePreviewCache {
                preview_buffer:
                    "line one wraps quickly\nline two wraps quickly\nline three wraps quickly"
                        .to_owned(),
                emit: super::CliChatLivePreviewEmitCache {
                    chars_seen: 8,
                    visual_line_count: 1,
                    elapsed_ms: Some(100),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(state.preview.should_emit(18, Some(118)));
    }

    #[test]
    fn delta_commit_boundary_detects_newline_and_structural_tokens() {
        assert!(super::cli_chat_live_delta_has_commit_boundary(
            "line done\n"
        ));
        assert!(super::cli_chat_live_delta_has_commit_boundary("<think>"));
        assert!(super::cli_chat_live_delta_has_commit_boundary("</think>"));
        assert!(super::cli_chat_live_delta_has_commit_boundary("```rust"));
        assert!(!super::cli_chat_live_delta_has_commit_boundary(
            "plain delta"
        ));
    }

    #[test]
    fn compact_observer_rerenders_preview_when_width_changes() {
        let captured_batches = Arc::new(StdMutex::new(Vec::<Vec<String>>::new()));
        let render_sink: CliChatLiveSurfaceSink = {
            let captured_batches = Arc::clone(&captured_batches);
            Arc::new(move |lines| {
                let mut batches = captured_batches
                    .lock()
                    .expect("captured batches lock should not be poisoned");
                batches.push(lines);
            })
        };
        let render_width = Arc::new(AtomicUsize::new(32));
        let (observer, rerender) =
            build_cli_chat_live_compact_observer_controller(Arc::clone(&render_width), render_sink);

        observer.on_phase(ConversationTurnPhaseEvent::requesting_provider(
            1,
            3,
            Some(96),
        ));
        observer.on_streaming_token(crate::acp::StreamingTokenEvent {
            event_type: "text_delta".to_owned(),
            delta: crate::acp::TokenDelta {
                text: Some("alpha beta gamma delta epsilon".to_owned()),
                tool_call: None,
            },
            index: None,
            elapsed_ms: Some(42),
        });

        render_width.store(12, Ordering::Relaxed);
        rerender();

        let batches = captured_batches
            .lock()
            .expect("captured batches lock should not be poisoned");
        let last_batch = batches.last().expect("rerender batch");
        assert!(last_batch.len() > 1);
        assert!(last_batch.iter().any(|line| line.contains("alpha beta")));
    }

    #[test]
    fn compact_observer_commits_preview_immediately_on_newline_boundary() {
        let captured_batches = Arc::new(StdMutex::new(Vec::<Vec<String>>::new()));
        let render_sink: CliChatLiveSurfaceSink = {
            let captured_batches = Arc::clone(&captured_batches);
            Arc::new(move |lines| {
                let mut batches = captured_batches
                    .lock()
                    .expect("captured batches lock should not be poisoned");
                batches.push(lines);
            })
        };
        let render_width = Arc::new(AtomicUsize::new(80));
        let (observer, _) =
            build_cli_chat_live_compact_observer_controller(Arc::clone(&render_width), render_sink);

        observer.on_phase(ConversationTurnPhaseEvent::requesting_provider(
            1,
            3,
            Some(32),
        ));
        observer.on_streaming_token(crate::acp::StreamingTokenEvent {
            event_type: "text_delta".to_owned(),
            delta: crate::acp::TokenDelta {
                text: Some("ok\n".to_owned()),
                tool_call: None,
            },
            index: None,
            elapsed_ms: Some(8),
        });

        let batches = captured_batches
            .lock()
            .expect("captured batches lock should not be poisoned");
        let last_batch = batches.last().expect("newline-triggered batch");
        assert!(last_batch.iter().any(|line| line.contains("ok")));
    }

    #[test]
    fn compact_observer_skips_rerender_when_width_change_keeps_same_lines() {
        let captured_batches = Arc::new(StdMutex::new(Vec::<Vec<String>>::new()));
        let render_sink: CliChatLiveSurfaceSink = {
            let captured_batches = Arc::clone(&captured_batches);
            Arc::new(move |lines| {
                let mut batches = captured_batches
                    .lock()
                    .expect("captured batches lock should not be poisoned");
                batches.push(lines);
            })
        };
        let render_width = Arc::new(AtomicUsize::new(80));
        let (observer, rerender) =
            build_cli_chat_live_compact_observer_controller(Arc::clone(&render_width), render_sink);

        observer.on_phase(ConversationTurnPhaseEvent::requesting_provider(
            1,
            3,
            Some(64),
        ));
        observer.on_streaming_token(crate::acp::StreamingTokenEvent {
            event_type: "text_delta".to_owned(),
            delta: crate::acp::TokenDelta {
                text: Some("short line ".to_owned()),
                tool_call: None,
            },
            index: None,
            elapsed_ms: Some(10),
        });

        let batch_count_before = captured_batches
            .lock()
            .expect("captured batches lock should not be poisoned")
            .len();

        render_width.store(79, Ordering::Relaxed);
        rerender();

        let batch_count_after = captured_batches
            .lock()
            .expect("captured batches lock should not be poisoned")
            .len();

        assert_eq!(batch_count_after, batch_count_before);
    }

    #[test]
    fn parse_live_preview_blocks_preserves_alternating_thinking_and_visible_segments() {
        let parsed = super::parse_live_preview_state(
            "intro<think>reasoning one</think>middle<think>reasoning two</think>tail",
            CliChatLivePreviewBlockKind::Visible,
        );
        let blocks = parsed.blocks;

        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].kind, super::CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[0].text, "intro");
        assert_eq!(blocks[1].kind, super::CliChatLivePreviewBlockKind::Thinking);
        assert_eq!(blocks[1].text, "reasoning one");
        assert_eq!(blocks[2].kind, super::CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[2].text, "middle");
        assert_eq!(blocks[3].kind, super::CliChatLivePreviewBlockKind::Thinking);
        assert_eq!(blocks[3].text, "reasoning two");
        assert_eq!(blocks[4].kind, super::CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[4].text, "tail");
    }

    #[test]
    fn parse_live_preview_blocks_ignores_partial_trailing_think_marker() {
        let parsed = super::parse_live_preview_state(
            "visible answer</t",
            CliChatLivePreviewBlockKind::Visible,
        );
        let blocks = parsed.blocks;

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, super::CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[0].text, "visible answer");
    }

    #[test]
    fn build_snapshot_reconstructs_preview_blocks_when_cache_is_empty() {
        let state = super::CliChatLiveSurfaceState {
            latest_phase_event: Some(ConversationTurnPhaseEvent {
                phase: ConversationTurnPhase::FinalizingReply,
                provider_round: Some(1),
                lane: Some(ExecutionLane::Fast),
                tool_call_count: 0,
                message_count: Some(1),
                estimated_tokens: Some(64),
            }),
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "intro<think>reasoning</think>answer".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        let snapshot = build_cli_chat_live_surface_snapshot(&state).expect("snapshot");
        let blocks = snapshot
            .preview
            .as_ref()
            .map(|preview| preview.blocks.clone())
            .expect("preview blocks");
        assert_eq!(
            snapshot
                .preview
                .as_ref()
                .and_then(CliChatLiveSnapshotPreview::text),
            Some("introreasoninganswer")
        );

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].kind, CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[0].text, "intro");
        assert_eq!(blocks[1].kind, CliChatLivePreviewBlockKind::Thinking);
        assert_eq!(blocks[1].text, "reasoning");
        assert_eq!(blocks[2].kind, CliChatLivePreviewBlockKind::Visible);
        assert_eq!(blocks[2].text, "answer");
    }

    #[test]
    fn build_snapshot_prefers_cached_display_text_over_raw_think_tags() {
        let state = super::CliChatLiveSurfaceState {
            latest_phase_event: Some(ConversationTurnPhaseEvent {
                phase: ConversationTurnPhase::FinalizingReply,
                provider_round: Some(1),
                lane: Some(ExecutionLane::Fast),
                tool_call_count: 0,
                message_count: Some(1),
                estimated_tokens: Some(64),
            }),
            preview: super::CliChatLivePreviewCache {
                preview_buffer: "intro<think>reasoning</think>answer".to_owned(),
                preview_blocks: vec![
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Visible,
                        text: "intro".to_owned(),
                    },
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Thinking,
                        text: "reasoning".to_owned(),
                    },
                    CliChatLivePreviewBlock {
                        kind: CliChatLivePreviewBlockKind::Visible,
                        text: "answer".to_owned(),
                    },
                ],
                preview_display_text: "introreasoninganswer".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        let snapshot = build_cli_chat_live_surface_snapshot(&state).expect("snapshot");

        assert_eq!(
            snapshot
                .preview
                .as_ref()
                .and_then(CliChatLiveSnapshotPreview::text),
            Some("introreasoninganswer")
        );
        assert_eq!(
            snapshot
                .preview
                .as_ref()
                .and_then(CliChatLiveSnapshotPreview::blocks)
                .map(|blocks| blocks.len()),
            Some(3)
        );
    }

    #[test]
    fn build_snapshot_hides_partial_raw_tag_suffix_when_display_text_is_stable() {
        let mut state = super::CliChatLiveSurfaceState {
            latest_phase_event: Some(ConversationTurnPhaseEvent {
                phase: ConversationTurnPhase::RequestingProvider,
                provider_round: Some(1),
                lane: Some(ExecutionLane::Fast),
                tool_call_count: 0,
                message_count: Some(1),
                estimated_tokens: Some(32),
            }),
            ..Default::default()
        };

        super::append_cli_chat_live_preview_delta(&mut state, "intro<th", 80, Some(9));

        let snapshot = build_cli_chat_live_surface_snapshot(&state).expect("snapshot");

        assert_eq!(
            snapshot
                .preview
                .as_ref()
                .and_then(CliChatLiveSnapshotPreview::text),
            Some("intro")
        );
        assert_eq!(
            snapshot
                .preview
                .as_ref()
                .and_then(CliChatLiveSnapshotPreview::blocks)
                .map(|blocks| blocks.len()),
            Some(1)
        );
    }

    #[test]
    fn preview_cache_snapshot_preview_derives_text_from_effective_preview_when_cache_text_is_empty()
    {
        let preview = super::CliChatLivePreviewCache {
            preview_buffer: "intro<think>reasoning</think>answer".to_owned(),
            ..Default::default()
        };

        let snapshot_preview = preview.snapshot_preview().expect("snapshot preview");

        assert_eq!(snapshot_preview.text(), Some("introreasoninganswer"));
        assert_eq!(
            snapshot_preview.blocks().map(|blocks| blocks.len()),
            Some(3)
        );
    }

    #[test]
    fn compact_render_uses_preparsed_preview_blocks_without_raw_preview_text() {
        let snapshot = CliChatLiveSurfaceSnapshot {
            phase: ConversationTurnPhase::FinalizingReply,
            provider_round: Some(1),
            lane: Some(ExecutionLane::Fast),
            tool_call_count: 0,
            message_count: Some(1),
            estimated_tokens: Some(32),
            first_token_latency_ms: Some(12),
            preview: Some(CliChatLiveSnapshotPreview::from_blocks(vec![
                CliChatLivePreviewBlock {
                    kind: CliChatLivePreviewBlockKind::Thinking,
                    text: "quiet reasoning".to_owned(),
                },
                CliChatLivePreviewBlock {
                    kind: CliChatLivePreviewBlockKind::Visible,
                    text: "visible answer".to_owned(),
                },
            ])),
            tools: Vec::new(),
        };

        let lines = render_cli_chat_live_compact_lines_with_width(&snapshot, 40);
        let joined = lines.join("\n");

        assert!(joined.contains("quiet reasoning"));
        assert!(joined.contains("visible answer"));
    }

    #[test]
    fn live_preview_keeps_command_lines_split_after_label() {
        let lines = render_live_preview_segment_lines(
            "Command:\ncargo test --workspace --all-features",
            80,
        );

        assert_eq!(
            lines,
            vec![
                "Command:".to_owned(),
                "cargo test --workspace --all-features".to_owned(),
            ]
        );
    }

    #[test]
    fn live_preview_splits_inline_numbered_runs_into_separate_lines() {
        let lines = render_live_preview_segment_lines(
            "1. inspect the repo 2. fix the bug 3. run tests",
            80,
        );

        assert_eq!(
            lines,
            vec![
                "1. inspect the repo".to_owned(),
                "2. fix the bug".to_owned(),
                "3. run tests".to_owned(),
            ]
        );
    }

    #[test]
    fn live_preview_wraps_key_value_lines_with_continuation_indent() {
        let lines = render_live_preview_segment_lines(
            "- streaming renderer: enabled with a longer explanation that must wrap",
            28,
        );

        assert!(lines[0].starts_with("- streaming"));
        assert!(lines.len() >= 2);
        assert!(lines[1].starts_with("                      "));
    }

    #[test]
    fn live_preview_keeps_path_lines_split_after_label() {
        let lines = render_live_preview_segment_lines("Path:\n~/chat/.omx/state.json", 80);

        assert_eq!(
            lines,
            vec!["Path:".to_owned(), "~/chat/.omx/state.json".to_owned(),]
        );
    }

    #[test]
    fn live_preview_reflows_plain_paragraph_lines_into_one_paragraph() {
        let lines = render_live_preview_segment_lines(
            "This is the first sentence\nthat continues the same thought\nbefore the next block.",
            120,
        );

        assert_eq!(
            lines,
            vec![
                "This is the first sentence that continues the same thought before the next block."
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn live_preview_reflows_cjk_paragraph_lines_without_extra_spaces() {
        let lines = render_live_preview_segment_lines("这是第一句\n继续说明同一段内容", 120);

        assert_eq!(lines, vec!["这是第一句继续说明同一段内容".to_owned(),]);
    }

    #[test]
    fn live_preview_keeps_logfmt_lines_out_of_paragraph_reflow() {
        let lines = render_live_preview_segment_lines(
            "prefix\n2026-04-25T11:02:58.547678Z WARN Loong.tools: tool execution failed requested_tool_name=file.read payload_kind=object duration_ms=0",
            96,
        );

        assert_eq!(lines.first().map(String::as_str), Some("prefix"));
        assert!(lines.iter().any(|line| line.contains("WARN Loong.tools:")));
        assert!(!lines[0].contains("WARN Loong.tools:"));
    }

    #[test]
    fn live_preview_preserves_code_like_lines_without_markdown_fence() {
        let lines = render_live_preview_segment_lines(
            "import \"strings\"\nconst (\n    openAIToolCallTypeCustom = \"custom_tool_call\"\n)\nfunc RequiresOpenAIWSV2Continuation(reqBody map[string]any) bool {\n    return false\n}",
            96,
        );

        assert!(lines.iter().any(|line| line == "import \"strings\""));
        assert!(lines.iter().any(|line| line.contains("const (")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("func RequiresOpenAIWSV2Continuation"))
        );
    }

    #[test]
    fn preview_emit_stride_is_more_responsive_on_narrow_widths() {
        assert_eq!(super::cli_chat_live_preview_emit_stride(4), 8);
        assert_eq!(super::cli_chat_live_preview_emit_stride(12), 12);
        assert_eq!(super::cli_chat_live_preview_emit_stride(20), 20);
        assert_eq!(super::cli_chat_live_preview_emit_stride(80), 48);
    }

    #[test]
    fn preview_buffer_limit_stays_large_even_for_narrow_widths() {
        assert_eq!(
            super::cli_chat_live_preview_char_limit(12),
            super::CLI_CHAT_LIVE_PREVIEW_MAX_BUFFER_CHARS
        );
        assert_eq!(
            super::cli_chat_live_tool_args_char_limit(12),
            super::CLI_CHAT_LIVE_TOOL_ARGS_MAX_BUFFER_CHARS
        );
        assert_eq!(
            super::cli_chat_live_output_char_limit(12),
            super::CLI_CHAT_LIVE_OUTPUT_MAX_BUFFER_CHARS
        );
    }
}
