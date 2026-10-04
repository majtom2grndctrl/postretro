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

## Perf-floor measurement

Gates the default (index.md Decisions). Setup: 2026-10-03, branch `spike/sh-compose-half-rate-on-main` (fc6c515ac: the spike commit cherry-picked onto main d82e590cb), release, campaign-test, GTX 1660 Super, `POSTRETRO_GPU_TIMING=1` per-pass timing. Full and half ran the same binary; `POSTRETRO_SPIKE_SH_HALF_RATE=1` selects half. One owner reading per mode.

| Pass | Full | Half |
|---|---|---|
| sh_compose (streamed indirect) | 0.40 ms | 0.23 ms |
| animated_direct_sh_compose (Pass B) | 0.56 ms | 0.33 ms |
| forward | 3.23 ms | 3.37 ms |
| Sum of all timed passes | 5.03 ms | 4.86 ms |

- Both compose passes drop by about 42%, in line with the Mac. The combined saving is 0.40 ms.
- The two compose passes are about 19% of timed GPU work at full, against about a third of the frame on the 5300M.
- The 1660's whole timed frame is about 5 ms, far from GPU-bound at 60 Hz, so the 0.40 ms buys no visible frame-time change on the perf floor.
- Forward moved +0.14 ms and the spot-shadow sample count differed (52/120 vs 62/120). That is view or run drift, not the rate, which forward does not read.

### stress-warren-hallway-inspection

2026-10-03, same branch, vsync off, one owner reading per mode. The machine was not recorded.

| Pass | Full | Half |
|---|---|---|
| sh_compose | 0.38 ms | 0.38 ms |
| animated_direct_sh_compose | 0.49 ms | 0.48 ms |
| forward | 2.65 ms | 2.67 ms |

- Half saves nothing here. The hallway's compose cost is not the steady-state animated-delta rows the rate holds back. The likely cause is residency rows, which always compose, plus fixed per-dispatch cost, but the `[SH streaming]` rows-composed line was not captured to confirm it.
- On the Mac, turning off light terms in the developer toolbar did not lower frame time at the owner's test pose, which was CPU-bound. That does not hold map-wide: the large arena is GPU-bound, with compose its top GPU cost (§Mac compose cost).

## Mac compose cost

Measured 2026-10-04 during `shadow-fill-cost`. The protocol, run records and scripts are in its `measurements/shadow-fill-cost/` (on that PR's branch until it merges).

**Setup.**
- Radeon Pro 5300M, release builds from `shadow-fill-cost` 6c7f0a1aa (CPU shadow world reach).
- Exclusive window, render resolution `half`, shadow and fog quality `low`, vsync on.
- Frame and GPU wait: medians of three clean runs per pose (the arena: one run).
- Pass times: one 4 s Metal System Trace per pose. Labelled-pass time is divided by the frame count, taken as the largest count among once-per-frame passes.

| Pose | Frame | GPU wait | Pass B | Indirect | Direct SH promotion | Animated LM | Textured Pass |
|---|---|---|---|---|---|---|---|
| hallway, large arena west end (`--start-pose=21.13,2.44,30.48,0,0`) | 28.7 | 20.8 | 7.63 | 6.73 | — | — | 0.86 |
| hallway, on the lift (`0,3.05,105.664,0,0`) | 26.2 | 17.9 | 0.81 | 1.02 | 0.21 | — | 4.02 |
| kinematic-platform, spawn | 32.0 | 21.4 | 10.82 | 9.61 | 4.74 | 1.56 | 4.48 |
| kinematic-platform, promotion station (`-6.5,1.22,-27.94,0,0`) | 34.0 | 26.2 | 10.99 | 9.77 | 3.38 | 1.58 | 5.93 |
| campaign-test, spawn | 17.0 (vsync) | 9.8 | 3.56 | 3.11 | — | 0.32 | 3.03 |

All values are ms per frame. "Pass B" is Streamed Animated Direct SH and "Indirect" is Streamed SH Compose.

- **The revive condition is met.** The arena and kinematic-platform are GPU-bound on the Mac, and compose is their top GPU cost:
  - arena: 14.4 ms of 28.5 ms labelled GPU time;
  - kinematic-platform: 20–25 ms of 37–39 ms.
- **Cost follows pose.** On one map, compose costs 1.8 ms at the lift and 14.4 ms in the arena 25 m away. Sampled rows scale with the visible probe volume.
- **The CPU side is small in the arena:** planning 0.22 ms, compose recording 0.07 ms.
- **Kinematic-platform is a CPU exception, from animated lightmap compose, not SH.** An owner dev-tools run showed `rec_animated_lm` at 4.06 ms per frame. It was 3.84 ms with every light term but the ambient floor off.
  - The map's newest animated light is the gable's `style 2` pulse spot, with a 900-unit range across the 87.9 m cut wall.
  - Animated lightmap memory is 96 MiB there, against 24 MiB on the hallway.
- **Light terms gate part of the GPU cost.** Owner test on kinematic-platform with every term but the ambient floor off: GPU wait 15.1 → 10.4 ms, frame 30.3 → 25.3 ms.
- **The shadow change did not move compose.** Kinematic-platform compose before and after `shadow-fill-cost` agree within 0.2 ms per pass.
- **Unmeasured: rows composed per pass at these poses.** The `[SH streaming]` rows-composed line was not captured, as on 10-03. It decides the lever:
  - residency rows always compose;
  - steady animated-delta rows are what rate limiting or per-light scoping can skip.

  The 1660 hallway result above (`half` saved nothing) hints that residency rows dominate there.
- **The §10 premise has changed.** Since 0f988c7bf, `rendering_pipeline.md` §10 names the 5300M class a perf-tuned Mac target, not a must-run floor. The Default decision in `index.md` still cites the older wording. A revival should rule on it again.

## Ordering pins

From `/review-brief` (rows lens), 2026-10-03. Acceptance rows cite these by id.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Bridge writes a compose descriptor; the same frame's uniform update uploads the mirror; the frame then plans, recorded or skipped | bridge write → descriptor upload (clears dirty) → acquire → plan (records_compose false on a failed acquire) | The step belongs to the frame whose upload carried the change. A write made after that frame's upload is the next frame's step, never this one's. A skipped frame keeps the latch. |
| P2 | Animated promotion weights change on a frame that records no compose; the next frame records with steady weights | weight change (skipped frame) → recorded frame | The Pass B weight-change exemption carries to the next recorded frame, as a step does: every stale gated Pass B row composes there. Today `snapshot_changed` consumes the change before the `records_compose` early return. |
| P3 | A finite-playCount curve (the a11y strobe light pads) plays at half | each new cycle repacks the compose descriptor | Every period starts with a step frame at full rate, so the strobe A/B sees splits only between cycle starts. A playCount-null chase (campaign-test arena sweeps) has no such step and is the clean fast-curve case. |
| P4 | No previous-frame snapshot exists: first recorded frame after full→half, after a level install, after a session clear | snapshot missing → plan | A missing snapshot counts as "behind": a gated row that re-enters the gate on that frame composes, whatever its phase. |
| P5 | At half, a pass's only stale gated rows are all off-phase, so its plan is empty; or the frame records no compose | plan (empty) → dispatch skipped | The gauge is set at plan time and reports the held-back rows even when nothing dispatches. On a frame that records no compose it reads zero, because the rate held nothing back. |
| P6 | Rate set to half, then a level change installs new geometry | set rate → level install (streaming state and planner rebuilt) → first recorded frame | The rate survives the level change. Phase, latched step and previous-frame snapshot start fresh with the new planner. |

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
