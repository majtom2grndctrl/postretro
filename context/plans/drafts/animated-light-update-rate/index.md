# animated-light-update-rate

Brief · compact · reads: `context/lib/rendering_pipeline.md` §4 (Cluster SH residency, Sampled-row compose, Promoted static lights, Promoted animated lights), §7.1 step 5, §7.8 (Frame capture), §10, §12 · `context/lib/player_options.md` §2, §4 · `context/lib/ui.md` §5 · read at 940f5338b · evidence: `research.md`

> **Status: parked (owner, 2026-10-03).** Do not promote. No measured target shows a player-visible win (`research.md` §Perf-floor measurement):
> - On the GTX 1660 Super, `half` saves 0.40 ms of a ~5 ms GPU frame on campaign-test, which is far from GPU-bound.
> - On the Mac, campaign-test stays vsync-capped either way.
> - On stress-warren-hallway-inspection, `half` saves nothing on the 1660, and the owner's Mac test pose was CPU-bound.
>
> Meanwhile the brief adds a player setting, departs from the §4 exactness rule, and needs step and weight-carry machinery to avoid split frames.
>
> **Revive when** a measured GPU-bound target, on a map or hardware tier the owner targets, shows streamed SH compose among its top GPU costs.
>
> **Condition met (2026-10-04)** on the Mac. Compose is the top GPU cost in the hallway's large arena (14.4 of 28.5 ms) and on kinematic-platform (20–25 ms). See `research.md` §Mac compose cost, which also flags the changed §10 premise behind the Default decision. Revival is still the owner's call, and the per-light scoping below comes first.
>
> **Weigh first:** per-light change scoping, the follow-up `done/perf-sh-compose-sampled-row-gating` named. It composes only the rows a changed light touches. It is exact and helps every player, with no setting, step latch or lag rule.
>
> **Carry over on revival:** the validate-plan and review-brief findings already folded into Decisions, Acceptance and §Ordering pins. The owner chose not to adopt these (2026-10-03):
> - counting curve-sample swaps as steps;
> - carrying promotion-weight changes across skipped frames (the P2 row is kept, but no Decision backs it yet);
> - dropping the deferred gauge.
>
> A revival should rule on all three again. The brief is also about twice the compact length; trim it then.

## Problem
A developer spike measured the cost and found a lever. Every recorded frame, the streamed SH planner recomposes every gated row whose animated-light inputs changed, so compose cost scales with gated rows times frame rate. Indirect light, and the entity direct term, from slow baked animated lights change little between frames. On the compatibility-floor Mac (Radeon Pro 5300M, Metal), the indirect and animated-direct passes cost 5.4 ms of a GPU-bound ~16 ms frame on campaign-test at Auto. Render scale does not shrink this cost, and it is the largest such cost on that frame. Composing each gated row every second frame halved both passes, and the owner saw no artifact (`research.md` §Spike). That frame stays vsync-capped either way: the win is GPU headroom, not a visible frame-time drop. When this is done, players choose how often animated SH light is recomposed: `full` is today's behaviour, and `half` recomposes each steady-state gated row every second frame, while sudden light changes still land on the frame they happen. The option defaults to `full` and applies live, `half` is the opt-in for GPU-bound machines such as laptops, and capture stays exact.

## Decisions
- **Player option `animated_light_update`: `full` or `half`, default `full`.** It is a `PlayerOptions` graphics field like `render_resolution`. It applies live, persists, and an unknown value falls back to `full` for that field alone (`player_options.md` §2, §4). No `auto`: nothing in the engine detects adapter tier.
- **Default `full`, set from the perf floor, not the Mac (owner decision).** The 5300M is in the compatibility-floor class (§10 names the 5500M class), which §10 calls "must run, not perf-tuned", and `player_options.md` §4 says Auto does not tune defaults for it. On the perf floor (GTX 1660 Super), `half` saves 0.40 ms of a ~5 ms GPU frame that is far from GPU-bound at 60 Hz, so it buys nothing visible there (`research.md` §Perf-floor measurement). The default therefore stays at full fidelity, like `surface_depth_quality`. No §10 divergence, and capture and goldens cover the default look. `half` is the opt-in for GPU-bound machines; nothing detects them, so the menu label and help text must tell a laptop player what the option buys.
- **Exactness becomes rate-relative. This diverges from a commitment.** §4 Sampled-row compose promises that every sampled slot equals frame N's full-resident compose, and `done/perf-sh-compose-sampled-row-gating` made rate-limiting a non-goal on that ground. New rule: at update divisor n, a sampled slot equals full-resident compose from at most n − 1 recorded compose frames earlier, except on a step frame (below); `full` (n = 1) is the old rule. The lag is accepted because steady-state content is low-frequency and the owner's A/B showed no artifact. Undo is the `full` setting.
- **Lag is counted in recorded frames, not time (owner decision).** A frame-count rule matches the bounded-lag rule and needs no clock. The worst case is stated, not hidden: at 30 fps, `half` means 15 Hz steady-state updates and about 33 ms of lag, which is where the saving matters most. Step frames are exempt, so the lag never applies to a sudden change.
- **Step frames compose every stale gated row (owner decision).** On any frame where an animated light's descriptor changes (`setLightAnimation`) or a light switches on or off, both rate-limited passes compose every stale gated row that frame, whatever its phase. Why: a split frame shows the old value beside the new one on half the 4 m bricks, the same pattern as `research.md` §Re-entry hazard, and scripted lights-out and alarm reveals are first-class set-pieces (`context/lib/index.md` §1). One flag per frame covers both passes. It needs no per-light row index, and steps are rare, so the steady-state saving is untouched. A step on a frame that records no compose carries to the next recorded frame.
- **No fast-curve rule yet; the strobe A/B decides (owner decision).** A fast curve changes every frame without a step, so it is the remaining case where `half` could split bricks. Nothing limits light-animation curves today: `E23--photosensitivity-source-floor` is an undrafted seed, so the A/B is the only guard. The manual A/B adds a strobe and a chase case. If either shows the split, add the cheap rule: while any active light's curve is fast, both passes run at full rate. The brief does not pre-build it.
- **Round-robin by row.** A stale gated row composes when its row id matches the frame's phase modulo n. The phase is planner-owned and advances once per frame that records compose. The divisor is 1 or 2; no third rate is built.
- **Bounded lag overrides phase (owner decision).** A gated row more than n − 1 recorded frames behind composes now, whatever its phase: re-entry into the gate, a camera cut. A row n − 1 or fewer recorded frames behind waits for its phase. Otherwise a row coming into view would show a value from when it last left view, which breaks §4's rule that lagging rows compose on re-entry before any consumer samples them (`research.md` §Re-entry hazard).
- **Only steady-state animated-delta compose defers.** Always composed at full rate: step frames (above); residency rows (install, slot reuse, partial eviction, and their writer rows); control-change repair; force-full-resident; static direct (Pass A); animated direct (Pass B) on a frame whose static or animated promotion weights change, and every row Pass A rewrote. Why: a deferred row would pair last frame's `(1 − w)` baked arm with this frame's `w` runtime arm, and "no receiver sums the light twice" (§4 Promoted static lights, Promoted animated lights; `research.md` §Exemption analysis).
- **Scope: the streamed indirect pass and Pass B.** The legacy whole-load path, billboard scatter compose, and animated lightmap compose stay at full rate. The lightmap compose is direct surface light — visible, high-frequency, and outside this measurement.
- **Placement: policy in player options, mechanism in the renderer.** `startup/render_profile.rs` maps the option to an update divisor through an exhaustive match; the renderer never names the option. A renderer never given a rate composes at `full`, the exactness reference. The bridge applies the rate live and full init re-applies it. Capture builds its own renderer and applies no player options, so it stays at `full`; force-full-resident still bypasses the rate. So no capture or golden test covers `half`; the planner tests and the manual A/B are its only coverage.
- **Diagnostics keep their meaning and gain one gauge.** A deferred row is stale, so it counts in "resident rows still lagging", and in "rows composed because they lagged" on the frame it composes. Each pass adds "rows deferred by update rate" to the `[SH streaming]` log and the Streaming tab; still-lagging minus deferred is the view-gate lag. It is zero at `full` and under force-full-resident. The capture report is unchanged.
- **Not in this brief:** adapter detection or `auto`; per-light change scoping (the follow-up `done/perf-sh-compose-sampled-row-gating` named); per-row shader cost; residency or install compose changes; bake or format changes.
- **At promotion:** `rendering_pipeline.md` §4 Sampled-row compose and §7.1 step 5 record the rate-relative invariant, exemptions and gauge; `player_options.md` §4 and the `ui.md` §5 slot list gain the option (eight writable options slots become nine).

### Scripting surface
```ts
import { defineReaction } from "postretro";
import { getGameState, stateEquals, updateState } from "postretro/ui";

const options = getGameState().options;
// options.animatedLightUpdate: Ref<"full" | "half">; writable, unreplicated.
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
The Luau mirror ships through the same typedef generator. The persisted form is `animated_light_update = "half"` in `settings.toml` (the opt-in; the default is `full`).

## Acceptance
### Automated
Planner, GPU-free:
- [ ] At half, a row that stays gated while its light stays active composes on exactly every second recorded frame. Over any two consecutive recorded frames, every such row composes at least once. Covers an odd gated count and a single gated row.
- [ ] At full, plans match today's: the existing planner tests and oracle equivalence tests pass with the rate set to full and no other change.
- [ ] A gated row more than n − 1 recorded frames behind composes that frame, whatever its phase. A gated row n − 1 or fewer recorded frames behind does not compose until its phase. Both directions are pinned at n = 2, directly and through the planner.
- [ ] At half, a lagging row that re-enters the gate composes that frame, whatever its phase. After a camera cut into a lagging room, every gated row there composes that frame.
- [ ] A lagging row that re-enters the gate composes that frame, whatever its phase, on the first recorded frame after full→half and on a fresh planner. (pin P4)
- [ ] At half, none of these is deferred: install, slot-reuse and partial-eviction rows and their writer rows; a control-change repair; force-full-resident; static direct; animated direct on a frame whose promotion weights change; any row static direct rewrote.
- [ ] Switching half to full mid-sequence: every deferred row composes on the next recorded frame. Switching full to half leaves no gated row lagging past the half bound.
- [ ] At half, on a step frame every stale gated row in both passes composes that frame, whatever its phase. Pinned for a descriptor write that changes bytes, a light switched on, and the last active light switched off; after the last one, nothing dispatches on later frames.
- [ ] The other side of the step predicate: a descriptor write or `set_active` that leaves the mirror bytes unchanged is not a step, and that frame defers by phase as usual. The switch-on and switch-off cases in the step row are driven through the bridge's descriptor write, the production path.
- [ ] A step on a frame that records no compose is honoured on the next recorded frame, and only that one, including after two or more non-recording frames in a row and when a second step lands among them. (pin P1)
- [ ] At half, an animated promotion-weight change on a frame that records no compose exempts Pass B on the next recorded frame: every stale gated Pass B row composes there, whatever its phase. (pin P2)
- [ ] A frame that records no compose does not advance the phase. Two planners fed the same frame sequence produce identical plans.
- [ ] Only divisors 1 and 2 are representable: the rate type cannot hold any other value.
- [ ] Randomized sequences at either rate: regions move, lights toggle, descriptors change, promotion weights change, frames skip compose, clusters install and evict, the rate switches between full and half, force-full-resident toggles, the session clears, and the generation rebases. After each recorded frame, no gated resident row lags by more than n − 1 recorded frames under the current n, and after each recorded step frame none lags at all. (pin P4)
- [ ] The deferred gauge equals the gated stale rows the rate held back that frame, and those rows are included in resident rows still lagging. A deferred row counts in rows composed because they lagged on the frame it composes. The gauge reports held-back rows when the pass's plan is empty, and reads zero on a frame that records no compose, at full, and under force-full-resident. The `[SH streaming]` log and the Streaming tab show it per pass. (pin P5)
- [ ] At half, the legacy whole-load compose still covers every affinity row on each frame it fires, and the billboard scatter and animated lightmap compose dispatch as at full.
Renderer, end to end:
- [ ] On a renderer driving a streamed fixture at half: steady animated frames compose about half the gated rows per pass and the deferred gauge is non-zero. A frame whose light-bridge snapshot changes a compose descriptor composes every stale gated row in both passes that frame. Set back to full, the gauge reads zero on the next recorded frame. (pin P1)
Option and surface:
- [ ] `animated_light_update` round-trips `full` and `half`. A file without it loads `full`. An unknown value falls back to `full` for that field alone, warns once, and survives in the file until the player writes the field.
- [ ] Every option value maps at the render-profile chokepoint through an exhaustive match: `full` to divisor 1, `half` to divisor 2.
- [ ] The catalog declares `options.animatedLightUpdate` as a writable, unreplicated enum of `full` and `half`, and the slot table holds it. A drift guard checks the catalog values against the slot parser in both directions.
- [ ] A menu write reaches the renderer on the next frame and schedules the settled save. Full init applies the saved value before the first gameplay frame.
- [ ] A rate set before a level change still applies on the first recorded frame of the new level. (pin P6)
- [ ] The regenerated TypeScript and Luau SDK typedefs and the typedef fixtures include the slot. The dev frontend's graphics tab reads and writes it with the Scripting surface names, and its script builds.
- [ ] The dev frontend's graphics row for the option carries a note that tells a player half frees GPU time on GPU-bound machines such as laptops. The label does not claim to cover surface animated lightmaps.
Capture:
- [ ] A renderer that was never given a rate composes at full. The capture path never applies the player option, and existing capture exactness and golden tests pass unchanged.
- [ ] Grep gates: no renderer source names `animated_light_update` or its option enum; no capture source calls the rate setter; the rate is read only by the streamed planner and its frame input, so the legacy whole-load compose, billboard scatter and animated lightmap compose cannot see it.
### Manual
- [x] Before promotion, on the GTX 1660 Super: per-pass GPU times at full and half on campaign-test, recorded in `research.md` §Perf-floor measurement. Default set to `full` from them.
- [ ] On this Mac, record a Metal System Trace (`rendering_pipeline.md` §12) of campaign-test: release build, Auto, 1280×720 logical, 30 s, once at full and once at half. At half, rows composed per pass per frame are about half of full (spike: 730 → 360 / 370) and the deferred gauge reads about half the gated rows. Streamed SH Compose and Streamed Animated Direct SH drop in line. Spike reference: 2.489 → 1.279 ms and 2.885 → 1.472 ms. Check machine-state confounders first (§12).
- [ ] Visual A/B at half against full on campaign-test, near the crusher and the slow-pulse lights. Look for seams at 4 m brick boundaries, shimmer, stepping, and brightness pops on the viewmodel or characters. There should be none.
- [ ] Turn into a room whose animated lights ran while it was out of view. No one-frame brick pattern appears.
- [ ] At half, step onto an a11y-strobe-test light pad (`content/dev/maps/a11y-strobe-test.map`): a light switching on with a new `setLightAnimation` curve. No one-frame brick split appears on the step.
- [ ] At half, view a strobe (`content/dev/maps/a11y-strobe-test.map`) and a chase (campaign-test's arena 1 sweep and arena 2 wave, from `arena-lights.ts`). (pin P3) If either shows a brick split, add the fast-curve rule (Decisions) before landing.
- [ ] Toggle the option live in the options menu mid-session. No artifact persists.

## Path
- **Seams:**
  - The spike (`spike/sh-compose-half-rate`, 0c2b2ac1f) filters in `PassStaleness::plan_into`'s gated branch and advances a global counter in `StreamedComposePlanner::plan_frame_into` after the `records_compose` early return. Replace the global and the env read with planner state and a rate field on `ComposePlannerFrame`, beside `force_full_resident`.
  - `force_full_resident` reaches the planner through `Renderer::prepare_streamed_sh_compose` and `ShResidencyState::prepare_compose_frame`.
  - Its source, `force_full_resident_sh_compose`, lives in full-renderer state, and its setter needs full init. The rate setter must accept a value before full init, as `set_surface_depth_quality` does, or run after `ensure_full_ready` as fog does.
- **Step signal:** `AnimatedLightBuffers` (`sh_volume.rs`) already sets `dirty` in `write_descriptor` and `set_active`, and only when the mirror bytes change. `upload_descriptors_if_dirty` clears it in `renderer_frame.rs`, and it must run before compose. So capture the step when the upload fires, then latch it on the planner until a frame records compose; don't read `dirty` at plan time. One mirror serves both passes' descriptors, so a step exempts both, which is conservative and fine.
- **Pass B exemption:** `plan_frame_into` folds promotion-weight changes into `animated_changed`. Keep those flags separate so `plan_into` can tell a weight-change frame from an animation frame.
- **Bounded-lag shape:** with n capped at 2, keep each pass's epoch snapshot from the previous recorded frame. A gated row may defer when it is current against that snapshot and is not carrying lag across a membership change. That rule is exact for n = 2 (`research.md` §Re-entry hazard) and costs one snapshot per pass, no per-row memory. Rival: a per-row last-composed-frame stamp, which adds 4 B per row per pass to the per-row bound in §4.
- **Rejected direction rivals:** a time-based rate (adds a clock to a frame-count rule; the stated 30 fps worst case is accepted instead), and per-light content-aware deferral (needs a per-light row index; the step flag covers sudden changes without one).
- **Oracle:** `compose_plan_oracle.rs` models the old push planner. Keep its equivalence tests at full, and prove half with independent property tests rather than teaching the oracle the split.
- **Option wiring:** follow `render_resolution`'s footprint (`research.md` §Touch points). Typedefs regenerate with `cargo run -p postretro-sim --bin gen-script-types`.
- **First slice:** the planner change, its tests and the deferred gauge, with the rate passed in directly. Then check the gauge on campaign-test, where about half of the gated rows should be deferred. This tests the riskiest assumption: that bounded lag and the exemptions leave the steady-state saving intact. Wire the option after that.
- **File size:** `renderer_types.rs`, `sh_streaming.rs`, `splash_lifecycle.rs` and the `engine_state_catalog.rs` data table are past the split threshold, but each gains only a field, an entry or a line, so no split is owed. The new planner logic belongs in `compose_staleness.rs` or a sibling module. `compose_plan.rs`, `options/mod.rs` and `options/bridge/mod.rs` are large mostly because of their tests.

## Open questions
- Whether the fast-curve rule ships. It follows the strobe and chase A/B — owner — decided during manual proof
- Menu label, help text and row placement. "Animated lighting: Full / Half rate" overstates the scope, because surface animated lightmaps stay at full rate. With `full` the default, the help text is how a laptop player learns that `half` frees GPU time — **delegated**
