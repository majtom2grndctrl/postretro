# Research anchors — pose-modifier stack

Source anchors grounding the spec (verified against current source). Not decisions — a fact-check aid.

## The seam (crates/model/src/anim.rs, ~1437 lines)
- Local-pose resolution and hierarchy composition are already factored apart — the seam exists.
- `LocalTrs { translation: Vec3, rotation: Quat, scale: Vec3 }` (anim.rs:51) — the intermediate per-joint local pose; `to_mat4` at :64.
- `resolve_blend_into(a, b, weight, skeleton, out: &mut Vec<LocalTrs>)` (:386) fills the per-joint blended local buffer.
- `compose_world_pose(skeleton, world, local_of)` (:159) — shared parent-before-child forward sweep, PRE-inverse-bind.
- `compose_palette(skeleton, out, local_of)` (:195) — multiplies world × `inverse_bind`, pushes `BonePaletteEntry`.
- Insertion point: inside `sample_blended` (:285), between `resolve_blend_into` (line 294) and `compose_palette` (line 295). Single-clip path `sample_clip_looped` (:256) fuses sample+compose in a per-joint closure and does NOT materialize a `LocalTrs` buffer — must be routed through one for cross-joint modifiers.
- World-pose variants `sample_clip_looped_world` (:315), `sample_blended_world` (:345) — used by hit-zones/attachments; must stay unmodified.
- No per-bone mask / joint-chain / additive-layer machinery exists yet — all net-new.
- `BonePaletteEntry { matrix: [[f32;4];4] }` (lib.rs:57); `MAX_JOINTS = 256` (mesh.rs:11), `u8` joint indices.
- `Joint { parent: Option<usize>, inverse_bind, rest_local }` (skeleton.rs:39); no joint-name field — joints identified by index. Parent-before-child guaranteed.

## Palette build site (renderer, not entities)
- The renderer builds the palette: `crates/renderer/src/render/mesh_pass.rs` calls `postretro_model::anim::sample_clip_looped` / `sample_blended` / `capture_blend`.
- The game-side collector `MeshRenderCollector::collect_inner` (`crates/postretro/src/scripting/systems/mesh_render.rs:227`) emits copyable `MeshSampleParams` in `MeshInstanceInput`; it does NOT compute the palette. Entry: `collect_with_hit_zones` (:203); `seed = id.to_raw()` (:281).
- `animate_entity(anim, anim_time, phase) -> Option<AnimResult>` (`scripting/systems/mesh_anim.rs:204`) resolves sample params.

## extras tagging precedent
- `read_joint_zone(node.extras) -> Option<JointZone>` (gltf_loader.rs:550); `JointZone { tag: String, radius: Option<f32> }` (:44); `JointZoneExtras` reads per-node keys `hitZone`/`hitZoneRadius` (:533).
- Reindexed through the topo remap into `LoadedModel.joint_zones` (:867), parallel to `Skeleton::joints`.
- Hit-zones consume tags + parent/first-child hierarchy to form capsules (`hit_zones.rs` `nearest_zone_hit`); no named "chain" abstraction today — per-joint tag string only.

## Tick → collector ordering (plumbing constraint)
- `simulate_tick(...)` (`crates/postretro/src/sim/mod.rs:104`) is the fixed-tick seam; `update_brain_animation_playback_rates` (:294) runs after steering.
- The render collector borrows the registry IMMUTABLY, so per-entity pose-input writes MUST happen in the tick, not the collector.
- Enemy aim source: `BrainComponent.acquired_target: Option<EntityId>` (`crates/entities/src/components/brain.rs:160`); velocity/facing via `agent_steering::path_state(registry, id)` (`agent_steering.rs:222`) → `AgentPathState.velocity`. Player aim: `Camera.pitch`/`yaw` (`crates/postretro/src/camera.rs:88`), reaching the tick via `PostMovementCommand.aim_direction` and `MovementInput.facing_yaw`.
- `MeshComponent { model, animation: Option<MeshAnimation>, origin_offset }` (`crates/entities/src/components/mesh.rs:28`) — add the transient `pose_inputs` here.

## Headless harness
- `crates/postretro/src/sim/determinism_tests.rs`: `tick`/`run_stream` drive `simulate_tick` directly with `floor_world()` + `HitZoneStore::new()`, no device. Template test `simulate_tick_scales_walk_rate_...` (:993).
- `crates/model` tests are in-file `#[cfg(test)] mod tests`, CPU-only, deterministic.

## Layering
- `crates/model` (`postretro-model`) is CPU-only: no wgpu, no parry3d/collision (Cargo.toml confirmed). Collision lives in the `postretro` binary (`collision/mod.rs`).
