# Research notes — E21 Bone Sockets + Attachments

Code-grounding map from the drafting session (2026-07). Line numbers are drift-prone; treat as starting points.

## Rigid single-bone render path (already complete)

- Non-skinned models load with one identity joint; vertices bind fully to joint 0 (`SkinnedVertex::rigid`, `crates/model/src/mesh.rs:40`; loader note `crates/model/src/gltf_loader.rs:1209`).
- Per-instance SSBO entry = model matrix + palette base index (`build_instance_entry`, `crates/renderer/src/render/mesh_pass.rs:66-84`); a rigid model's palette entry is identity (identity-fallback branch, `mesh_pass.rs:761-768`), so placement rides entirely on the instance transform. No GPU work needed for attachments.
- Frame plan: `MeshInstanceInput` (`crates/render-cpu/src/mesh_instances.rs:48`) → `plan_mesh_frame` (`:189`); every model consumes ≥1 palette slot (`:34-38`); `MAX_INSTANCES == MAX_PALETTE_ENTRIES == 4096`.

## Pose pipeline and the sampler gap

- Pipeline: clip sample (`anim/track.rs`) → blend (`anim/blend.rs`) → `apply_pose_modifier_stack` (`crates/model/src/pose_modifier.rs:152`) → compose (`anim/compose.rs`).
- `compose_world_pose` (`anim/compose.rs:27`) yields model-space joint matrices pre-inverse-bind; `compose_palette` (`:63`) multiplies inverse-bind for skinning.
- Unmodified world samplers `sample_clip_looped_world` / `sample_blended_world` (`anim/mod.rs:230,260`) — docstrings already name "attachment queries", but they deliberately skip modifiers (hit-zone authority: `hit_zones.rs:1123-1143`).
- Modified samplers (`sample_clip_looped_modified` etc., `anim/mod.rs:133,167,192`) run the stack but return skinning matrices; the post-modifier world pose is discarded inside `compose_palette`. **No modifier-applied world sampler exists** — the plan's Task 3 fills this. `anim/mod.rs` is only 275 lines post-split; clean extension point.
- Modifier stack shapes: `JointMask` (4×u64 bitset), `PoseModifier` enum (concrete, not trait — `pose_modifier.rs:69-97`), `ModifierEntry`, `PoseModifierStack`; `joint_model_transform` walk at `pose_modifier.rs:435`.
- External-input seam: `PoseInputs` POD (`crates/foundation/src/pose.rs:39`), carried as `MeshComponent.pose_inputs` `#[serde(skip)]`, written by `update_pose_inputs` (`crates/postretro/src/sim/mod.rs:484`), packed per instance by the collector (`mesh_render.rs:321`). Pose-modified models opt out of time-slicing via `force_resample_model` (`mesh_render.rs:238,305`).

## Extras tagging precedent (the pattern sockets mirror)

- Per-node extras, not document-level: `JointZoneExtras { hitZone, hitZoneRadius }` / `read_joint_zone` (`gltf_loader.rs:571-595`); pose masks + leg tags in `read_pose_masks` (`:650,683-745`).
- Raw reads happen in skin-joint order; `build_skeleton` (`:1294`) reindexes through `skin_joint_to_topo` (`:1427`; `joint_zones` remap `:1463-1466`). Any new per-joint tag must go through this remap.
- Joints carry no names (`Skeleton::joints` index-addressed, parent-before-child invariant, `skeleton.rs:74-89`) — socket identity must be the tag string.
- Degradation contract: extras never hard-fail a load; warnings only (`gltf_loader.rs:742-767`).
- Game-side retention: `ModelHitZones` (`hit_zones.rs:54,68`) keyed by `ModelHandle` holds `joint_zones`, `legs`; posed world joints via `pose_world_joints` (`hit_zones.rs:1130`).

## Extraction site and transforms

- Frame extraction: `resolve_pending_animation_stamps` → `collect_with_hit_zones` → `renderer.set_mesh_draws` (`crates/postretro/src/main.rs:2631-2662`).
- Collector builds each instance transform from the **interpolated** transform + `origin_offset` (`mesh_render.rs:312-325`; `interpolated_transform` at `registry.rs:1069`). Hit zones, by contrast, use game-tick position+yaw only (M10 design decision).
- Culling is positional per instance (`postretro_render_cpu::mesh_pass::mesh_visible`, current-tick position, `mesh_render.rs:261`) — attachments inherit the holder's `forward_visible` by design (spec sketch).
- Shadow retention for non-forward instances: `mesh_render.rs:271-273,354-367`.

## Entity/component surface

- `MeshComponent { model: String, animation, origin_offset, pose_inputs }` (`crates/entities/src/components/mesh.rs:28-39`), stored boxed as `ComponentValue::Mesh` (`registry.rs:206`), `ComponentKind::Mesh = 9`.
- No scene-graph/parenting exists; nearest entity-ref precedents are `ParticleState.emitter: Option<EntityId>` and `BrainComponent.acquired_target`. The plan avoids entity-refs entirely (attachments are model handles on the holder).
- Spawn paths: `prop_mesh` classname handler (`builtins/prop_mesh.rs`, `MeshComponent::stateless`) and descriptor materialization (`builtins/data_archetype.rs`, net path `netcode/remote_materialize.rs` — presentation-only invariant, no Brain/Agent/Health client-side).
- Level-load model sweep: `distinct_mesh_models` (`main.rs:198`) collects distinct `MeshComponent.model` handles, caller uploads once each; clip resolution `resolve_mesh_entity_clips` (`main.rs:226`) is the warn-and-degrade load-time-resolution precedent; spawner-only archetypes have descriptor pre-upload sets (test `spawner_only_archetype_is_preuploaded_for_both_roles...`, `main.rs:7622`).

## Descriptor/authoring surface

- `EntityTypeDescriptor` → `components.mesh: MeshDescriptor { model, animations, defaultState, locomotion }` (`sdk/types/postretro.d.ts:52-77`); typedefs generated (`gen-script-types`) with a drift test.
- Map-keyed descriptor precedent: `HealthDescriptor.zoneMultipliers` (`crates/foundation/src/data_descriptors/types/combat.rs:161`, finite ≥0 validation `:211-219`).
- FFI twins: `mesh_descriptor_from_js` (`crates/scripting-core/src/data_descriptors/js/entity.rs`) + Luau twin; shared validation `MeshDescriptor::build` (`crates/entities/src/data_descriptors/types/entity.rs:30-223`).
- "Model ships the spatial tag, script ships the balance" exemplar: `content/dev/scripts/target-dummy.ts:34-58`.
- `weapon-model.md` reserves `mount: { point: MountPoint, mesh: MeshId }` on `defineAugment` (`:193-204`) and the "one modifier system, not two" invariant (`:71`); visible-mount augments are an explicitly deferred open seam (`:485`). Sockets supply the joint half of that shape.
- Asset conventions (`resource_management.md` §7): authored at final meters, no import normalization, one mesh node per model, origin-between-feet pivot — so a prop is a separate model authored with its origin at the grip point.

## Oversized files flagged

| File | Lines | Disposition in plan |
|---|---|---|
| `crates/postretro/src/main.rs` | 8,006 | touched only at existing sweep/resolve seams |
| `crates/model/src/gltf_loader.rs` | 4,826 | split-before-extend (Task 1: extras module) |
| `crates/renderer/src/render/mesh_pass.rs` | 3,733 | untouched (rigid path complete) |
| `crates/postretro/src/scripting/systems/hit_zones.rs` | 3,296 | small delta (socket table retention) |
| `crates/postretro/src/scripting/systems/mesh_render.rs` | 1,576 | small delta; new logic in new `attachments` module |
