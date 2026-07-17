# Research anchors — foot IK + locomotion descriptor

Current-source fact-check for this in-progress plan. The E21 implementation is
landed. These anchors describe the present code; the historical baseline below
explains which assumptions the original plan replaced.

## Current pose and IK surface

- `crates/foundation/src/pose.rs` defines `FootProbe`, `MAX_FEET = 6`, and
  `PoseInputs.feet` / `foot_count`. Probes carry model-space contact height,
  normal, and hit state. `PoseInputs` remains `Copy` and has no serde form.
- `crates/model/src/pose_modifier.rs` contains the closed `PoseModifier` enum,
  including `FootIk { legs: Vec<LegChain> }`. The stack applies each leg against
  its matching probe; a miss or clip-lifted swing pose preserves that leg's
  clip pose. A hit outside leg reach clamps to the nearest reachable-annulus
  boundary.
- `crates/model/src/anim/mod.rs:133` / `:167` / `:192` expose the modified
  clip, rest-pose, and blend samplers. They materialize local TRS only for an
  active stack and inputs, apply the stack, then compose the palette. The world
  samplers at `:230` and `:260` stay unmodified for hit-zone and attachment
  queries.

## Loader and authored leg data

- `crates/model/src/gltf_loader.rs:641` reads `poseMask` metadata. It supports
  existing aim/body names plus `legL` / `legR` / `footL` / `footR` and indexed
  `leg{i}` / `foot{i}` tags.
- `PoseMaskSet` stays fixed and `Copy` (`gltf_loader.rs:70`). `LoadedModel`
  carries variable-length `legs: Vec<LegChain>` beside it (`:80`), capped by
  `MAX_FEET` and reindexed into skeleton topological order.
- `pose_masks_from_topo_metadata` builds the ordered leg set; malformed or
  incomplete chains warn and drop. `build_pose_modifier_stack` (`:933`) appends
  one `FootIk` entry when that set is non-empty, after the body split and
  aim-pitch entries.

## Travel-speed calibration

- `AnimationClip.travel_speed: Option<f32>` is present in
  `crates/model/src/skeleton.rs:216`.
- `gltf_loader::load_clip` derives it after loading tracks
  (`gltf_loader.rs:2086`). `derive_and_neutralize_root_motion` (`:2111`)
  measures selected-root first-to-last XZ displacement over duration, then
  removes its linear net XZ drift. Absent, near-in-place, or invalid clips
  yield `None`.
- `crates/entities/src/components/animation/` replaces the former monolithic
  `animation.rs`. `state.rs` owns per-state `travel_speed`; `mod.rs` owns the
  `speed_scale` runtime flag; `playback.rs` owns rate calculation.
- `crates/postretro/src/sim/mod.rs:370` selects an authored state override,
  otherwise the loaded clip speed, before falling back to `move_speed`. It
  skips scaling when `speed_scale` is false. The E10 `speed_xz / move_speed`
  behavior is therefore the no-calibration fallback, not current primary logic.

## Descriptor and SDK boundary

- `MeshDescriptor`, `LocomotionDescriptor`, and `RawAnimationState` in
  `crates/entities/src/data_descriptors/types/entity.rs` carry `speed_scale`
  and optional `travel_speed`. The shared builder validates a present travel
  speed as finite and positive.
- Both scripting readers parse `locomotion.speedScale` and per-state
  `travelSpeed`: `crates/scripting-core/src/data_descriptors/js/entity.rs` and
  `crates/scripting-core/src/data_descriptors/lua/entity.rs`.
- SDK type files remain generated from the Rust typedef registry and are
  guarded by the committed-type drift test; do not hand-edit the generated
  TypeScript or Luau twins.

## Ground-probe bridge

- `update_pose_inputs` and `update_foot_ground_probes` in
  `crates/postretro/src/sim/mod.rs:458` and `:570` run in the fixed tick.
  Probes sample the unmodified world pose, ray-cast downward, reject
  non-walkable surfaces with `COS_WALKABLE`, and write model-space results to
  `PoseInputs`.
- Collision remains in the game crate: `cast_ray` is at
  `crates/postretro/src/collision/mod.rs:280`; mover-aware casts remain in
  `collision/moving.rs:163`. Model sampling stays CPU-only and does not take a
  collision dependency.

## Historical baseline

Before E21 landed, `PoseInputs` had only aim/heading values; `PoseModifier` had
only aim-pitch and upper/lower-body variants; the loader recognized only the
three original pose masks; and `AnimationClip` had no travel-speed field.
Playback-rate code also lived in the single
`crates/entities/src/components/animation.rs`. Those are historical planning
conditions, not descriptions of current source.
