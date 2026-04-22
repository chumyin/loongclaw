# Pi Surface TUI Reference

This note documents the current operator-facing expectations for
`crates/app/src/chat/pi_surface/`.

## UX commitments

- **Pi Mono-aligned composer** keeps the borderless prompt layout and now keeps
  cursor placement aligned with wrapped content width instead of re-applying the
  first-line prompt offset after every wrap.
- **Pending-turn streaming** shows the live draft/tool-activity preview inline
  while a turn is still running, so “show thinking” style feedback is visible
  before the final assistant message lands.
- **Command deck and control-plane copy** is routed through
  `pi_surface/i18n.rs`, which centralizes slash-command descriptions and keeps
  future locale-specific routing isolated from the widget code.
- **Control-plane surfaces** remain reachable from both the command palette and
  the startup surface: `/sessions`, `/workers`, `/review`, and `/mission`.

## Files to review when changing this surface

- `crates/app/src/chat/pi_surface/app.rs` — overall layout, pending-turn panel,
  startup sections, focus handling.
- `crates/app/src/chat/pi_surface/composer.rs` — input editing, cursor
  positioning, wrap math.
- `crates/app/src/chat/pi_surface/command_palette.rs` — slash-command deck UI.
- `crates/app/src/chat/pi_surface/i18n.rs` — user-facing copy routing for the
  command deck/startup helper text.
- `crates/app/src/chat/pi_surface/message_list.rs` — transcript rendering and
  compaction/tool activity blocks.
- `crates/app/src/chat/live_runtime.rs` — the live preview/tool-activity data
  that feeds the pending-turn surface.
- `docs/references/pi-surface-resize-render-audit.md` — resize flicker audit,
  current protections, and follow-up rendering recommendations.
