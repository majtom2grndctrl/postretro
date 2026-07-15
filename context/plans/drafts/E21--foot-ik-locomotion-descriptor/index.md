# Foot IK + Locomotion Descriptor

## Goal
Stop feet from sliding and floating. Add a foot-IK modifier to the pose-modifier stack, feed it game-logic ground probes, and add a per-archetype locomotion descriptor that selects how each character syncs animation to motion — speed-scaled playback, IK planting, or both composed — plus a per-clip `travelSpeed` auto-derived from root motion so playback rate is exactly `measured / authored`. Enemies inherit the calibration; the shipped E10 speed-scaled walk becomes the degenerate single-mode case.

Depends on `E21--pose-modifier-stack` (the stack, per-bone masks, `PoseInputs`, and the tick→collector→renderer plumbing this plan extends).

## Scope

### In scope
- A foot-IK `PoseModifier` variant: a two-bone (hip-knee-ankle) analytic solver run once per leg over a **model-declared set of leg chains**, operating on masked local-TRS joints, consuming per-foot ground-probe results from `PoseInputs`. The data path — probe array, leg set, and solver loop — is sized for **N legs**, never hardcoded to two; this plan authors and tests bipeds. Wgpu-free and collision-free.
- Game-logic ground probes: the fixed tick computes each IK entity's animated foot world positions (one per declared leg) from the unmodified world pose, casts short downward rays against the collision world, and writes the results into the entity's `PoseInputs`.
- A locomotion descriptor authored per archetype (Rust ↔ TS/Luau): a sync-mode selector (speed-scaled / IK-plant / both) and a per-clip `travelSpeed` override.
- `travelSpeed` auto-derivation at load from the root-joint translation track (ground units per animated second), with the descriptor override taking precedence.
- Reworking speed-scaled playback so the rate is `measured_ground_speed / travelSpeed` (clamped), replacing the current `speed_xz / move_speed` reference; the E10 walk is the degenerate case when no root motion and no override exist.
- Leg-chain and foot-target joint tagging via glTF `extras`, reusing the mask mechanism from the stack plan.
- A behavior-preserving split of `crates/entities/src/components/animation.rs` (~1602 lines) before its playback-rate machinery is extended.
- Headless tests for probe determinism, IK planting on slopes, and `travelSpeed`-driven rate.

### Out of scope
- The pose-modifier stack, masks, aim modifiers, and base plumbing — the prerequisite plan.
- Hand IK onto the weapon, full-body IK beyond legs, cloth/hair, facial animation (Epic 21 non-goals).
- Any change to hit-zone authority — leg hit capsules keep reading the unmodified pose.
- Blend trees, directional locomotion, multi-locomotion-state selection — deferred (would need a state-table flag).
- Co-op avatar / weapon presentation and any wire change — the co-op-avatar plan.
- Content and tuning for more than two legs, and front legs doubling as melee attackers. The data path here is N-leg-sized, but authoring multi-legged monster archetypes, per-monster leg tuning, and the leg↔melee-activation bridge are a later spec (roadmap Epic 21, couples to Epic 16 melee).
- `travelSpeed` derivation for non-forward clips. Derivation measures root XZ displacement magnitude; a turn-in-place or strafe clip with near-zero net root displacement carries a descriptor `travelSpeed` override instead (directional locomotion is itself out of scope above).

## Acceptance criteria
- [ ] A character with foot IK enabled keeps both feet in contact with flat ground across a walk cycle — no visible sink or float — while a foot lifted by the clip still lifts.
- [ ] On a slope within the walkable limit, each foot plants to the slope surface at the correct height and the foot orients toward the ground normal; the pelvis is not pushed through the ground.
- [ ] When a downward probe finds no ground within reach, that leg falls back to the clip pose with no snap.
- [ ] A synthetic model tagged with more than two leg chains drives one independent two-bone solve per leg with no code change — the probe array, leg set, and solver loop are not biped-hardcoded.
- [ ] A clip authored with root motion reports an authored travel speed measured from its root translation; a descriptor `travelSpeed` override replaces the measured value.
- [ ] A character moving faster than its clip's authored travel speed plays the clip proportionally faster (and slower when slower), so the planted foot does not slide; the rate stays within the clamp bounds.
- [ ] An archetype set to speed-scaled-only syncs stride rate but does not IK-plant; set to IK-plant-only it plants but does not rate-scale; set to both, it does both.
- [ ] The shipped E10 enemy walk still speed-scales identically when its archetype declares no root motion, no override, and the speed-scaled mode — no behavioral regression.
- [ ] Ground probes produce identical results across repeated headless runs of the same tick sequence.
- [ ] After the `animation.rs` split, every pre-existing `postretro-entities` test passes unchanged and no resulting source file exceeds ~800 lines.

## Tasks

### Task 1: Split `animation.rs` into a module directory (behavior-preserving)
`crates/entities/src/components/animation.rs` is ~1602 lines and Task 4 extends its playback-rate machinery; split it first along existing seams, changing no behavior. Convert to an `animation/` directory with `mod.rs` re-exporting the current public surface (`MeshAnimation`, `AnimationState`, `InterruptPolicy`, `FadeSourceKind`, `InterruptedOutgoing`, `SwitchResult`, `RestartResult`, `switch_animation_state`, `restart_animation_clip`, `resolve_pending_animation_stamps`, the `RATE_*` / `DEFAULT_CROSSFADE_MS` consts) so `MeshComponent` and all `crates/postretro` callers keep resolving. Suggested cuts: the enums + `AnimationState` data model into `animation/state.rs`; the playback-rate/timeline slice of `impl MeshAnimation` (`update_playback_rate`, `normalized_playback_rate`, `playback_rate_needs_update`, `scaled_elapsed`, `previous_scaled_elapsed`, and the private rebase helpers) into `animation/playback.rs`; the registry-mutating verbs (`switch_animation_state`, `restart_animation_clip` + result enums) into `animation/transitions.rs`; the resolve-pass (`resolve_pending_animation_stamps` + its predicates) into `animation/resolve.rs`. The `MeshAnimation` struct + `new` stay in `mod.rs`. Move the in-file tests alongside their targets. All existing tests pass with no assertion edits.

### Task 2: `travelSpeed` auto-derivation at load
Derive each clip's authored travel speed at model load in `crates/model`. The root-joint translation track is already loaded (`AnimationClip.joints[0].translation` survives `gltf_loader::load_clip` — in-place clips simply have an empty track). For each clip, measure the horizontal (XZ) displacement of the root joint from the first to the last translation keyframe and divide by the clip duration to get ground units per animated second; a clip with no root translation track yields `None` (no authored travel speed). Store the derived per-clip value on the loaded model's clip metadata next to name/duration so the runtime can read it by clip index. This is derivation only — the override and the rate consumption live in Tasks 3 and 4. Add a `crates/model` unit test on a synthetic clip with a known root translation track.

### Task 3: Locomotion descriptor (Rust ↔ TS/Luau)
Add an authored locomotion descriptor to the mesh component surface. Extend `MeshDescriptor` with an optional `locomotion` block carrying a sync-mode selector with exactly three values — speed-scaled playback, IK planting, both — defaulting to speed-scaled when the block is absent (preserving current behavior). Add an optional per-state `travelSpeed` override (ground units per animated second, finite and > 0) on the animation-state descriptor; when present it replaces the Task 2 derived value for that state's clip. The mesh descriptor is decoded by hand (not serde-derived) in both `crates/scripting-core/src/data_descriptors/js/entity.rs` and `.../lua/entity.rs` — add parsing and validation to both, funnelling through one shared validator so QuickJS and Luau reject the same inputs (unknown sync-mode string, non-positive `travelSpeed`). Mirror the new fields into the SDK typedefs `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` (drift-tested twins) and the reference behaviors under `sdk/behaviors/reference/`. Casing per the Boundary inventory. Carry the decoded sync mode and per-state override onto the runtime `MeshAnimation` / `AnimationState` so the tick and the palette build can read them.

### Task 4: Playback-rate rework to `measured / travelSpeed`
Rework speed-scaled playback (extends `animation/playback.rs` from Task 1) to compute the rate as `measured_ground_speed / effective_travel_speed`, clamped by the existing `RATE_MIN`/`RATE_MAX` and gated by `RATE_CHANGE_EPSILON`, where `effective_travel_speed` is the active state's descriptor override (Task 3) if present, else the clip's derived value (Task 2). The producer `update_brain_animation_playback_rates` (`crates/postretro/src/sim/mod.rs`) already computes `speed_xz` from `agent_steering::path_state().velocity`; change its denominator from `agent.move_speed` to the effective travel speed, applied only for archetypes whose sync mode includes speed-scaling. Degenerate case: when a state has no derived travel speed and no override, fall back to the current `speed_xz / move_speed` behavior so the shipped E10 walk is byte-for-byte unchanged. Update the existing headless walk-rate test and add one asserting the `travelSpeed`-driven rate.

### Task 5: Foot-IK two-bone solver modifier (N-leg data path)
Add a foot-IK `PoseModifier` variant to the stack (the prerequisite plan's open enum), operating on masked local-TRS leg joints. The modifier holds a **set** of leg chains (not a fixed pair) and iterates them, indexing the matching `FootProbe` in `PoseInputs` by leg index — so a biped runs two solves and a hexapod six, from the same loop. Per leg (hip → knee → ankle/foot), given that leg's chain mask and its `FootProbe`, run a planar two-bone analytic solve so the ankle reaches the probed ground-contact target: forward-compute the current hip and ankle world positions from the chain's local transforms, solve the knee angle from the two segment lengths and the hip→target distance (clamped when the target is out of reach so the leg never hyperextends), and write back the hip and knee local rotations; orient the foot toward the probed ground normal within an angular limit. Each leg is independent — a probe miss leaves that leg on its clip pose without affecting the others. The solver reads only `PoseInputs` and the local buffer — no collision, no wgpu. Add `crates/model` unit tests: flat-ground plant, sloped-ground plant with normal alignment, out-of-reach clamp, miss fallback, and **a synthetic model with more than two leg chains producing one independent solve per leg** (proving the loop is not biped-hardcoded).

### Task 6: Game-logic ground probes feeding `PoseInputs`
Extend the tick→collector plumbing (from the prerequisite plan) with per-foot ground probes. Add a **fixed-capacity** foot-probe array plus a live count to `PoseInputs` (`feet: [FootProbe; MAX_FEET]`, `foot_count: u8`; each `FootProbe` carries contact height, ground normal, and a hit flag). The capacity is chosen to cover multi-legged monsters — not fixed at two — while keeping `PoseInputs` `Copy`/POD (state the constraint; the exact cap is an implementer choice, suggested in the sketch). In `simulate_tick` (`crates/postretro/src/sim/mod.rs`), for each IK-enabled animated entity, iterate its model-declared leg set (Task 7): compute each leg's animated (pre-IK) foot world position by sampling the unmodified world pose (`postretro_model::anim::sample_blended_world` / `sample_clip_looped_world` — already collision-free) at the entity's current anim params, transform that foot joint model→world via the entity transform, and cast a short downward ray with `collision::cast_ray` (or `cast_ray_combined` when movers are present) capturing time-of-impact and surface normal; write the per-leg results into `feet[0..foot_count]` on the entity's `MeshComponent.pose_inputs`, leg index aligned with the leg set. This stays in the tick (the collector borrows the registry immutably) and reuses the walkable-normal convention (`COS_WALKABLE`) already used by movement ground-stick. Add a determinism-harness test (`sim/determinism_tests.rs`) asserting repeatable probe outputs.

### Task 7: Leg-chain and foot-target tagging at load (indexed, N-leg)
Extend the loader's pose-mask reading (from the prerequisite plan) with an **indexed** leg scheme: per-node `extras` mask names `leg{i}` mark the joints of leg `i`'s hip→knee→ankle chain and `foot{i}` marks that leg's foot/ankle target joint (i = 0, 1, 2, …), reindexed through the same topo remap as the other masks. Collect these into an ordered leg set (capacity `MAX_FEET`; extra legs beyond capacity warn and are dropped). Build one foot-IK stack entry per declared leg when the archetype's sync mode includes IK planting, and surface the leg set on `LoadedModel` — each entry a `{ chain mask, foot joint index }` — so the tick's ground probe (Task 6) knows which joints to project and in what leg order. A biped tags `leg0`/`leg1` + `foot0`/`foot1`; nothing in the loader, solver, or probe path assumes exactly two. Mask names follow the Boundary inventory. Unknown/malformed tags warn, never fail.

### Task 8: Slope-planting integration test
Prove end-to-end planting on non-flat ground. Add a headless test driving `simulate_tick` for an IK-enabled entity walking across a sloped `CollisionWorld` fixture (mirroring the harness's `floor_world` but tilted), asserting the computed foot probes report the slope height/normal and that the foot-IK modifier applied with those probes plants the ankle at the surface rather than the clip's flat-ground height. CPU-only, no GPU/window.

## Sequencing

**Phase 1 (sequential):** Task 1 (split `animation.rs`) — blocks the playback-rate rework.
**Phase 2 (concurrent):** Task 2 (`travelSpeed` derive), Task 3 (locomotion descriptor), Task 5 (foot-IK modifier), Task 7 (leg/foot tagging) — disjoint files/crates.
**Phase 3 (concurrent):** Task 4 (playback rework — consumes Task 1 split + Task 2 derive + Task 3 sync mode/override), Task 6 (ground probes — consumes Task 5 modifier + Task 7 foot joints + the prerequisite plan's side channel).
**Phase 4 (sequential):** Task 8 (slope integration test — consumes Tasks 5, 6, 7).

## Rough sketch
- `crates/model`: `PoseModifier::FootIk { legs: .. }` variant on the prerequisite plan's enum, holding the leg set and looping it; a two-bone solver helper; `FootProbe { contact_height: f32, normal: Vec3, hit: bool }` plus `feet: [FootProbe; MAX_FEET]` and `foot_count: u8` on `PoseInputs` (`MAX_FEET` a small const covering multi-legged monsters — e.g. 6 — keeping the POD `Copy`); per-clip `travel_speed: Option<f32>` on the loaded clip metadata; an ordered `LegChain { chain_mask, foot_joint }` leg set on `LoadedModel`.
- `crates/entities`: `LocomotionSyncMode { SpeedScaled, IkPlant, Both }`; `MeshAnimation` gains the sync mode; `AnimationState` gains `travel_speed: Option<f32>`. Playback rate helper takes an `effective_travel_speed` argument.
- Descriptor decode: `data_descriptors/{js,lua}/entity.rs` parse `locomotion.syncMode` and per-state `travelSpeed`; shared validator; typedef twins updated.
- `crates/postretro`: `update_brain_animation_playback_rates` denominator swap; a `probe_feet(...)` step in `simulate_tick` writing `PoseInputs.feet` via `collision::cast_ray` / `cast_ray_combined` and `sample_*_world`.

## Boundary inventory
The locomotion descriptor crosses Rust ↔ TS/Luau (no wire or FGD surface — mesh presets are authored, not map KVPs). Casing: camelCase in TS/Luau and the decoded JSON/table, snake_case in Rust, matching the existing `crossfadeMs`/`crossfade_ms` and `moveSpeed`/`move_speed` conventions.

| Name | Rust | Authored key (TS / Luau) | Values |
|---|---|---|---|
| locomotion block | `MeshDescriptor.locomotion: Option<LocomotionDescriptor>` | `locomotion?` | object below |
| sync mode | `LocomotionSyncMode` | `syncMode` | `"speedScaled"` \| `"ikPlant"` \| `"both"` (default `"speedScaled"`) |
| per-clip travel speed override | `AnimationState.travel_speed: Option<f32>` | `travelSpeed?` (on an animation state) | finite, > 0; ground units / animated second |
| leg chain i (glTF `extras`) | leg `i` chain mask via `read_pose_masks` | node `extras` `poseMask: "leg0"`, `"leg1"`, … | topo joint set, one per leg |
| foot target i (glTF `extras`) | leg `i` foot joint | node `extras` `poseMask: "foot0"`, `"foot1"`, … | single joint, one per leg |

Leg/foot masks extend the prerequisite plan's `poseMask` `extras` mechanism; they are glTF-authoring only. The index `i` is the leg's position in the ordered leg set and aligns with its `PoseInputs.feet` slot. A biped authors `leg0`/`leg1` + `foot0`/`foot1`.
