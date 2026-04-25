# Chat Surface TUI Reference

This note documents the current operator-facing expectations for
`crates/app/src/chat/chat_surface/`.

## UX commitments

- **Loong-aligned composer** keeps the borderless prompt layout and now keeps
  cursor placement aligned with wrapped content width instead of re-applying the
  first-line prompt offset after every wrap.
- **Pending-turn streaming** shows the live draft/tool-activity preview inline
  while a turn is still running, so “show thinking” style feedback is visible
  before the final assistant message lands. Live preview cadence now uses a
  smooth/catch-up split so long wrapped bursts, newline boundaries, and CJK
  growth force progress without turning ordinary token flow into flicker.
  The structure-aware preview path also recognizes provisional diff fences and
  partial markdown table rows before the final closing fence/table separator
  arrives, avoiding the “raw first, formatted later” jump for common streaming
  markdown.
- **Command deck and control-plane copy** now uses a stable slash-command deck
  ordered around session, configuration, tools, workflow, editing, and sharing
  tasks. Every requested command remains visible and enabled in the deck
  so the UI no longer jumps as workflow surfaces mature, while copy avoids
  dead-end or placeholder language.
- **Startup presence** uses the requested centered LOONG wordmark, keeps the
  compact mark for narrow widths, and animates the two wide-screen O letters
  with a fixed-geometry 60-frame eye overlay through the same redraw cadence as
  rotating tips. Operators that set `LOONG_TUI_REDUCED_MOTION=1` or run under a
  `TERM=dumb` surface get a steady logo/tip/spinner frame instead. The startup
  version row is intentionally product-only (`vX.Y.Z`) and does not echo branch,
  worktree, or local checkout names.
- **Structured preview consistency** keeps diff, markdown, table, image, and
  code-like content readable in both the pending area and the settled
  transcript. Live preview uses lighter structure-aware rendering, while the
  transcript keeps the richer final layout with width-aware markdown table
  fallback, wrapped table cells, diff file/hunk headers, and bounded image
  source/action metadata.
- **Skills inventory** counts and offers both workspace skills and configured
  user-level skill directories through one deduped command-palette surface. The
  startup screen still keeps this compact: it only shows the skill count.
- **Calmer tool activity and error groups** collapse duplicate tool/subagent
  burst noise into stable called/request/file/metrics/stdout/stderr groups, and
  provider failures render as short summaries with bounded detail rows so
  runtime-heavy turns stay readable without hiding unique work.
- **Control-plane surfaces** remain reachable from both the command palette and
  the startup surface: `/sessions`, `/workers`, `/review`, and `/mission`.

## Files to review when changing this surface

- `crates/app/src/chat/chat_surface/app.rs` — overall layout, pending-turn panel,
  startup sections, focus handling.
- `crates/app/src/chat/chat_surface/composer.rs` — input editing, cursor
  positioning, wrap math.
- `crates/app/src/chat/chat_surface/command_palette.rs` — slash-command deck UI.
- `crates/app/src/chat/chat_surface/diff_viewer.rs` — width-aware diff block
  rendering shared by transcript surfaces.
- `crates/app/src/chat/chat_surface/i18n.rs` — user-facing copy routing for the
  command deck/startup helper text.
- `crates/app/src/chat/chat_surface/markdown.rs` — markdown/image/table parsing,
  grid rendering, and narrow-width fallback.
- `crates/app/src/chat/chat_surface/message_list.rs` — transcript rendering,
  burst compaction, tool-activity grouping, viewport caching, startup
  animation, image cards, and resize anchor protections.
- `crates/app/src/chat/chat_surface/utils.rs` — structured preview compaction for
  request/args summaries plus reduced-motion gates for shared animation bits.
- `crates/app/src/chat/cli_render.rs` — structure-aware assistant-section
  parsing that promotes diff/tool-activity sections into typed transcript
  blocks.
- `crates/app/src/chat/live_runtime.rs` — the live preview/tool-activity data
  that feeds the pending-turn surface, including structured-preview cadence.
- `docs/references/chat-surface-resize-render-audit.md` — resize flicker audit,
  current protections, and follow-up rendering recommendations.

## Concrete findings on this branch

- `crates/app/src/chat/live_runtime.rs` now makes pending streaming updates feel
  steadier by separating smooth emission from catch-up emission. The current
  branch explicitly forces progress for long unstable suffix bursts, visual-line
  growth, newline boundaries, width rerenders, provisional diff fences, and
  partial markdown tables instead of waiting on a naive fixed token stride.
- `crates/app/src/chat/chat_surface/message_list.rs` now keeps structured
  transcript output calmer under tool-heavy turns by deduping repeated
  request/args/status/stdout/stderr bursts while still preserving unique child
  details per called group. The startup renderer also owns the wide/compact
  LOONG wordmark selection and fixed-geometry eye/tip animation signatures so
  resize keeps the logo centered without uncontrolled redraw churn. The message
  list also caches the visible viewport slice, not just the full wrapped
  transcript, so unchanged resize/scroll frames can reuse already-materialized
  visible lines.
- `crates/app/src/chat/chat_surface/markdown.rs`,
  `crates/app/src/chat/chat_surface/diff_viewer.rs`, and
  `crates/app/src/chat/cli_render.rs` now keep markdown tables, diff fences,
  image blocks, and tool-activity sections closer to each other across pending
  preview and settled transcript rendering, with narrow-width table fallback,
  wrapped wide table cells, diff file/hunk headers, and bounded image card
  actions instead of raw markdown noise.
- `crates/app/src/chat/chat_surface/app.rs` keeps the queue, restore,
  provider-error, and footer surfaces visible during resize pressure, and the
  command palette exposes the full requested slash-command deck with stable
  enabled entries and no placeholder copy. Footer builders now prefer compact
  `queued ×n`, `restore ×n`, and model-only forms before resorting to noisy
  truncation on narrow terminals.

## Current verification evidence

The current maturity checkpoint is protected by targeted `loong-app` tests that
cover the operator-facing promises above:

- `live_runtime`
  - `preview_emit_forces_progress_after_large_unstable_burst`
  - `preview_emit_mode_enters_catch_up_when_visual_backlog_grows`
  - `preview_emit_cadence_keeps_catch_up_faster_than_smooth`
  - `compact_observer_rerenders_preview_when_width_changes`
  - `compact_render_structures_provisional_markdown_tables_in_preview`
  - `compact_render_structures_provisional_diff_fence_in_preview`
- `chat_surface/message_list`
  - `assistant_markdown_table_renders_as_structured_grid`
  - `rendered_lines_are_pre_padded_for_stable_cached_redraws`
  - `resize_preserves_top_visible_line_when_scrolled_up`
  - `width_resize_preserves_bottom_anchor_for_wrapped_tail_content`
  - `startup_wordmark_eye_frames_animate_the_two_o_letters`
  - `startup_wordmark_eye_frames_keep_fixed_geometry`
  - `image_block_renders_bounded_source_and_media_actions`
  - `tool_activity_burst_keeps_unique_request_children_per_called_group`
- `chat_surface/app`
  - `pending_footer_yields_to_queue_hint_when_draft_exists`
  - `pending_footer_shows_restore_hint_when_queue_exists`
  - `width_resize_keeps_provider_error_and_footer_visible`
  - `width_resize_keeps_pending_restore_footer_and_previews_visible`
  - `pending_preview_shows_queued_steer_and_follow_up_above_composer`
  - `requested_slash_commands_keep_product_order_and_ready_entries`
  - `provider_error_rendering_bounds_detail_noise`
  - `detect_available_skills_reads_skill_metadata_from_workspace`
  - `queue_footer_truncates_to_available_width`
  - `restore_footer_truncates_to_available_width`
  - `renders_file_and_hunk_headers_without_treating_markers_as_edits`

Run:

- `cargo test -p loong-app live_runtime -- --nocapture`
- `cargo test -p loong-app chat_surface -- --nocapture`
- `cargo check -p loong-app --tests`
- `cargo clippy -p loong-app --tests -- -D warnings`
