# Research anchors — foot IK + locomotion descriptor

Source anchors grounding the spec (verified against current source). Not decisions — a fact-check aid. See the sibling `E21--pose-modifier-stack/research.md` for the stack/plumbing anchors this plan builds on.

## travelSpeed derivation — root translation IS loaded
- `gltf_loader::load_clip` (gltf_loader.rs:1396) loads every channel with no root/root-motion special-casing; translation channels write `joints[topo_idx].translation` (:1488) — the root joint (topo 0) is treated identically.
- Test evidence: `clips[1].joints[0].translation` sampled as "STEP translation on the root joint (topo 0)" (gltf_loader.rs:3211). So authored stride speed is measurable at load from `AnimationClip.joints[0].translation` — provided the DCC exported root motion as a root-joint translation channel; in-place clips have an empty track.
- `Track` fields are private; access via `times()` / `values()` (skeleton.rs:157). `AnimationClip { name, duration, joints }` (:215). CUBICSPLINE degrades to Linear at load.

## Playback-rate rework (crates/entities/src/components/animation.rs, ~1602 lines)
- `RATE_MIN = 0.5` (:20), `RATE_MAX = 1.5` (:23), `RATE_CHANGE_EPSILON = 0.02` (:26), `DEFAULT_CROSSFADE_MS = 150.0` (:14).
- Rate math on `MeshAnimation`: `update_playback_rate(raw_ratio, now)` (:221), `normalized_playback_rate` (:237, clamp), `playback_rate_needs_update` (:248), `scaled_elapsed` (:255), `previous_scaled_elapsed` (:269). Runtime rate triple is `#[serde(skip)]`, default 1.0.
- Producer today: `update_brain_animation_playback_rates` (`sim/mod.rs:294`) — `speed_xz` from `path_state().velocity` XZ, ratio `speed_xz / agent.move_speed` (`AgentComponent.move_speed`, agent.rs:51), applied only when current state == `states.animation_for(LogicalState::Alert)`. This is the denominator to swap to effective travel speed.
- Split seams (production code is lines 1–703; tests 704–1601): `animation/state.rs` (enums + `AnimationState`, :32–128), `animation/playback.rs` (rate/timeline slice, :215–299), `animation/transitions.rs` (`switch_animation_state` + `restart_animation_clip`, :387–619), `animation/resolve.rs` (`resolve_pending_animation_stamps`, :621–701).

## Descriptor surface (Rust ↔ TS/Luau)
- Mesh animation decode is hand-written (not serde-derived): `crates/scripting-core/src/data_descriptors/js/entity.rs` and `.../lua/entity.rs` parse `animations` / `defaultState`; `crossfadeMs` read at lua/entity.rs:263.
- `AnimationState { clip, looping (#[serde(rename="loop")]), crossfade_ms (#[serde(rename="crossfadeMs")]), interrupt, clip_index (skip) }` (animation.rs:60). `MeshDescriptor` in the same crate; TS types `sdk/types/postretro.d.ts` (MeshDescriptor/AnimationStateDescriptor around :37–55), Luau twin `sdk/types/postretro.d.luau`, drift-tested.
- Casing precedent: `AiDescriptor` (`crates/foundation/src/data_descriptors/types/combat.rs:250`) uses `#[serde(rename_all="camelCase")]`; `moveSpeed`/`move_speed` (:258). Validation `AiDescriptor::validate` funnels QuickJS + Luau through one path — mirror for the locomotion validator.
- `AiStateNames` (combat.rs:232, `deny_unknown_fields`): closed idle/alert/attack/death.

## Foot ground probe (collision)
- `CollisionWorld { mesh: TriMesh, isometry }` (`crates/postretro/src/collision/mod.rs:36`). No `raycast` method — free functions.
- `cast_ray(world, origin: Point<f32>, dir: Vector<f32>, max_toi) -> Option<RayIntersection>` (:279) — `mesh.cast_ray_and_get_normal(..., solid=true)`; `RayIntersection.time_of_impact` / `.normal`. Convert glam::Vec3 → parry `Point`/`Vector` at the call site.
- Mover-aware: `cast_ray_combined(...)` (`collision/moving.rs:163`). Walkable-normal constant `COS_WALKABLE = 0.643` (collision/mod.rs:132). Movement ground-stick precedent: `movement/substrate.rs` (`cast_ray_query` :525, `ground_ref_from_hit` :541).
- Animated foot world position is computable collision-free via `sample_blended_world` / `sample_clip_looped_world` (anim.rs) then entity model→world transform.

## E10 shipped locomotion contract
- `context/plans/done/E10--speed-scaled-walk-playback/`, `E10--enemy-locomotion-animation/`, `M10--skinned-animation-runtime/`.
- Idle-vs-walk: `moving = vel_xz_sq > MOVE_SPEED_EPSILON²` (0.05); `BrainComponent.locomotion_moving` latch; carries squared speed (sqrt for linear rate).
- Stated future seams (deliberately deferred in E10): authored rate fields / reference-speed overrides / rate curves in descriptors; blend trees / directional / multi-locomotion states.
