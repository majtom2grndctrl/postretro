# Research anchors — foot IK + locomotion descriptor

Source anchors grounding the spec, verified against current source after the prerequisite `E21--pose-modifier-stack` shipped. Not decisions — a fact-check aid. The stack, `PoseInputs`, `PoseModifier`, the `poseMask` loader path, and the modified samplers are landed, not assumed.

## Shipped stack surface this plan extends
- `PoseInputs` at `crates/foundation/src/pose.rs:8` — three `f32` fields (`aim_pitch`, `aim_yaw`, `heading_yaw`), `derive(Debug, Clone, Copy, PartialEq, Default)`, no serde. It is a closed POD with **no reserved room for feet** — `feet`/`foot_count` are net-new fields that must keep it `Copy`. Re-exported from `postretro_entities` (`crates/entities/src/lib.rs:27`).
- `PoseModifier` at `crates/model/src/pose_modifier.rs:71` — **closed enum, NOT `#[non_exhaustive]`** (doc comment: extend the dispatch directly, no dynamic dispatch in the palette hot path). Variants today: `AimPitchBend { bend_weights: Vec<f32> }`, `UpperLowerSplit { lower_body_mask: JointMask }`. `FootIk` is a direct add. `JointMask` (:19, fixed `[u64; 4]` bitset over `MAX_JOINTS=256`, `Copy`), `ModifierEntry { mask, modifier }` (:93), `PoseModifierStack` (:100). `apply_pose_modifier_stack(stack, inputs: &PoseInputs, skeleton, locals: &mut [LocalTrs])` (:124) matches each variant — the FootIk arm joins here.
- Modified samplers in `crates/model/src/anim/mod.rs`: `sample_clip_looped_modified` (:133), `sample_blended_modified` (:192), `sample_rest_pose_modified` (:167). Each materializes/resolves the `[LocalTrs]` buffer, runs `apply_pose_modifier_stack`, then `compose_palette`; empty stack or `inputs == None` delegates to the unmodified path (allocation-free). FootIk flows through these unchanged — no new sampler.
- World-pose samplers `sample_clip_looped_world` (:230), `sample_blended_world` (:260) compose the forward hierarchy to model-space joint transforms (pre-inverse-bind), **no `_modified` variant**. Collision-free; the probe step samples these for animated foot world position, leaving hit-zone authority untouched.

## Loader — leg/foot tags extend the shipped mask path
- `read_pose_masks(extras, node_index, path_str) -> JointPoseMetadata` (`crates/model/src/gltf_loader.rs:597`) exact-matches `"aimSpine"`/`"upperBody"`/`"lowerBody"` today; unknown names warn and are ignored. Extend with `legL`/`legR`/`footL`/`footR` and the `leg{i}`/`foot{i}` numeric form.
- `PoseMaskSet { aim_spine, upper_body, lower_body }` (:59) derives `Copy` (fixed masks). The N-leg leg set is variable-length — keep it a separate `Vec<LegChain>` field on `LoadedModel`, not inside `PoseMaskSet`.
- `LoadedModel` (:70) carries `pose_masks: PoseMaskSet` and `pose_stack: PoseModifierStack`; add the leg set beside them. Topo remap via `pose_masks_from_topo_metadata` (:678).
- `build_pose_modifier_stack(skeleton, masks, aim_bend_weights, path_str)` (:751) assembles entries in fixed order (split before aim bend); push a `FootIk` entry when the leg set is non-empty, mirroring the `aimSpine`→`AimPitchBend` rule. Chain validation precedent: `ordered_aim_spine_chain` (:708) returns root→tip only for a single connected non-branching chain.

## travelSpeed derivation — root translation IS loaded
- `gltf_loader::load_clip` (:1648) writes each translation channel at `joints[topo_idx].translation` (:1740) with no root special-casing; root = topo 0, in-place clips have an empty track. So authored stride speed is measurable from `AnimationClip.joints[0].translation`.
- `AnimationClip { name, duration, joints }` (`crates/model/src/skeleton.rs:216`); `JointTracks { translation, rotation, scale }` (:198). `Track` fields private — read via `times()` / `values()`. No `travel_speed` field exists today.

## Playback-rate rework
- Consts (`crates/entities/src/components/animation.rs`): `DEFAULT_CROSSFADE_MS=150.0` (:14), `RATE_MIN=0.5` (:20), `RATE_MAX=1.5` (:23), `RATE_CHANGE_EPSILON=0.02` (:26). File is ~1616 lines, still one file (Task 1 splits it).
- Rate math on `MeshAnimation`: `update_playback_rate` (:221), `normalized_playback_rate` (:237, clamp), `playback_rate_needs_update` (:248), `scaled_elapsed` (:255), `previous_scaled_elapsed` (:269). The type only clamps the incoming ratio; the denominator is the producer's.
- Producer `update_brain_animation_playback_rates` (`crates/postretro/src/sim/mod.rs:355`): `speed_xz` from `path_state().velocity` XZ (:368); `raw_ratio = speed_xz / agent.move_speed` (:369), gated to `animation_for(LogicalState::Alert)`. This is the denominator to swap to effective travel speed.
- `update_pose_inputs` (sim/mod.rs:412) is called immediately after the rate producer (:288) — the ground-probe step is its sibling in the tick.

## Descriptor surface (Rust ↔ TS/Luau) — shared validator already exists
- Both front-ends funnel through one shared validator `MeshDescriptor::build(model, states, default_state, animations_present)` (`crates/entities/src/data_descriptors/types/entity.rs:59`) — extend it for `travelSpeed` positivity/finiteness, do not build a parallel validator. `RawAnimationState` (:43) is the parsed-but-unvalidated entry; add `travel_speed` there.
- Front-ends: `mesh_descriptor_from_js` (`crates/scripting-core/src/data_descriptors/js/entity.rs:166`) + `raw_animation_state_from_js` (:208, `crossfadeMs` at :214); Luau twins `mesh_descriptor_from_lua` (`.../lua/entity.rs:197`) + `raw_animation_state_from_lua` (:257, `crossfadeMs` :263). Parse `locomotion.speedScale` and per-state `travelSpeed` in both.
- `AnimationState { clip, looping (rename "loop"), crossfade_ms (rename "crossfadeMs"), interrupt, clip_index (skip) }` (animation.rs:60). `MeshDescriptor { model, animations, default_state }` (entity.rs:28).
- Typedefs are **generated**, not hand-maintained twins: `sdk/types/postretro.d.ts` and `.d.luau` regenerate from the Rust typedef source via `gen-script-types` (`crates/postretro/src/bin/gen_script_types.rs`), guarded by `committed_sdk_types_match_current_registry` (`crates/postretro/src/scripting/typedef/tests/committed.rs:8`). Extend the source and regenerate; never hand-edit the twins. Reference behaviors: `sdk/behaviors/reference/entities.{ts,luau}` (mesh block e.g. `entities.ts:61`).

## Foot ground probe (collision)
- `cast_ray(world, origin: Point<f32>, dir: Vector<f32>, max_toi) -> Option<RayIntersection>` (`crates/postretro/src/collision/mod.rs:279`) — `.time_of_impact` / `.normal`. Mover-aware `cast_ray_combined(...) -> Option<CombinedCastHit>` (`collision/moving.rs:163`); `CombinedCastHit.time_of_impact` / `.normal` (`:33`). Both `pub(crate)` — the tick (same crate) calls them. Convert glam → parry `Point`/`Vector` at the call site.
- `COS_WALKABLE = 0.643` (mod.rs:132); floor classification `normal.y >= COS_WALKABLE` in `classify_contact` (moving.rs:402). Movement ground-stick precedent: `cast_ray_query` (`movement/substrate.rs:525`), `ground_ref_from_hit` (:541).
- No existing call site joins a `crates/model` `sample_*_world` output with a `crates/postretro` `cast_ray` — the probe step is that net-new bridge.

## E10 shipped locomotion contract
- `context/plans/done/E10--speed-scaled-walk-playback/`, `E10--enemy-locomotion-animation/`, `M10--skinned-animation-runtime/`.
- Idle-vs-walk from squared ground speed; `BrainComponent.locomotion_moving` latch (`crates/entities/src/components/brain.rs:155`). The degenerate rate case (no derived travel speed, no override, default `speedScale`) must stay byte-for-byte `speed_xz / move_speed`.
