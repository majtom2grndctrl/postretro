# Skinned Animation Runtime

> **Status:** draft.
> **Milestone:** 10 (Animated Enemies) — render foundation track, final bullet.
> **Prerequisite:** sibling plan `M10--gltf-skeleton-clip-loading` (all clips cached + name lookup) — orchestrate that plan first.
> **Related:** `context/lib/rendering_pipeline.md` §9 · `context/lib/entity_model.md` §5 · `context/lib/scripting.md` §1, §10 · `context/plans/done/M10--model-pipeline-slice/findings.md` (no-`ozz` decision).

## Goal

Build the animation state surface on the slice's raw single-clip sampling: per-entity logical states (idle / locomotion / attack / death vocabulary) declared as descriptor data, each mapping to a named clip with loop and crossfade policy; the engine samples and blends poses into the bone palette each frame, switches states via a tag-targeted reaction primitive and a Rust API (the future Enemy-AI plan's hook), and time-slices sampling for distant instances. Scripts declare; Rust executes — no live VM, no per-tick script.

## Scope

### In scope

- **Blend sampling.** A two-clip blended sample in the model module: sample both clips' local joint poses, blend per joint (lerp translation/scale, slerp rotation), compose the hierarchy once, apply inverse-bind. Allocation-free steady state, like `sample_clip`.
- **Loop policy.** Per state: looping clips wrap (today's behavior); non-looping clips clamp at the final keyframe and hold (death pose persists).
- **Declared state surface.** Entity-type descriptors gain `components.mesh`: a model handle plus an optional animation block — state name → `{ clip, loop?, crossfadeMs?, interrupt? }` — and a `defaultState`, required whenever `animations` is present (an unordered state map has no well-defined "first"). The four canonical names (`idle`, `locomotion`, `attack`, `death`) are documentation vocabulary; keys are author-defined strings. A descriptor with `components.mesh` is directly map-placeable via `canonicalName`.
- **Per-entity runtime state.** The mesh component carries the declared state map (copied at spawn) and mutable state: current state, previous state, and the timestamps crossfade and clip-local time derive from. `prop_mesh` entities (no descriptor) stay stateless: first clip, looped, phase offset — exactly today.
- **State switching.** A tag-targeted reaction primitive `setAnimationState` (`{ state: string }`), mirroring `setEmitterRate` semantics, plus a `pub(crate)` Rust setter by `EntityId` sharing the same validation — the seam the Enemy-AI plan drives.
- **Crossfade.** Switching states blends old → new over the *entered* state's `crossfadeMs` (default constant when unspecified; `0` = hard cut). During a fade the outgoing clip keeps playing on its own timeline (motion-preserving; a non-looping outgoing clip clamps as usual).
- **Interrupt policy (author-facing).** Each state entry may declare `interrupt: "smooth" | "snap"` — how a fade *into* it takes over when another fade is already in flight. (In A→B interrupted by C, the in-flight fade's incoming state B is the *interrupted state* and becomes the new outgoing source.) `"snap"`: the new fade blends from the interrupted state's clip and the in-flight blend drops — a deliberate, fade-window-bounded pop that reads as a snappier takeover. `"smooth"` (the default): the in-flight blended pose is captured once as a static per-joint local-TRS snapshot and the new fade blends from it — no discontinuity. A fade's outgoing source is therefore either a clip or a snapshot; a smooth interrupt of a snapshot-fade captures `blend(snapshot, clip)` the same way. Per-state, so one character can ease its locomotion but snap its attack. Steady-state cost ceiling stays two clip samples per instance per frame; the interrupt frame pays one extra one-time capture.
- **Animation clock.** All animation timing — entry stamps, clip-local times, fade windows, the pending-stamp resolve — reads one game-layer clock that *accumulates* `real_dt × time_scale` per frame (engine-side scale field, default `1.0`; no script surface yet). Accumulation, not scaling of absolute time, so changing the scale never jumps existing poses; scale `0` holds every clip and fade (pause). This is the slow-motion seam.
- **Clip resolution + validation at level load.** The model sweep records each model's clip metadata (the loader plan's cache query) game-side; each spawned state map validates against it — a state naming a missing clip warns once and becomes unusable (switching to it warns + no-op).
- **Per-instance sample inputs.** The collector resolves state → clip index and emits plain copyable sample data (clip indices, times, blend weight, loop flags) per instance; the render pass samples what it is told instead of hardcoding "first clip at render-clock + phase". No per-frame heap allocation per instance in steady state.
- **Wave de-sync.** Looping states keep the per-entity phase offset; one-shot (non-looping) states play from state entry, synced to their triggering event.
- **Animation time-slicing.** Distance-bucketed resampling: instances beyond configured camera-distance thresholds re-sample every Nth frame, re-uploading a cached palette on skipped frames. A state change or active crossfade forces a resample. Off-screen instances already cost nothing (culled before planning).

### Out of scope

- **State *selection* logic** — idle→alert→attack decisions are the Enemy-AI plan's; this plan plays whatever state is set.
- **Auto-transitions** (e.g. attack returning to idle when its clip ends) — caller policy, deferred with state selection.
- **Blend trees, locomotion blendspaces, per-bone masks, additive layers** — crossfade between two whole-skeleton poses is the ceiling here.
- **`ozz-animation-rs`.** Slice findings settle the CPU axis (~3.6 µs/skeleton); crossfade-only blending hand-rolls.
- **GPU changes.** Palette layout, instance SSBO, and shaders are untouched — this is CPU-side selection and blending feeding the existing buffers.
- **Animation-driven events** (footsteps, hit frames) and **despawn at death-clip end** — later plans.
- **Engine-wide time scaling.** The animation clock scales skeletal animation only; migrating fog, billboard, and light animation onto the shared clock (full slow-motion), and any script surface for the scale field, ride a future plan that consumes the clock seam built here.
- **FGD surface.** No new KVPs; the state map is descriptor-owned, never map-overridable (`entity_model.md` §4).

## Acceptance criteria

- [ ] A map entity whose classname matches a descriptor with `components.mesh` spawns and renders that model; its model uploads once through the existing level-load sweep.
- [ ] The entity plays its `defaultState` clip at spawn; dispatching a `setAnimationState` reaction step at its tag makes it play the new state's clip.
- [ ] During the fade window after a switch, the pose is a blend: at the switch instant it equals the old pose, after `crossfadeMs` it equals the new clip's pose, midway it differs from both (unit-verifiable on the palette, no GPU).
- [ ] Midway through a fade, the outgoing contribution equals the outgoing clip sampled at its *advanced* clip-local time — it differs from that clip's pose at the switch instant (unit-verifiable on the palette); a non-looping outgoing clip clamps while fading out.
- [ ] A non-looping state holds its final pose after the clip ends (palette stops changing); a looping state wraps.
- [ ] A switch during an active fade honors the *entered* state's interrupt policy: `"smooth"` shows no pose discontinuity at the interrupt instant (the blend source equals the in-flight blended pose — unit-verifiable on the palette), including when the active fade's source is itself a snapshot; `"snap"` blends from the *interrupted state's* clip. With no `interrupt` declared, behavior is `"smooth"`.
- [ ] A `"smooth"` interrupt whose capture frame was never planned (culled or budget-dropped) degrades that one fade to the fallback clip source — no panic, no stale snapshot, no store entry.
- [ ] The state-elapsed query reports elapsed seconds since state entry; a non-looping state reports complete exactly when its clip duration has elapsed, a looping state never does. A still-pending stamp reads as elapsed `0` / not complete, and a second switch in the same tick collapses the never-rendered intermediate state out of the fade.
- [ ] With the animation time scale at `0.5`, clip-local time and fade progression advance at half rate; at `0`, poses and fades hold; changing the scale mid-fade produces no pose jump (accumulated clock — unit-verifiable, no GPU).
- [ ] Unknown state name → warn, current state unchanged. Tagged non-mesh entity → warn, skipped. Empty target set → no-op (mirrors `setEmitterRate`).
- [ ] A state whose clip is absent from the model warns at level load; switching to it warns and is a no-op.
- [ ] `prop_mesh` entities render exactly as before this plan: first clip, looped, phase-offset.
- [ ] Two entities in the same looping state are not lock-step (distinct phases); two entities entering the same one-shot state both play it from the start.
- [ ] Instances beyond the slicing distance re-sample at the reduced rate — the per-frame resample count (a game-side counter at the bucketing decision) drops accordingly in a unit test — while a crossfading or state-changing distant instance still resamples that frame.
- [ ] `gen-script-types` output matches committed `.d.ts`/`.d.luau` (drift test); `cargo test -p postretro` passes; `cargo clippy -p postretro -- -D warnings` clean.

## Tasks

### Task 1: Blend + loop sampling in the model module

Add to `model/anim.rs` a blended sampler: two `(clip, time)` pairs plus a blend weight produce one palette — sample each clip's local TRS per joint (extract a TRS-returning core from `sample_local_pose`; the composing wrapper stays for the single-clip path), blend (component lerp; shortest-path slerp), then run the existing compose + inverse-bind sweep once. Either blend source may alternatively be a caller-provided per-joint local-TRS buffer — the `"smooth"` interrupt's snapshot (TRS, never matrices: a matrix snapshot could not slerp) — and a companion helper evaluates a blend of two sources *into* such a buffer (the one-time snapshot capture, including snapshot × clip when a snapshot-fade is itself interrupted). Add loop handling at the sampling boundary: a loop flag per sampled clip chooses wrap (`rem_euclid`, today's behavior) vs clamp-to-duration. `sample_clip` remains for the single-clip path (gaining the loop flag). Reuse the thread-local scratch; steady-state calls allocate nothing. Synthetic-skeleton unit tests pin blend endpoints, midpoint divergence, and clamp-vs-wrap.

### Task 2: Per-entity animation state + switch API

Extend `MeshComponent` (`scripting/components/mesh.rs`) with an optional animation block: the declared state map (state → clip name, loop, crossfade ms), `defaultState`, and runtime fields — current state, previous state with its own entry stamp (the outgoing clip keeps playing during a fade; only the `"smooth"` interrupt snapshot is static), the entered-at timestamp, the active fade's **source kind** (interrupted-state clip vs snapshot), and a resolved clip index per state (filled by Task 5's validation; unresolved = unusable). Absent block = stateless legacy entity. Add a `pub(crate)` switch function (registry + `EntityId` + state name) that validates (declared? resolved?), records previous state + stamps entry — the single path the reaction (Task 4), the future AI plan, and future command-buffer guards call. This task also owns the **animation clock** (see scope): an `App` field beside the existing `script_time` accumulator (`main.rs`), advanced at the same site by `frame_dt × time_scale` (a sibling `App` scale field, default `1.0`) and **not advanced while the dev-tools `freeze_time()` flag is set** — preserving the freeze contract that stops mesh poses today. The pass's `now_seconds` sampling parameter retires once all sample times arrive in the per-instance params (it survives only if the warn rate-limiter needs it). A switch stamps "pending", and a small game-layer resolve pass — run with `&mut` registry in the render-frame collection sub-stage, immediately before the collector — fills pending stamps from the frame's post-advance clock value. Pending semantics are pinned: the state-elapsed query reads a pending stamp as elapsed `0` / not complete, and a switch whose current state's stamp is still pending replaces it with **no fade contribution** — the never-rendered intermediate collapses out; the fade source stays whatever was last resolved (clip or snapshot). Serde round-trip tests per the existing component tests; a clock test proves scale `0.5` halves advancement and a mid-fade scale change causes no jump.

### Task 3: `components.mesh` descriptor surface

Add a mesh descriptor to `EntityTypeDescriptor` (`scripting/data_descriptors.rs`, both QuickJS and Luau parsers): `{ model: string, animations?: { [state]: { clip: string, loop?: boolean, crossfadeMs?: number, interrupt?: "smooth" | "snap" } }, defaultState?: string }`. Validation at parse: non-empty `model` and `clip` strings; `crossfadeMs` finite ≥ 0; `interrupt`, when present, must be `"smooth"` or `"snap"`; `animations` present requires `defaultState`, which must name a declared state; `animations` present but empty rejects. In `scripting/builtins/data_archetype.rs`: `attach_descriptor_components` builds the `MeshComponent` (declared map copied in, current = default state, stamp pending), adds a `DescriptorComponentKind::Mesh`, and `is_directly_map_placeable` includes the mesh component. Update `typedef.rs` SDK blocks and regenerate `sdk/types/postretro.d.ts` / `.d.luau`. The existing component-driven model sweep (`distinct_mesh_models`) picks these entities up with no change. Descriptor parse/attach tests mirror the emitter/light ones.

### Task 4: `setAnimationState` reaction primitive

New `scripting/reactions/set_animation_state.rs` mirroring `set_emitter_rate.rs`: typed args `{ state: String }`, per-target dispatch through Task 2's switch function; warn-and-skip on non-mesh or stateless targets, warn on unknown/unresolved state, debug-log empty target set. Register `"setAnimationState"` in `scripting/reactions/registry.rs`; mention it in the typedef reaction doc comments only (the `setEmitterRate` precedent — no typed step interface; SDK sugar stays deferred per Open questions). Tests mirror `set_emitter_rate.rs` including log-capture assertions.

### Task 5: Per-instance sample inputs through collector, planner, and pass

Replace the pass's hardcoded sampling with caller-supplied data. At the level-load model sweep (`main.rs`), read each uploaded model's clip metadata (the loader plan's `Renderer` accessor) into a game-side per-model map — handle → clip name/duration/index list — owned beside the `MeshRenderCollector` and passed into `collect`; the collector needs durations to compute looping phase and clip-local times game-side (today the pass derives phase from `clip.duration`; that moves here with the rest of time computation). Resolve + validate every mesh entity's state map against it (fill per-state clip indices; warn per missing clip). Extend `MeshInstanceInput` (`render/mesh_instances.rs`) with copyable sample params — primary clip index + time, optional secondary clip index + time + blend weight, loop flags — defaulted for stateless entities to today's behavior (first clip, looped, animation clock + phase). The collector (`scripting/systems/mesh_render.rs`) gains the animation-clock value alongside `alpha` and computes per-entity times: clip-local time from entry stamp (+ phase for looping states), crossfade weight from the entered state's fade window. On a `"smooth"` interrupt frame the collector emits a one-shot capture instruction carrying the in-flight blend's inputs — outgoing source (a `(clip, time)` pair *or* a reference to the entity's stored snapshot), incoming `(clip, time)`, and weight; all copyable, snapshots referenced by entity seed. The pass evaluates it once into a per-entity snapshot store: a plain CPU-side map keyed by entity seed, each entry tagged with its fade's entry stamp — a GPU-free seam (the `model_bounds` precedent) so the smooth ACs test without a device. Subsequent snapshot-fade frames carry source kind = snapshot *plus* the interrupted state's `(clip, time)` pair as fallback: a store hit with matching tag blends against the snapshot; a miss — the capture frame was culled or budget-dropped (the planner drops renderer-side, invisible to the game layer) — degrades that fade to `"snap"` via the fallback pair, a discontinuity no one saw because the entity was not drawn at the interrupt instant. Store lifecycle: an entry drops on the first planned frame without an active snapshot fade (fade over, or tag mismatch on replacement) and on the level-load clear — a new `MeshPass` clear hook introduced here, called where the model cache installs; entries for entities never planned again are bounded by entity count and die with the level. `PlannedInstance` carries the params through `plan_mesh_frame`; `plan_and_upload` (`render/mesh_pass.rs`) samples via Task 1's API — single, blended, or snapshot-blended per the params. This task also adds the `pub(crate)` **state-elapsed query** beside the metadata map: current state, elapsed seconds since entry, and — for non-looping states — whether the clip has completed; consumed by tests now, with the Enemy-AI plan's return-to-idle / despawn-after-death and future interruption guards as the named consumers. Collector and planner tests pin stateless defaults, stateful times, crossfade weights, and both interrupt policies.

### Task 6: Animation time-slicing

Distance-bucketed resampling, decided game-side and cached renderer-side. The collector receives the camera position, computes each instance's distance, and marks a resample flag from named threshold constants (e.g. every frame near, every 2nd / 4th frame in two far buckets — constants are tuning knobs, documented at the definition). A state change since last sample, an active crossfade, or a cache miss forces resample. The pass keeps a per-entity palette cache keyed by the instance's entity seed: resample frames sample and update the cache; skipped frames re-upload the cached run at the instance's current palette base. Eviction: entries absent from the current frame's plan drop each frame, so the cache never exceeds the frame's planned instances (≤ `MAX_INSTANCES` entries, ≤ `MAX_PALETTE_ENTRIES` total slots) and a culled instance re-entering view misses the cache — the miss forces a resample, so no stale pose shows. The cache also clears on level load via the `MeshPass` clear hook Task 5 introduced for the snapshot store, extended here to the palette cache. Resample decisions are counted game-side where the bucketing happens, so a collector unit test asserts the reduced rate without a device (`pose_sample_stats` is log-only, env-gated, and not a test surface).

## Sequencing

**Phase 1 (concurrent):** Task 1 (model module), Task 2 (component + switch API) — independent.
**Phase 2 (concurrent):** Task 3, Task 4 — both consume Task 2's component shape / switch function; independent of each other.
**Phase 3 (sequential):** Task 5 — consumes Task 1's sampler, Task 2's state, Task 3's spawned entities.
**Phase 4 (sequential):** Task 6 — layers on Task 5's per-instance flow.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| mesh descriptor | `MeshDescriptor` on `EntityTypeDescriptor` | n/a (descriptor, not serialized) | `components.mesh` | `components.mesh` | n/a |
| model handle | `MeshComponent.model` | `"model"` | `model` | `model` | `model` (existing `prop_mesh` key only) |
| animation block | `animations` map | `"animations"` | `animations` | `animations` | n/a |
| state entry fields | `clip` / `looping` (`#[serde(rename = "loop")]` — `loop` is a Rust keyword) / `crossfade_ms` | `"clip"` / `"loop"` / `"crossfadeMs"` | `clip` / `loop` / `crossfadeMs` | same | n/a |
| default state | `default_state` | `"defaultState"` | `defaultState` | `defaultState` | n/a |
| interrupt policy | `InterruptPolicy::{Smooth, Snap}` | `"interrupt"`: `"smooth"` / `"snap"` | `interrupt` | same | n/a |
| switch reaction | `set_animation_state::dispatch` | `"setAnimationState"` | reaction step `primitive: "setAnimationState"`, args `{ state }` | same | n/a |
| state names | verbatim `String`, case-sensitive | author-defined | canonical vocabulary documented: `"idle"`, `"locomotion"`, `"attack"`, `"death"` | same | n/a |

## Open questions

- **Default crossfade constant** when `crossfadeMs` is unspecified — proposed 150 ms; cosmetic, tune on device.
- **Time-slice thresholds** — proposed two buckets (≈20 m / 40 m); constants to tune against a wave scene, not contracts.
- **`phase_seed` naming** — it becomes both phase seed and palette-cache key (it is already the raw `EntityId`); rename to an entity-seed name at the implementer's discretion.
- **SDK sugar** — whether a typed step-constructor helper for `setAnimationState` is worth adding now or rides with the Enemy-AI plan's SDK work; the raw reaction step shape works without it.
- **Game-side poses for hit zones.** Poses are sampled renderer-side for planned (visible) instances only. The skeletal-hit-zones plan needs visibility-independent joint poses game-side; the model module is CPU-only by contract, so a game-side sampling path is additive — design it in that plan, not here.
- **Command-buffer convergence.** The switch function is the intended entry point for future command-buffer-driven state selection — transition guards and interruptibility windows wrap the switch caller-side; the runtime stays dumb. The state-elapsed query is the primitive those guards read.
