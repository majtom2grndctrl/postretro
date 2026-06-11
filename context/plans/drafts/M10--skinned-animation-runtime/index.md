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
- **Declared state surface.** Entity-type descriptors gain `components.mesh`: a model handle plus an optional animation block — state name → `{ clip, loop?, crossfadeMs? }` — and an optional `defaultState`. The four canonical names (`idle`, `locomotion`, `attack`, `death`) are documentation vocabulary; keys are author-defined strings. A descriptor with `components.mesh` is directly map-placeable via `canonicalName`.
- **Per-entity runtime state.** The mesh component carries the declared state map (copied at spawn) and mutable state: current state, previous state, and the timestamps crossfade and clip-local time derive from. `prop_mesh` entities (no descriptor) stay stateless: first clip, looped, phase offset — exactly today.
- **State switching.** A tag-targeted reaction primitive `setAnimationState` (`{ state: string }`), mirroring `setEmitterRate` semantics, plus a `pub(crate)` Rust setter by `EntityId` sharing the same validation — the seam the Enemy-AI plan drives.
- **Crossfade.** Switching states blends old → new over the *entered* state's `crossfadeMs` (default constant when unspecified; `0` = hard cut).
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
- **FGD surface.** No new KVPs; the state map is descriptor-owned, never map-overridable (`entity_model.md` §4).

## Acceptance criteria

- [ ] A map entity whose classname matches a descriptor with `components.mesh` spawns and renders that model; its model uploads once through the existing level-load sweep.
- [ ] The entity plays its `defaultState` clip at spawn; dispatching a `setAnimationState` reaction step at its tag makes it play the new state's clip.
- [ ] During the fade window after a switch, the pose is a blend: at the switch instant it equals the old pose, after `crossfadeMs` it equals the new clip's pose, midway it differs from both (unit-verifiable on the palette, no GPU).
- [ ] A non-looping state holds its final pose after the clip ends (palette stops changing); a looping state wraps.
- [ ] Unknown state name → warn, current state unchanged. Tagged non-mesh entity → warn, skipped. Empty target set → no-op (mirrors `setEmitterRate`).
- [ ] A state whose clip is absent from the model warns at level load; switching to it warns and is a no-op.
- [ ] `prop_mesh` entities render exactly as before this plan: first clip, looped, phase-offset.
- [ ] Two entities in the same looping state are not lock-step (distinct phases); two entities entering the same one-shot state both play it from the start.
- [ ] Instances beyond the slicing distance re-sample at the reduced rate — the per-frame sampled-instance count (existing pose-sample stats hook) drops accordingly in a synthetic test — while a crossfading or state-changing distant instance still resamples that frame.
- [ ] `gen-script-types` output matches committed `.d.ts`/`.d.luau` (drift test); `cargo test -p postretro` passes; `cargo clippy -p postretro -- -D warnings` clean.

## Tasks

### Task 1: Blend + loop sampling in the model module

Add to `model/anim.rs` a blended sampler: two `(clip, time)` pairs plus a blend weight produce one palette — sample each clip's local TRS per joint (reusing `sample_local_pose` and the track samplers), blend (component lerp; shortest-path slerp), then run the existing compose + inverse-bind sweep once. Add loop handling at the sampling boundary: a loop flag per sampled clip chooses wrap (`rem_euclid`, today's behavior) vs clamp-to-duration. `sample_clip` remains for the single-clip path (gaining the loop flag). Reuse the thread-local scratch; steady-state calls allocate nothing. Synthetic-skeleton unit tests pin blend endpoints, midpoint divergence, and clamp-vs-wrap.

### Task 2: Per-entity animation state + switch API

Extend `MeshComponent` (`scripting/components/mesh.rs`) with an optional animation block: the declared state map (state → clip name, loop, crossfade ms), `defaultState`, and runtime fields — current state, previous state, the entered-at timestamp, and a resolved clip index per state (filled by Task 5's validation; unresolved = unusable). Absent block = stateless legacy entity. Add a `pub(crate)` switch function (registry + `EntityId` + state name) that validates (declared? resolved?), records previous state + stamps entry — the single path both the reaction (Task 4) and the future AI plan call. Timestamps share the render-frame clock the pass samples with: a switch stamps "pending", and a small game-layer resolve pass — run with `&mut` registry in the render-frame collection sub-stage, immediately before the collector — fills pending stamps from the frame's `now_seconds`. Serde round-trip tests per the existing component tests.

### Task 3: `components.mesh` descriptor surface

Add a mesh descriptor to `EntityTypeDescriptor` (`scripting/data_descriptors.rs`, both QuickJS and Luau parsers): `{ model: string, animations?: { [state]: { clip: string, loop?: boolean, crossfadeMs?: number } }, defaultState?: string }`. Validation at parse: non-empty `model` and `clip` strings; `crossfadeMs` finite ≥ 0; `defaultState` must name a declared state; `animations` present but empty rejects. In `scripting/builtins/data_archetype.rs`: `attach_descriptor_components` builds the `MeshComponent` (declared map copied in, current = default state, stamp pending), adds a `DescriptorComponentKind::Mesh`, and `is_directly_map_placeable` includes the mesh component. Update `typedef.rs` SDK blocks and regenerate `sdk/types/postretro.d.ts` / `.d.luau`. The existing component-driven model sweep (`distinct_mesh_models`) picks these entities up with no change. Descriptor parse/attach tests mirror the emitter/light ones.

### Task 4: `setAnimationState` reaction primitive

New `scripting/reactions/set_animation_state.rs` mirroring `set_emitter_rate.rs`: typed args `{ state: String }`, per-target dispatch through Task 2's switch function; warn-and-skip on non-mesh or stateless targets, warn on unknown/unresolved state, debug-log empty target set. Register `"setAnimationState"` in `scripting/reactions/registry.rs`; declare it in the typedef reaction docs. Tests mirror `set_emitter_rate.rs` including log-capture assertions.

### Task 5: Per-instance sample inputs through collector, planner, and pass

Replace the pass's hardcoded sampling with caller-supplied data. At the level-load model sweep (`main.rs`), read each uploaded model's clip metadata (loader plan's cache query) and resolve + validate every mesh entity's state map (fill per-state clip indices; warn per missing clip). Extend `MeshInstanceInput` (`render/mesh_instances.rs`) with copyable sample params — primary clip index + time, optional secondary clip index + time + blend weight, loop flags — defaulted for stateless entities to today's behavior (first clip, looped, render-clock + phase). The collector (`scripting/systems/mesh_render.rs`) gains the frame clock alongside `alpha` and computes per-entity times: clip-local time from entry stamp (+ phase for looping states), crossfade weight from the entered state's fade window. `PlannedInstance` carries the params through `plan_mesh_frame`; `plan_and_upload` (`render/mesh_pass.rs`) samples via Task 1's API — single or blended per the params. Collector and planner tests pin stateless defaults, stateful times, and crossfade weights.

### Task 6: Animation time-slicing

Distance-bucketed resampling, decided game-side and cached renderer-side. The collector receives the camera position, computes each instance's distance, and marks a resample flag from named threshold constants (e.g. every frame near, every 2nd / 4th frame in two far buckets — constants are tuning knobs, documented at the definition). A state change since last sample or an active crossfade forces resample. The pass keeps a per-entity palette cache (keyed by the instance's entity seed, cleared on level load, bounded by the existing instance budget): resample frames sample + update the cache; skipped frames re-upload the cached run at the instance's current palette base. The existing `pose_sample_stats` counter verifies reduced sampling in a synthetic test.

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
| state entry fields | `clip` / `loop` / `crossfade_ms` | `"clip"` / `"loop"` / `"crossfadeMs"` | `clip` / `loop` / `crossfadeMs` | same | n/a |
| default state | `default_state` | `"defaultState"` | `defaultState` | `defaultState` | n/a |
| switch reaction | `set_animation_state::dispatch` | `"setAnimationState"` | reaction step `primitive: "setAnimationState"`, args `{ state }` | same | n/a |
| state names | verbatim `String`, case-sensitive | author-defined | canonical vocabulary documented: `"idle"`, `"locomotion"`, `"attack"`, `"death"` | same | n/a |

## Open questions

- **Default crossfade constant** when `crossfadeMs` is unspecified — proposed 150 ms; cosmetic, tune on device.
- **Time-slice thresholds** — proposed two buckets (≈20 m / 40 m); constants to tune against a wave scene, not contracts.
- **`phase_seed` naming** — it becomes both phase seed and palette-cache key (it is already the raw `EntityId`); rename to an entity-seed name at the implementer's discretion.
- **SDK sugar** — whether a typed step-constructor helper for `setAnimationState` is worth adding now or rides with the Enemy-AI plan's SDK work; the raw reaction step shape works without it.
