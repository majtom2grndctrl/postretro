# animated-light-update-rate

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Sampled-row compose, Promoted static lights, Promoted animated lights), §7.1 step 5, §7.8 (Frame capture), §12 · `context/lib/player_options.md` §2, §4 · `context/lib/ui.md` §5 · read at 832e20c8a · evidence: `research.md`

## Problem
A developer spike measured the cost and found a lever. Every recorded frame, the streamed SH planner recomposes every gated row whose animated-light inputs changed, so compose cost scales with gated rows times frame rate. Indirect light, and the entity direct term, from slow baked animated lights change little between frames. On the compatibility-floor Mac (Radeon Pro 5300M, Metal), the indirect and animated-direct passes cost 5.4 ms of a GPU-bound ~16 ms frame on campaign-test at Auto. Render scale does not shrink this cost, and it is the largest such cost on that frame. Composing each gated row every second frame halved both passes, and the owner saw no artifact (`research.md` §Spike). When this is done, players choose how often animated SH light is recomposed: `full` is today's behaviour, and `half` recomposes each gated row every second frame. The option ships defaulting to `half`, applies live, and capture stays exact.

## Decisions
- **Player option `animated_light_update`: `full` or `half`, default `half`.** It is a `PlayerOptions` graphics field like `render_resolution`. It applies live, persists, and an unknown value falls back to `half` for that field alone (`player_options.md` §2, §4). The default follows `surface_depth_quality`: the feature ships on and `full` is the escape hatch. No `auto`: nothing in the engine detects adapter tier.
- **Exactness becomes rate-relative. This diverges from a commitment.** §4 Sampled-row compose promises that every sampled slot equals frame N's full-resident compose, and `done/perf-sh-compose-sampled-row-gating` made rate-limiting a non-goal on that ground. New rule: at update divisor n, a sampled slot equals full-resident compose from at most n − 1 recorded compose frames earlier; `full` (n = 1) is the old rule. The lag is accepted because the content is low-frequency and the owner's A/B showed no artifact. Undo is the `full` setting.
- **Round-robin by row.** A stale gated row composes when its row id matches the frame's phase modulo n. The phase is planner-owned and advances once per frame that records compose. The code takes any n ≥ 1; only 1 and 2 ship.
- **Bounded lag overrides phase (owner decision).** A gated row more than n − 1 recorded frames behind composes now, whatever its phase: re-entry into the gate, a camera cut. A row n − 1 or fewer recorded frames behind waits for its phase. Otherwise a row coming into view would show a value from when it last left view, which breaks §4's rule that lagging rows compose on re-entry before any consumer samples them (`research.md` §Re-entry hazard).
- **Only steady-state animated-delta compose defers.** Always composed at full rate: residency rows (install, slot reuse, partial eviction, and their writer rows); control-change repair; force-full-resident; static direct (Pass A); animated direct (Pass B) on a frame whose static or animated promotion weights change, and every row Pass A rewrote. Why: a deferred row would pair last frame's `(1 − w)` baked arm with this frame's `w` runtime arm, and "no receiver sums the light twice" (§4 Promoted static lights, Promoted animated lights; `research.md` §Exemption analysis).
- **Scope: the streamed indirect pass and Pass B.** The legacy whole-load path, billboard scatter compose, and animated lightmap compose stay at full rate. The lightmap compose is direct surface light — visible, high-frequency, and outside this measurement.
- **Placement: policy in player options, mechanism in the renderer.** `startup/render_profile.rs` maps the option to an update divisor through an exhaustive match; the renderer never names the option. A renderer never given a rate composes at `full`, the exactness reference. The bridge applies the rate live and full init re-applies it. Capture builds its own renderer and applies no player options, so it stays at `full`; force-full-resident still bypasses the rate.
- **Diagnostics keep their meaning and gain one gauge.** A deferred row is stale, so it counts in "resident rows still lagging", and in "rows composed because they lagged" on the frame it composes. Each pass adds "rows deferred by update rate" to the `[SH streaming]` log and the Streaming tab; still-lagging minus deferred is the view-gate lag. It is zero at `full` and under force-full-resident. The capture report is unchanged.
- **Not in this brief:** adapter detection or `auto`; per-light change scoping (the follow-up `done/perf-sh-compose-sampled-row-gating` named); per-row shader cost; residency or install compose changes; bake or format changes.
- **At promotion:** `rendering_pipeline.md` §4 Sampled-row compose and §7.1 step 5 record the rate-relative invariant, exemptions and gauge; `player_options.md` §4 and the `ui.md` §5 slot list gain the option.

### Scripting surface
```ts
import { defineReaction } from "postretro";
import { getGameState, stateEquals, updateState } from "postretro/ui";

const options = getGameState().options;
// options.animatedLightUpdate: Ref<"full" | "half">; writable, menu-scoped, unreplicated.
defineReaction(
  "frontend.options.animatedLightUpdate.full",
  updateState(options.animatedLightUpdate, "full"),
);
defineReaction(
  "frontend.options.animatedLightUpdate.half",
  updateState(options.animatedLightUpdate, "half"),
);
const halfRate = stateEquals(options.animatedLightUpdate, "half");
```
The Luau mirror ships through the same typedef generator. The persisted form is `animated_light_update = "half"` in `settings.toml`.

## Acceptance
### Automated
Planner, GPU-free:
- [ ] At half, a row that stays gated while its light stays active composes on exactly every second recorded frame. Over any two consecutive recorded frames, every such row composes at least once. Covers an odd gated count and a single gated row.
- [ ] At full, plans match today's: the existing planner tests and oracle equivalence tests pass with the rate set to full and no other change.
- [ ] A gated row more than n − 1 recorded frames behind composes that frame, whatever its phase. A gated row n − 1 or fewer recorded frames behind does not compose until its phase. Both directions are pinned at n = 2 and, through the planner, at n = 3.
- [ ] At half, a lagging row that re-enters the gate composes that frame, whatever its phase. After a camera cut into a lagging room, every gated row there composes that frame.
- [ ] At half, none of these is deferred: install, slot-reuse and partial-eviction rows and their writer rows; a control-change repair; force-full-resident; static direct; animated direct on a frame whose promotion weights change; any row static direct rewrote.
- [ ] Switching half to full mid-sequence: every deferred row composes on the next recorded frame. Switching full to half leaves no gated row lagging past the half bound.
- [ ] At half, the last active light deactivates: every gated contributing row reaches its final value within two recorded frames, and then nothing dispatches.
- [ ] A frame that records no compose does not advance the phase. Two planners fed the same frame sequence produce identical plans.
- [ ] Randomized sequences at either rate: regions move, lights toggle, promotion weights change, frames skip compose, clusters install and evict. After each recorded frame, no gated resident row lags by more than n − 1 recorded frames.
- [ ] The deferred gauge equals the gated stale rows the rate held back that frame, and those rows are included in resident rows still lagging. The gauge is zero at full and under force-full-resident.
- [ ] At half, the legacy whole-load compose still covers every affinity row on each frame it fires, and the billboard scatter and animated lightmap compose dispatch as at full.
Option and surface:
- [ ] `animated_light_update` round-trips `full` and `half`. A file without it loads `half`. An unknown value falls back to `half` for that field alone, warns once, and survives in the file until the player writes the field.
- [ ] Every option value maps at the render-profile chokepoint through an exhaustive match: `full` to divisor 1, `half` to divisor 2.
- [ ] The catalog declares `options.animatedLightUpdate` as a writable, unreplicated enum of `full` and `half`, and the slot table holds it. A drift guard checks the catalog values against the slot parser in both directions.
- [ ] A menu write reaches the renderer on the next frame and schedules the settled save. Full init applies the saved value before the first gameplay frame.
- [ ] The regenerated TypeScript and Luau SDK typedefs and the typedef fixtures include the slot. The dev frontend's graphics tab reads and writes it with the Scripting surface names, and its script builds.
Capture:
- [ ] A renderer that was never given a rate composes at full. The capture path never applies the player option, and existing capture exactness and golden tests pass unchanged.
### Manual
- [ ] On this Mac, record a Metal System Trace (`rendering_pipeline.md` §12) of campaign-test: release build, Auto, 1280×720 logical, 30 s, once at full and once at half. Streamed SH Compose and Streamed Animated Direct SH drop roughly in line with rows composed. Spike reference: 2.489 → 1.279 ms and 2.885 → 1.472 ms. Check machine-state confounders first (§12).
- [ ] Visual A/B at half against full on campaign-test, near the crusher and the slow-pulse lights. Look for seams at 4 m brick boundaries, shimmer, stepping, and brightness pops on the viewmodel or characters. There should be none.
- [ ] Turn into a room whose animated lights ran while it was out of view. No one-frame brick pattern appears.
- [ ] Toggle the option live in the options menu mid-session. No artifact persists.

## Path
- **Seams:**
  - The spike (`spike/sh-compose-half-rate`, 0c2b2ac1f) filters in `PassStaleness::plan_into`'s gated branch and advances a global counter in `StreamedComposePlanner::plan_frame_into` after the `records_compose` early return. Replace the global and the env read with planner state and a rate field on `ComposePlannerFrame`, beside `force_full_resident`.
  - `force_full_resident` reaches the planner through `Renderer::prepare_streamed_sh_compose` and `ShResidencyState::prepare_compose_frame`.
  - Its source, `force_full_resident_sh_compose`, lives in full-renderer state, and its setter needs full init. The rate setter must accept a value before full init, as `set_surface_depth_quality` does, or run after `ensure_full_ready` as fog does.
- **Pass B exemption:** `plan_frame_into` folds promotion-weight changes into `animated_changed`. Keep those flags separate so `plan_into` can tell a weight-change frame from an animation frame.
- **Bounded-lag shape:** keep a ring of each pass's last n epoch snapshots, taken per recorded frame. A gated row may defer when it is current against the snapshot from n − 1 recorded frames back and is not carrying lag across a membership change. That costs O(n) per pass and no per-row memory. Rival: a per-row last-composed-frame stamp, which adds 4 B per row per pass to the per-row bound in §4.
- **Oracle:** `compose_plan_oracle.rs` models the old push planner. Keep its equivalence tests at full, and prove half with independent property tests rather than teaching the oracle the split.
- **Option wiring:** follow `render_resolution`'s footprint (`research.md` §Touch points). Typedefs regenerate with `cargo run -p postretro-sim --bin gen-script-types`.
- **First slice:** the planner change, its tests and the deferred gauge, with the rate passed in directly. Then check the gauge on campaign-test, where about half of the gated rows should be deferred. This tests the riskiest assumption: that bounded lag and the exemptions leave the steady-state saving intact. Wire the option after that.
- **File size:** `renderer_types.rs`, `sh_streaming.rs`, `splash_lifecycle.rs` and the `engine_state_catalog.rs` data table are past the split threshold, but each gains only a field, an entry or a line, so no split is owed. The new planner logic belongs in `compose_staleness.rs` or a sibling module. `compose_plan.rs`, `options/mod.rs` and `options/bridge/mod.rs` are large mostly because of their tests.

## Open questions
- Menu label wording and row placement. "Animated lighting: Full / Half rate" overstates the scope, because surface animated lightmaps stay at full rate — **delegated**
