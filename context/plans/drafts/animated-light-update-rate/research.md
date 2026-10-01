# animated-light-update-rate — research

Derivation behind `index.md`. Read at 832e20c8a. Source under `crates/` is identical to 97cf912ec, where the draft session read it.

## Spike

Branch `spike/sh-compose-half-rate`, commit 0c2b2ac1f, throwaway. With `POSTRETRO_SPIKE_SH_HALF_RATE=1`, the gated branch of `PassStaleness::plan_into` keeps a gated row only when `row % 2 == frame % 2`. A process-global `AtomicU32` holds the frame, and `StreamedComposePlanner::plan_frame_into` advances it after the `records_compose` early return. Pending (residency) rows are added before the filter, and the full-repair / force-full branch is untouched.

Setup: 2026-10-01, release, Auto, 1280×720 logical, Metal System Trace, 30 s recordings, ~15.5 s analysis window, campaign-test, Radeon Pro 5300M.

| Metric | Full | Half |
|---|---|---|
| Rows per pass per frame | 730 | 360 / 370 alternating |
| Streamed SH Compose (indirect) | 2.489 ms | 1.279 ms |
| Streamed Animated Direct SH (Pass B) | 2.885 ms | 1.472 ms |
| GPU busy (union) | 16.12 ms | 13.50 ms |
| Compute queue | 8.12 ms | 5.53 ms |

- Linear in rows: ~0.0033 ms/row indirect, ~0.0039 ms/row animated direct. Fixed cost ~0.06–0.07 ms per pass.
- Frame time stays vsync-capped either way. The gain is GPU headroom.
- Owner visual A/B on campaign-test: no flicker, seams, stepping or brightness pops.
- Re-recorded runs: one CPU pair was discarded because the screen locked (§12: no frames render while locked), and another for contention. Frame-regime differences did not reproduce across run orders.

## Why the stored value survives a skipped frame

- A row the plan omits is never committed, so `is_stale` stays true and the row is planned again on a later frame.
- The composed atlases are written in place, with no clear and no ping-pong. An omitted row's slots keep the last composed value.
- Cluster promotion keys on per-pass compose epochs (`required_compose_epochs`), and those advance on any successful dispatch. Install rows are pending and never deferred, so deferral cannot block promotion.

## Re-entry hazard

The spike's plain parity filter applies to every stale gated row, including one that lagged out of view. Suppose a row leaves the gate at time t0 and re-enters at t1 on the wrong phase. For one frame it shows its t0 value next to neighbours composed at t1, which reads as a 4 m brick pattern (4×4×4 probes at 1 m spacing). The spike's A/B ran with continuously animated lights in view and would not reliably show it. The bounded-lag rule in `index.md` removes the hazard. It costs one extra compose for a row that enters the gate off-phase.

Shape options for the bound:

| Shape | Memory | Note |
|---|---|---|
| Ring of the last n per-pass epoch snapshots, one per recorded frame | O(n) per pass | A row defers only when it is current against the snapshot from n − 1 recorded frames back and has no `CARRIED` lag. |
| Per-row last-composed recorded-frame stamp | +4 B per row per pass | Simpler test, but it grows §4's documented per-row bound (~35 B). |

A rule that only checks "current before this frame's fire" is exact for n = 2. For n > 2 it collapses the rate to 2, which rules it out for code meant to stay general.

## Exemption analysis

| Work | Deferred at half? | Why |
|---|---|---|
| Pending rows: install, slot reuse, partial eviction, writer closure | No | They are added before the split. A slot must never promote over a previous tenant's texels (`done/perf-sh-compose-sampled-row-gating`). |
| Full repair (light-term mask or dev-override change) | No | The repair branch ignores the gate. It changes only under dev-tools. |
| Force-full-resident | No | It is the exactness reference. |
| Static direct, Pass A | No | It fires only when uploaded promotion weights change. Splitting it would pair a stale baked subtraction with the current runtime `w` term, a transient double count. It is not part of the measured cost (`direct_sh_compose` in §12). |
| Pass B on a frame where static or animated promotion weights change | No | Same crossfade argument: `(1 − w) ×` the section-45 delta against `w ×` the runtime term. The planner folds these flags into `animated_changed` today, so they must be split out. |
| Pass B rows that Pass A rewrote | No | Pass A marks them pending in Pass B. |
| Indirect, steady animation | Yes | No promotion term. |
| Pass B, steady animation | Yes | `w` is steady at 0 or 1 outside a crossfade, so neither arm moves against a stale counterpart. |

## Diagnostics semantics

- `ComposePassPlan::lagged_rows` counts planned rows that are stale at plan time. Every planned gated row is stale by construction, so the counter means "composed because stale", not "composed because it missed earlier frames". Deferral leaves that meaning intact.
- `lagging_count` (resident minus current, maintained incrementally) is read after commit by `pass_diagnostics` as `resident_rows_still_lagging`. Deferred rows stay stale, so they count there. The spike left both counters untouched, and the deferred rows showed up only in still-lagging.
- Surfaces: `ShComposePassDiagnostics` feeds the `[SH streaming]` log (`session/sh_streaming_diagnostics.rs`), the Streaming tab (`debug_ui/streaming_tab.rs`), and the capture report's own JSON struct (`capture/report.rs`). The capture JSON copies fields one by one, so leaving the new gauge out of it is a choice, not an omission.

## Capture

`PreparedCapture::prepare` builds `Renderer::new_offscreen` and sets force-full-resident from the scene, false by default. It reads no `PlayerOptions`. With a renderer default of `full`, ordinary capture and measurement-mode capture (which steps animation time by 1/60 s) compose exactly as today.

## Handoff correction

The handoff said the rate would pass "through `FrameInputs`, like `force_full_resident`". No `FrameInputs` type exists. `force_full_resident_sh_compose` is a full-renderer state field (`renderer_types.rs`). It is set by `Renderer::set_force_full_resident_sh_compose` (dev-tools Streaming tab, capture) and read in `Renderer::prepare_streamed_sh_compose`, which passes it through `ShResidencyState::prepare_compose_frame` into `ComposePlannerFrame`. That setter uses `full_mut()`, so it is only valid after full init.

## Touch points

These follow the `render_resolution` precedent; a grep for it finds the same set.

| Area | Files |
|---|---|
| Option store | `crates/postretro/src/options/graphics.rs` (enum, slot vocabulary, catalog drift guard), `options/mod.rs` (field, key, per-field load, tests) |
| Bridge and live apply | `options/bridge/mod.rs` (slot const, observed generation, `OptionsApplyEffects`), `app/options_menu.rs` |
| Chokepoint and boot | `startup/render_profile.rs` (`renderer_*` mapping, `apply_player_*`), `startup/splash_lifecycle.rs` (`finish_renderer_full_init`) |
| Catalog and slots | `crates/entities/src/engine_state_catalog.rs` (entry plus sorted-list tests), `crates/entities/src/slot_table.rs` tests |
| SDK | `sdk/types/postretro.d.ts`, `.d.luau` via `cargo run -p postretro-sim --bin gen-script-types`; `crates/sim/src/scripting/typedef/tests/fixtures/expected.d.ts`, `expected.d.luau` |
| Content | `content/dev/scripts/frontend-menu.ts`: reactions, graphics row |
| Renderer | setter and state, `prepare_streamed_sh_compose`, `ComposePlannerFrame`, `PassStaleness`, `ShComposePassDiagnostics`, Streaming tab, log line |

## Related

- `drafts/animated-promotion-direct-sh-pop.md`: a known pop where a promoted animated light brightens as `w` rises. It predates this work. Judge visual A/B pops against `full`, not in isolation.
