# Chat Surface Resize / Render Audit

This note captures the current resize and render behavior in the Loong chat surface,
why the remaining resize flicker is likely still visible, and which general
terminal-UI reference patterns are worth preserving in follow-up work.

## Scope reviewed

- `crates/app/src/chat/chat_surface/app.rs`
- `crates/app/src/chat/chat_surface/message_list.rs`
- `crates/app/src/chat/live_runtime.rs`
- reference patterns: terminal render-cache invalidation and sticky bottom scroll behavior

## What is already working well locally

### 1. Resize handling already distinguishes width reflow from other redraws

`app.rs` only enables the "quiet window" when the terminal width changes.
`resize_reflow_required(...)` and `redraw_throttle_ready(...)` keep the surface
from redrawing as aggressively on pure height changes, and the main event loop
already caps rapid width redraws to roughly one frame every 16 ms with a short
70 ms quiet window.

### 2. Transcript rendering already has cache + anchor protections

`message_list.rs` keeps a width/revision render cache, now memoizes the visible
viewport slice for the current width/height/scroll/top-padding tuple, and
includes explicit resize tests for both scroll modes:

- `resize_preserves_top_visible_line_when_scrolled_up`
- `resize_preserves_bottom_anchor_when_following_tail`
- `width_resize_preserves_bottom_anchor_for_wrapped_tail_content`

That means the transcript is already better behaved than a naive full rerender
list: the main remaining roughness is not anchor loss, but how often we force
fresh layout work while the terminal is still moving.

### 3. Live preview width changes are intentionally covered by tests

`live_runtime.rs` exposes `build_cli_chat_live_compact_observer_controller(...)`
so the pending-turn preview can rerender when the width changes, and
`compact_observer_rerenders_preview_when_width_changes` locks that behavior in.

### 3b. Live preview cadence is now pressure-aware instead of purely token-count based

The newer `live_runtime.rs` path does more than simply emit every N characters.
`should_emit_cli_chat_live_preview(...)` now promotes three useful operator
behaviors that match the current maturity checkpoint better than this note's
older framing:

- large unstable suffix bursts force progress
- wrapped visual-line backlog can enter a faster catch-up mode
- newline / structural-token boundaries commit preview updates immediately

That behavior is protected by tests such as:

- `preview_emit_forces_progress_after_large_unstable_burst`
- `preview_emit_mode_enters_catch_up_when_visual_backlog_grows`
- `preview_emit_cadence_keeps_catch_up_faster_than_smooth`
- `compact_observer_commits_preview_immediately_on_newline_boundary`

### 3c. Structured preview and transcript rendering are now closer than this audit originally assumed

`live_runtime.rs`, `cli_render.rs`, `markdown.rs`, and `message_list.rs` now
share a clearer contract for structured content:

- diff fences render as diff-oriented preview/transcript blocks
- provisional diff fences render as diff-oriented previews before the closing
  fence arrives
- markdown tables render as grids, wrap wide cells, and fall back to stacked
  rows at narrow widths
- partial table rows render through a stable lightweight table preview instead
  of flashing raw pipe text first
- image blocks render as bounded media cards with source/action rows rather
  than dead-end preview copy
- tool activity is compacted into calmer grouped children instead of replaying
  every repeated request/detail/status line verbatim

The preview path is still intentionally lighter than the final transcript, but
that is now an explicit design choice rather than just an implementation gap.

### 4. Queue / restore / footer UX is already protected

`app.rs` keeps queue and restore hints in dedicated footer builders, and the
surface tests already protect the requirement to preserve queue/error/footer UX:

- `pending_footer_yields_to_queue_hint_when_draft_exists`
- `pending_footer_shows_restore_hint_when_queue_exists`
- `pending_preview_shows_queued_steer_and_follow_up_above_composer`
- `queue_footer_truncates_to_available_width`
- `restore_footer_truncates_to_available_width`

The narrow variants intentionally prefer compact semantic forms such as
`queued ×n`, `restore ×n`, model-only status, and short follow hints before
falling back to truncation.

## Why resize flicker can still happen

### 1. Width resize still has two render concerns, but live rerender is now coalesced

During `Event::Resize`, `app.rs` still marks the app dirty and records that the
pending live preview needs a width-aware rerender. The current branch no longer
needs to emit that rerender directly from the resize event; it coalesces the
rerender behind the redraw throttle so the pending preview and main frame settle
through the same draw path.

The remaining roughness is therefore narrower than the earlier audit: the app
still redraws a composed frame and still recomputes layout when the quiet window
opens, but the worst immediate double-settle path is guarded.

### 2. The pending area now has a geometry/signature cache, but width changes still reflow content

`pending_lines_for(...)` keeps a `PendingRenderCache` keyed by pending content,
width, height, and preview budget. That prevents same-geometry redraw churn.
Real width changes still correctly rebuild `pending_live_lines(...)`,
`build_pending_lines(...)`, and `compact_pending_lines_for_height(...)` for the
new width/height budget, which means rapid width changes can still reflow:

- blank-line normalization
- reasoning/visible split detection
- wrapped preview lines
- queue / steer preview lines
- pending-height compaction

The result is functionally stable, but not yet visually "quiet".

### 3. The transcript now has a cached viewport slice, but Ratatui still paints a frame

The new viewport cache avoids rebuilding the visible transcript slice when the
width, height, scroll start, top padding, and render revision are unchanged. It
is deliberately conservative: any content mutation, width change, scroll move,
or startup-animation signature change invalidates the relevant cache.

That moves the surface closer to dirty-region behavior without introducing a
custom renderer. The terminal still receives a composed Ratatui frame, so this
is a partial dirty-region step rather than a full row-diff renderer.

### 4. The surface redraw is still frame-wide, not dirty-region based

The current Ratatui flow redraws the composed frame. That is simpler and safe,
but it still leaves room for a narrower refresh model that only invalidates
visible dirty regions when the changed area is known.

## Reference patterns worth borrowing

### Cache rendered lines and refresh only the dirty viewport region

The useful reference pattern has three parts:

- keep an LRU-style render cache for terminal lines
- clear the cache on resize/theme changes
- refresh only affected visible regions when state deltas are known

A future `_update_from_state(...)`-style path could refresh targeted line regions
instead of blindly repainting the entire viewport every time new terminal data
lands. That is the clearest model for reducing visible churn once the chat
surface changes move beyond documentation into rendering work.

### Keep bottom-following scroll behavior explicit and separate from width logic

The useful reference pattern is to keep session scrolling bottom-anchored while
width/space calculations stay in a separate memoized path. Scroll feel should
route through a dedicated helper so acceleration remains configurable without
entangling layout code.

That separation is useful for the chat surface too: resize smoothness work should
stay separate from scroll policy and footer/composer behavior.

## Recommended follow-up order

1. **Keep coalesced pending preview rerenders during active resize**
   - Preserve the current width-change correctness.
   - Keep live preview rerender behind the redraw throttle instead of emitting
     directly from every resize event.
   - Prefer a single width-aware redraw path per resize tick.
2. **Keep the transcript viewport cache conservative**
   - Preserve invalidation on content mutation, width change, scroll movement,
     and startup animation signature changes.
   - Do not let viewport reuse bypass scroll-boundary snapping tests.
3. **Keep extending the pending width/signature cache**
   - Preserve the existing `PendingRenderCache` for same-geometry redraws.
   - Consider caching normalized preview segments separately if future profiling
     shows wrapping work is still the bottleneck.
4. **Explore narrower invalidation for transcript + pending regions**
   - Start with pending preview invalidation, not a whole-surface rewrite.
   - Preserve current queue, restore, provider-error, and footer behavior.

## Guardrails for any implementation follow-up

- Preserve current queue/restore footer wording and placement.
- Preserve transcript anchor behavior when following tail and when scrolled up.
- Do not regress provider-error or tool-activity rendering in the pending area.
- Treat dirty-region refresh as the quality bar, but not as a reason to rewrite
  the entire chat surface architecture in one pass.

## Verification evidence for the current checkpoint

The review above is grounded in the current `loong-app` test coverage for this
branch:

- `cargo test -p loong-app live_runtime -- --nocapture`
- `cargo test -p loong-app chat_surface -- --nocapture`

Key assertions worth keeping in mind when touching resize/render behavior:

- `assistant_markdown_table_renders_as_structured_grid`
- `renders_markdown_tables_as_stacked_rows_when_width_is_tight`
- `tool_activity_burst_keeps_unique_request_children_per_called_group`
- `rendered_lines_are_pre_padded_for_stable_cached_redraws`
- `resize_preserves_top_visible_line_when_scrolled_up`
- `width_resize_preserves_bottom_anchor_for_wrapped_tail_content`
- `pending_footer_yields_to_queue_hint_when_draft_exists`
- `width_resize_keeps_provider_error_and_footer_visible`
- `compact_observer_rerenders_preview_when_width_changes`
- `compact_observer_skips_rerender_when_width_change_keeps_same_lines`
