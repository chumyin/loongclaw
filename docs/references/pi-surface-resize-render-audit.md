# Pi Surface Resize / Render Audit

This note captures the current resize and render behavior in the Pi chat surface,
why the remaining resize flicker is likely still visible, and which reference
patterns from `repomix-output-batrachianai-toad.xml` and
`repomix-output-anomalyco-opencode.xml` are worth preserving in follow-up work.

## Scope reviewed

- `crates/app/src/chat/pi_surface/app.rs`
- `crates/app/src/chat/pi_surface/message_list.rs`
- `crates/app/src/chat/live_runtime.rs`
- reference repos: Toad terminal widget and OpenCode TUI session/scroll logic

## What is already working well locally

### 1. Resize handling already distinguishes width reflow from other redraws

`app.rs` only enables the "quiet window" when the terminal width changes.
`resize_reflow_required(...)` and `redraw_throttle_ready(...)` keep the surface
from redrawing as aggressively on pure height changes, and the main event loop
already caps rapid width redraws to roughly one frame every 16 ms with a short
70 ms quiet window.

### 2. Transcript rendering already has cache + anchor protections

`message_list.rs` keeps a width/revision render cache and includes explicit
resize tests for both scroll modes:

- `resize_preserves_top_visible_line_when_scrolled_up`
- `resize_preserves_bottom_anchor_when_following_tail`

That means the transcript is already better behaved than a naive full rerender
list: the main remaining roughness is not anchor loss, but how often we force
fresh layout work while the terminal is still moving.

### 3. Live preview width changes are intentionally covered by tests

`live_runtime.rs` exposes `build_cli_chat_live_compact_observer_controller(...)`
so the pending-turn preview can rerender when the width changes, and
`compact_observer_rerenders_preview_when_width_changes` locks that behavior in.

### 4. Queue / restore / footer UX is already protected

`app.rs` keeps queue and restore hints in dedicated footer builders, and the
surface tests already protect the requirement to preserve queue/error/footer UX:

- `pending_footer_yields_to_queue_hint_when_draft_exists`
- `pending_footer_shows_restore_hint_when_queue_exists`
- `pending_preview_shows_queued_steer_and_follow_up_above_composer`

## Why resize flicker can still happen

### 1. Width resize currently triggers two render paths

During `Event::Resize`, `app.rs` does two things:

1. it marks the full app as dirty for the next `terminal.draw(...)`
2. it also calls `live_rerender()` immediately when the width changed

That second path is useful for keeping the pending preview wrapped correctly,
but it also means the live preview can emit a fresh batch while the main surface
is still inside the resize quiet window. In practice this can show up as a
small flicker or "double settle" effect in the pending region during drag
resize.

### 2. The pending area still recomputes from full normalized text on every width tick

`pending_live_lines(...)`, `build_pending_lines(...)`, and
`compact_pending_lines_for_height(...)` rebuild the pending block from scratch
for the current width/height budget. That is correct, but it means rapid width
changes repeatedly rebuild:

- blank-line normalization
- reasoning/visible split detection
- wrapped preview lines
- queue / steer preview lines
- pending-height compaction

The result is functionally stable, but not yet visually "quiet".

### 3. The surface redraw is still frame-wide, not dirty-region based

The current Ratatui flow redraws the composed frame. That is simpler and safe,
but it differs from the Toad reference, which aggressively narrows refresh work
to visible dirty regions.

## Reference patterns worth borrowing

### Toad: cache rendered lines and refresh only the dirty viewport region

In `src/toad/widgets/terminal.py`, Toad does three things that matter here:

- keeps an LRU terminal render cache
- clears the cache on resize/theme changes
- refreshes only affected visible regions when state deltas are known

Its `_update_from_state(...)` path refreshes targeted line regions instead of
blindly repainting the entire viewport every time new terminal data lands. That
is the clearest model for reducing visible churn once Pi surface changes move
beyond documentation into rendering work.

### OpenCode: keep bottom-following scroll behavior explicit and separate from width logic

In `packages/opencode/src/cli/cmd/tui/routes/session/index.tsx`, OpenCode keeps
session scrolling bottom-anchored with `stickyScroll={true}` and
`stickyStart="bottom"`, while width/space calculations stay in separate memos.
It also routes scroll feel through a dedicated helper
(`packages/opencode/src/cli/cmd/tui/util/scroll.ts`) so scroll acceleration is
configurable without entangling layout code.

That separation is useful for Pi surface too: resize smoothness work should stay
separate from scroll policy and footer/composer behavior.

## Recommended follow-up order

1. **Coalesce pending preview rerenders during active resize**
   - Keep the width-change correctness.
   - Avoid emitting the live preview through `live_rerender()` on every resize
     event when the main frame is already waiting on the quiet window.
   - Prefer a single width-aware redraw path per resize tick.
2. **Add a width/signature cache for the pending block**
   - Similar to `MessageList` render caching.
   - Cache the normalized pending preview when width, height budget, and pending
     signature have not changed.
3. **Explore narrower invalidation for transcript + pending regions**
   - Start with pending preview invalidation, not a whole-surface rewrite.
   - Preserve current queue, restore, provider-error, and footer behavior.

## Guardrails for any implementation follow-up

- Preserve current queue/restore footer wording and placement.
- Preserve transcript anchor behavior when following tail and when scrolled up.
- Do not regress provider-error or tool-activity rendering in the pending area.
- Treat Toad's dirty-region refresh as the quality bar, but not as a reason to
  rewrite the entire Pi surface architecture in one pass.
