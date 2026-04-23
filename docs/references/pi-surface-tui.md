# Pi Surface TUI Reference

This note documents the current operator-facing expectations for
`crates/app/src/chat/pi_surface/`.

## UX commitments

- **Pi Mono-aligned composer** keeps the borderless prompt layout and now keeps
  cursor placement aligned with wrapped content width instead of re-applying the
  first-line prompt offset after every wrap.
- **Pending-turn streaming** shows the live draft/tool-activity preview inline
  while a turn is still running, so “show thinking” style feedback is visible
  before the final assistant message lands. Live preview cadence now uses a
  smooth/catch-up split so long wrapped bursts, newline boundaries, and CJK
  growth force progress without turning ordinary token flow into flicker.
- **Command deck and control-plane copy** is routed through
  `pi_surface/i18n.rs`, which centralizes slash-command descriptions and keeps
  future locale-specific routing isolated from the widget code.
- **Structured preview consistency** keeps diff, markdown, table, and code-like
  content readable in both the pending area and the settled transcript. Live
  preview uses lighter structure-aware rendering, while the transcript keeps the
  richer final layout with width-aware markdown table fallback.
- **Calmer tool activity groups** collapse duplicate tool/subagent burst noise
  into stable called/request/file/metrics/stdout/stderr groups so runtime-heavy
  turns stay readable without hiding unique work.
- **Control-plane surfaces** remain reachable from both the command palette and
  the startup surface: `/sessions`, `/workers`, `/review`, and `/mission`.

## Files to review when changing this surface

- `crates/app/src/chat/pi_surface/app.rs` — overall layout, pending-turn panel,
  startup sections, focus handling.
- `crates/app/src/chat/pi_surface/composer.rs` — input editing, cursor
  positioning, wrap math.
- `crates/app/src/chat/pi_surface/command_palette.rs` — slash-command deck UI.
- `crates/app/src/chat/pi_surface/diff_viewer.rs` — width-aware diff block
  rendering shared by transcript surfaces.
- `crates/app/src/chat/pi_surface/i18n.rs` — user-facing copy routing for the
  command deck/startup helper text.
- `crates/app/src/chat/pi_surface/markdown.rs` — markdown/image/table parsing,
  grid rendering, and narrow-width fallback.
- `crates/app/src/chat/pi_surface/message_list.rs` — transcript rendering,
  burst compaction, tool-activity grouping, and resize anchor protections.
- `crates/app/src/chat/pi_surface/utils.rs` — structured preview compaction for
  request/args summaries.
- `crates/app/src/chat/cli_render.rs` — structure-aware assistant-section
  parsing that promotes diff/tool-activity sections into typed transcript
  blocks.
- `crates/app/src/chat/live_runtime.rs` — the live preview/tool-activity data
  that feeds the pending-turn surface, including structured-preview cadence.
- `docs/references/pi-surface-resize-render-audit.md` — resize flicker audit,
  current protections, and follow-up rendering recommendations.

## Current verification evidence

The current maturity checkpoint is protected by targeted `loong-app` tests that
cover the operator-facing promises above:

- `live_runtime`
  - `preview_emit_forces_progress_after_large_unstable_burst`
  - `preview_emit_mode_enters_catch_up_when_visual_backlog_grows`
  - `preview_emit_cadence_keeps_catch_up_faster_than_smooth`
  - `compact_observer_rerenders_preview_when_width_changes`
- `pi_surface/message_list`
  - `assistant_markdown_table_renders_as_structured_grid`
  - `rendered_lines_are_pre_padded_for_stable_cached_redraws`
  - `resize_preserves_top_visible_line_when_scrolled_up`
  - `width_resize_preserves_bottom_anchor_for_wrapped_tail_content`
  - `tool_activity_burst_keeps_unique_request_children_per_called_group`
- `pi_surface/app`
  - `pending_footer_yields_to_queue_hint_when_draft_exists`
  - `pending_footer_shows_restore_hint_when_queue_exists`
  - `width_resize_keeps_provider_error_and_footer_visible`
  - `width_resize_keeps_pending_restore_footer_and_previews_visible`
  - `pending_preview_shows_queued_steer_and_follow_up_above_composer`

Run:

- `cargo test -p loong-app live_runtime -- --nocapture`
- `cargo test -p loong-app pi_surface -- --nocapture`
- `cargo check -p loong-app --tests`
- `cargo clippy -p loong-app --tests -- -D warnings`
