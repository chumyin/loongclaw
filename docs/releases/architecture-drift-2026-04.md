# Architecture Drift Report 2026-04

## Summary
- Generated at: 2026-04-13T16:57:01Z
- Report month: `2026-04`
- Baseline report: none
- Hotspots tracked: 14
- Boundary checks tracked: 5
- SLO status: PASS

## Hotspot Metrics

| Key | Classes | File | Lines | Max Lines | Line Headroom | Functions | Max Functions | Fn Headroom | Peak Usage | Pressure |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| spec_runtime | `foundation` | `crates/spec/src/spec_runtime.rs` | 3526 | 3600 | 74 | 65 | 65 | 0 | 100.0% | TIGHT |
| spec_execution | `foundation` | `crates/spec/src/spec_execution.rs` | 3571 | 3700 | 129 | 48 | 80 | 32 | 96.5% | TIGHT |
| provider_mod | `foundation` | `crates/app/src/provider/mod.rs` | 406 | 1000 | 594 | 11 | 20 | 9 | 55.0% | HEALTHY |
| memory_mod | `foundation` | `crates/app/src/memory/mod.rs` | 456 | 650 | 194 | 16 | 16 | 0 | 100.0% | TIGHT |
| acp_manager | `operational_density` | `crates/app/src/acp/manager.rs` | 3442 | 3600 | 158 | 12 | 12 | 0 | 100.0% | TIGHT |
| acpx_runtime | `operational_density` | `crates/app/src/acp/acpx.rs` | 1865 | 2800 | 935 | 56 | 65 | 9 | 86.2% | WATCH |
| channel_registry | `structural_size` | `crates/app/src/channel/registry.rs` | 9438 | 10500 | 1062 | 72 | 90 | 18 | 89.9% | WATCH |
| channel_config | `structural_size` | `crates/app/src/config/channels.rs` | 9718 | 9800 | 82 | 90 | 90 | 0 | 100.0% | TIGHT |
| chat_runtime | `structural_size,operational_density` | `crates/app/src/chat.rs` | 6591 | 7300 | 709 | 95 | 160 | 65 | 90.3% | WATCH |
| channel_mod | `structural_size,operational_density` | `crates/app/src/channel/mod.rs` | 1835 | 6400 | 4565 | 0 | 110 | 110 | 28.7% | HEALTHY |
| turn_coordinator | `structural_size,operational_density` | `crates/app/src/conversation/turn_coordinator.rs` | 8420 | 11200 | 2780 | 36 | 120 | 84 | 75.2% | HEALTHY |
| tools_mod | `structural_size` | `crates/app/src/tools/mod.rs` | 14213 | 15000 | 787 | 45 | 70 | 25 | 94.8% | WATCH |
| daemon_lib | `structural_size` | `crates/daemon/src/lib.rs` | 6334 | 6500 | 166 | 202 | 210 | 8 | 97.4% | TIGHT |
| onboard_cli | `structural_size` | `crates/daemon/src/onboard_cli.rs` | 9771 | 9800 | 29 | 237 | 250 | 13 | 99.7% | TIGHT |

## Prioritization Signals
- BREACH hotspots (>100% of any tracked budget): none
- TIGHT hotspots (>=95% of any tracked budget): spec_runtime (100.0%), spec_execution (96.5%), memory_mod (100.0%), acp_manager (100.0%), channel_config (100.0%), daemon_lib (97.4%), onboard_cli (99.7%)
- WATCH hotspots (>=85% and <95% of any tracked budget): acpx_runtime (86.2%), channel_registry (89.9%), chat_runtime (90.3%), tools_mod (94.8%)
- Mixed-class hotspots (size plus operational density): chat_runtime, channel_mod, turn_coordinator

## Boundary Checks

| Check | Status | Previous Status | Detail |
|---|---|---|---|
| memory_literals | PASS | n/a | memory operation literals are centralized in crates/app/src/memory/* |
| provider_mod_helper_definitions | PASS | n/a | provider/mod.rs keeps payload, parse, and recovery helper implementations outside the top-level module |
| conversation_provider_optional_binding_roundtrip | PASS | n/a | conversation/runtime.rs translates explicit conversation bindings into provider bindings without optional-kernel roundtrips |
| conversation_app_dispatcher_optional_kernel_context | PASS | n/a | conversation app-tool dispatcher approval hooks stay binding-based without optional kernel fallbacks |
| spec_app_dependency | PASS | n/a | spec crate remains detached from app crate at the Cargo dependency boundary |

## SLO Assessment
- Hotspot growth SLO (>10% month-over-month): PASS
- Boundary ownership SLO (helpers stay behind their module boundaries): PASS
- Overall architecture SLO status: PASS

## Refactor Budget Policy
- Monthly drift report command: `scripts/generate_architecture_drift_report.sh`
- Release checklist budget field lives in `docs/releases/support/TEMPLATE.md`.
- Rule: each release must name at least one hotspot metric paid down or explicitly state why no paydown happened.

## Detail Links
- [Architecture gate](../../scripts/check_architecture_boundaries.sh)
- [Release template](support/TEMPLATE.md)
- [CI workflow](../../.github/workflows/ci.yml)

<!-- arch-hotspot key=spec_runtime lines=3526 functions=65 -->
<!-- arch-hotspot key=spec_execution lines=3571 functions=48 -->
<!-- arch-hotspot key=provider_mod lines=406 functions=11 -->
<!-- arch-hotspot key=memory_mod lines=456 functions=16 -->
<!-- arch-hotspot key=acp_manager lines=3442 functions=12 -->
<!-- arch-hotspot key=acpx_runtime lines=1865 functions=56 -->
<!-- arch-hotspot key=channel_registry lines=9438 functions=72 -->
<!-- arch-hotspot key=channel_config lines=9718 functions=90 -->
<!-- arch-hotspot key=chat_runtime lines=6591 functions=95 -->
<!-- arch-hotspot key=channel_mod lines=1835 functions=0 -->
<!-- arch-hotspot key=turn_coordinator lines=8420 functions=36 -->
<!-- arch-hotspot key=tools_mod lines=14213 functions=45 -->
<!-- arch-hotspot key=daemon_lib lines=6334 functions=202 -->
<!-- arch-hotspot key=onboard_cli lines=9771 functions=237 -->
<!-- arch-boundary key=memory_literals status=PASS -->
<!-- arch-boundary key=provider_mod_helper_definitions status=PASS -->
<!-- arch-boundary key=conversation_provider_optional_binding_roundtrip status=PASS -->
<!-- arch-boundary key=conversation_app_dispatcher_optional_kernel_context status=PASS -->
<!-- arch-boundary key=spec_app_dependency status=PASS -->
