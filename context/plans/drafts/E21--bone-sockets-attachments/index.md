# E21 — Bone Sockets + Attachments

## Goal

An engine relation renders a rigid mesh at another entity's posed skeleton joint: attachment points are glTF `extras`-tagged joints ("sockets"), and a descriptor declares which prop model mounts at which socket. Demoable single-player on an enemy holding a prop; the seam the co-op avatar plan's third-person weapon and the weapon-model `mount: { point, mesh }` augment surface consume later. One socket system, not two.

## Scope

### In scope

- A per-node glTF `extras` key (`socket: "<name>"`) tagging a named attachment point. On skinned models the tag sits on a skin joint, loaded and topo-remapped exactly like hit-zone tags. On rigid (skinless) models the tag sits on any node and resolves to that node's model-space rest transform — so rigid weapon/prop models can carry sockets too (the E16 `mount` consumers: scope on an optic rail, barrel on a muzzle).
- Descriptor authoring: `components.mesh.attachments` maps socket name → prop model path (both script runtimes), validated at manifest load, resolved against the loaded model at level load with warn-and-skip degradation.
- Render relation: for each attachment on a visible mesh entity, emit one additional rigid mesh instance whose transform = holder's interpolated entity transform × posed joint matrix. Reuses the existing single-bone rigid path; no GPU/shader changes.
- Attachments follow the *rendered* pose: a new modifier-applied world-pose sampler, so a prop in an aim-bent hand tracks the bend.
- Attachment model handles join the existing level-load model sweep and descriptor pre-upload sets.
- Remote entities: attachments materialize through the existing descriptor-mesh presentation path — no wire changes.
- Behavior-preserving split of `extras` parsing out of `gltf_loader.rs` before extending it.
- Demo content: a dev enemy model gains a hand socket tag and holds a prop model.

### Out of scope

- Runtime attach/detach script API (set/clear at equip, pickup, weapon switch) — the co-op avatar + weapon presentation plan adds it when it consumes replicated weapon identity.
- Entity-as-attachment (a live entity — e.g. the pawn-sibling weapon entity — rendered at another entity's joint). This plan renders model handles, not entities; the relation can grow that later without changing the socket vocabulary.
- Per-attachment local offset/rotation. The prop's own authored origin is the grip point; the socket joint poses the prop. Art fixes placement in the prop or the socket joint, not in data.
- Weapon augment `mount` descriptor surface (`SlotKind`, `MountPoint`) — Epic 16; it will reference sockets by name.
- Hand IK onto the attached prop (roadmap non-goal: the socket poses the weapon, not the reverse).
- Attached props casting into hit-zone raycasts, collision, or any gameplay query — attachments are presentation only.
- Animated (skinned, multi-joint) attachments. Props render at rest pose via the identity-palette rigid path.

## Acceptance criteria

- [ ] A skinned glTF whose node `extras` carries `socket: "<name>"` on a skin joint loads with that socket name resolved to the correct joint after topo reordering; a socket on a non-joint node of a skinned model warns and is ignored. Malformed `extras` and duplicate socket names warn and never fail the load (duplicate: first winner in traversal order, deterministic).
- [ ] A rigid (skinless) glTF whose node `extras` carries `socket: "<name>"` on any node loads with that socket resolved to the node's model-space rest transform (node TRS composed through the node hierarchy).
- [ ] A `defineEntity` mesh block declaring `attachments: { "<socket>": "<model path>" }` passes manifest validation in both script runtimes; an empty model path or non-object shape is a descriptor validation error at manifest load.
- [ ] An attachment naming a socket the holder's model does not carry warns once at level load and is skipped; the entity still spawns and its own mesh still renders.
- [ ] In a dev map, an animated enemy authored with a hand-socket attachment renders the prop in its hand, and the prop tracks the hand through clip playback, crossfades, and pose-modifier output (aim bend moves the prop with the spine/hand).
- [ ] A stateless (no-animation) skinned mesh entity with an attachment renders the prop at the rest-pose joint; a rigid holder with an attachment renders the prop at the socket node's static rest transform with no pose sampling.
- [ ] The attachment renders exactly when its holder renders: culled together, present in shadow-caster retention together.
- [ ] On a connected client, a replicated enemy whose descriptor declares attachments shows the prop, with no new replicated component or wire field.
- [ ] Attachment model handles are uploaded exactly once by the level-load sweep; an attachment referencing a missing/failed model warns and renders nothing, and the holder is unaffected.
- [ ] Hit-zone raycast results are byte-identical before and after this plan (authority still samples the unmodified pose).
- [ ] The `extras`-parsing split is behavior-preserving: full existing test suite passes with no test edits beyond import paths.

## Tasks

### Task 1: Split extras parsing out of the glTF loader

Behavior-preserving refactor: move the per-node and document-level `extras` reading out of `crates/model/src/gltf_loader.rs` (4,826 lines) into a sibling module (e.g. `crates/model/src/gltf_extras.rs`) — `ModelExtras`/`read_model_tags`, `JointZone`/`JointZoneExtras`/`read_joint_zone`, and the `read_pose_masks` family with its serde shapes. `gltf_loader.rs` keeps orchestration (`build_skeleton`, the `skin_joint_to_topo` remap, `LoadedModel` assembly) and re-exports the moved types so external callers (`hit_zones.rs`, tests) need at most import-path updates. No behavior change; the full `postretro-model` test suite passes unchanged.

### Task 2: Socket extras tag → `LoadedModel.sockets` + game-side retention

Add a per-node `extras` key `socket` (string) parsed beside `hitZone` in the module Task 1 created. Surface it on `LoadedModel` as a socket table whose entries are one of two kinds: **skinned** — socket name → topo joint index, built inside `build_skeleton` through the same `skin_joint_to_topo` remap the `joint_zones`/`pose_masks` reads use (a raw skin-joint-order read reindexed to topo order); **rigid** — for a model with no skin, socket name → the tagged node's model-space rest transform, computed by composing the node's TRS through the glTF node hierarchy (rigid models load with one synthetic identity joint and no skin joints, so tags must resolve on plain nodes or rigid weapon/prop models could never carry sockets). Degradation contract matches the existing extras family: malformed value warns and skips; a duplicate socket name warns and keeps the first winner in a deterministic traversal order; on a *skinned* model a socket on a non-joint node is ignored with a warning. Retain the table game-side on the per-model store in `crates/postretro/src/scripting/systems/hit_zones.rs` (`ModelHitZones`), keyed by `ModelHandle` like `joint_zones`/`legs`, so game-side systems can resolve a socket without reloading the glTF. Unit tests cover the remap (a socket tagged on a node whose skin index ≠ topo index resolves to the right joint), the rigid-node rest-transform composition (a socket on a child node inherits its parent chain's TRS), duplicate handling, and malformed degradation.

### Task 3: Modifier-applied world-pose samplers

Add modified world-pose samplers to `crates/model/src/anim/mod.rs`, factored exactly like the existing `sample_clip_looped_modified` / `sample_blended_modified` / `sample_rest_pose_modified` trio but composing with `compose_world_pose` instead of `compose_palette`: materialize the `LocalTrs` buffer, run `apply_pose_modifier_stack` with the caller's `PoseInputs`, then the forward sweep — returning per-joint model-space matrices (pre-inverse-bind). Callers pass the same stack/inputs the palette samplers take, so a socket read and the rendered palette derive from one pose. The unmodified `sample_*_world` pair stays untouched — hit-zone authority keeps reading the unmodified pose, and a regression test pins that hit-zone raycasts through `pose_world_joints` are unchanged. Unit test: with an aim-bend stack and nonzero `aim_pitch`, the modified world matrix of a masked joint differs from the unmodified one and equals the transform obtained by composing the modified locals manually.

### Task 4: Descriptor surface + component field + load-time resolution

Author surface: `MeshDescriptor` gains `attachments?: { [socket: string]: string }` (socket name → content-relative model path). Wire it through both FFI parsers (`entity_type_descriptor_from_js` / `mesh_descriptor_from_js` in `crates/scripting-core/src/data_descriptors/js/entity.rs` and the Luau twin), the shared Rust `MeshDescriptor` + `build()` validation in `crates/entities/src/data_descriptors/types/entity.rs` (non-empty socket names and model paths; `DescriptorError::InvalidShape` on violations — the `zoneMultipliers` map on `HealthDescriptor` is the shape precedent), and the generated typedefs (`gen-script-types` regen for `.d.ts` / `.d.luau`, keeping the drift test green). Component side: `MeshComponent` (`crates/entities/src/components/mesh.rs`) gains an `attachments` list — per entry: socket name, model path, and a load-resolved `Option` joint index (mirroring `AnimationState.clip_index`). Spawn materialization copies descriptor attachments onto the component (data-archetype path; `prop_mesh` gains no KVP — descriptor-only authoring). Load-time resolution mirrors `resolve_mesh_entity_clips` in `crates/postretro/src/main.rs`: after the model sweep, fill each attachment's joint index from the Task 2 socket table; unknown socket warns once and leaves the index unresolved (entry skipped at render). Extend the model sweep (`distinct_mesh_models`) and the descriptor pre-upload sets to include attachment model handles so props upload exactly once at level install, including spawner-only archetypes.

### Task 5: Collector emission — render the attachment

Extend the mesh render collector (`crates/postretro/src/scripting/systems/mesh_render.rs`, `collect_inner`) to emit one extra `MeshInstanceInput` per resolved attachment on each collected mesh entity. New logic lives in a new sibling module (e.g. `scripting/systems/attachments.rs`) that the collector calls, keeping `mesh_render.rs` growth minimal: for a skinned holder, resolve the current animation sample (same stamps/anim-time the instance uses), sample the modifier-applied world pose via Task 3's samplers (falling back to rest pose for stateless holders), and take the socket joint's matrix; for a rigid holder, read the socket's static rest transform straight from the Task 2 table — no pose sampling. Either way, compose `holder_interpolated_transform (position + origin_offset, rotation, scale) × socket_matrix` as the attachment instance transform. The attachment inherits the holder's visibility verbatim — same `forward_visible`, same shadow-retention decision — no second cell lookup. Attachment instances carry no `pose_inputs` and no animation sample (rigid identity-palette path); the per-frame world-pose sample runs only for visible attachment-bearing holders. Entities with unresolved joint indices or missing models emit nothing. Verify the remote path: a materialized remote enemy's descriptor mesh carries its attachments and renders them through this same collector (test mirrors `remote_materialize.rs`'s presentation-only assertions). Draw-list-level tests assert instance count, transform composition against a hand-computed pose, and cull inheritance.

### Task 6: Demo content + end-to-end verification

Tag a hand joint with `socket: "hand_r"` in a dev enemy model's glTF (`extras` are JSON — edit in place; pick a model already carrying hit-zone tags), add or reuse a small rigid prop model under `content/dev/models/`, and extend a dev enemy descriptor (e.g. the anim-demo grunt) with `attachments: { hand_r: "<prop path>" }`. Verify in the movement-feel/anim fixture map: the enemy holds the prop through idle/walk/attack clips and aim bend. Capture the result with the E20 frame-capture path for the record. This task is the integration gate: it runs only content edits plus verification, no engine code.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split blocks the loader extension.
**Phase 2 (concurrent):** Task 2, Task 3 — independent (loader tag vs. anim samplers).
**Phase 3 (sequential):** Task 4 — consumes Task 2's socket table for load-time resolution.
**Phase 4 (sequential):** Task 5 — consumes Task 3's samplers and Task 4's component field.
**Phase 5 (sequential):** Task 6 — content + verification over the finished path.

## Rough sketch

- **Socket = tag string → joint index (skinned) or rest matrix (rigid).** Joints carry no names (`Skeleton::joints` is index-addressed, parent-before-child), so the socket *name* is the `extras` tag, resolved at load — the `JointZone` pattern verbatim for skinned models, including the `skin_joint_to_topo` remap inside `build_skeleton`. The rigid branch exists because rigid models have no skin joints at all (one synthetic identity joint), yet E16's `mount` consumers put sockets on rigid weapon models (scope on optic rail, barrel on muzzle). Widening the vocabulary now is cheap; retrofitting the skin-joints-only contract after E16 opens would reopen the loader and resolution semantics. It also pre-defuses future nested composition (scope on a weapon in a hand): a rigid attachment's own sockets are static matrices, so that later collector extension is one more matrix multiply, not a new system.
- **Rigid path already exists.** Every non-skinned model gets one identity joint (`SkinnedVertex::rigid`, joint 0); placement is carried entirely by the instance transform (`build_instance_entry`), palette entry is identity. An attachment is just another `MeshInstanceInput` with a computed transform — `mesh_pass.rs` and the shaders are untouched.
- **Modified pose is the load-bearing new primitive.** Today's world-pose samplers (`sample_clip_looped_world` / `sample_blended_world`) deliberately skip the modifier stack (hit-zone authority). The renderer's palette samplers apply modifiers but discard the world pose inside `compose_palette`. Task 3 fills the gap with `sample_*_world_modified` over `compose_world_pose` — without it, a prop in an aim-bent hand visibly detaches from the body.
- **Transform composition** matches the body instance exactly: the holder instance uses `Mat4::from_scale_rotation_translation(scale, rotation, position + origin_offset)`; the attachment right-multiplies the posed joint matrix onto that same matrix. Interpolated transform (render presentation), not the game-tick transform hit zones use.
- **Cull inheritance** is a decision, not an accident: culling is positional (`locate_cell`), so an independently-located long prop could straddle a cell boundary and pop separately from its holder. Inheriting the holder's `forward_visible` avoids the second locator descent and the pop.
- **Cost shape:** one modifier-applied world-pose sample per visible attachment-bearing holder per frame, game-side. Current scale (a handful of enemies) is far below concern; if wave-scale profiling ever bites, the sample can share the renderer's palette resample gate.
- **Oversized-file watch:** `gltf_loader.rs` 4,826 (split: Task 1), `hit_zones.rs` 3,296 and `mesh_render.rs` 1,576 (both extended by small deltas only; new logic goes to the new `attachments` module), `main.rs` 8,006 (touched only at the existing sweep/resolve seams).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | glTF extras |
|---|---|---|---|---|---|
| socket tag (per-node) | socket table on `LoadedModel` | n/a (load-time only) | n/a | n/a | `socket` (string) |
| attachments map | `attachments` on `MeshDescriptor` / `MeshComponent` | `"attachments"` | `attachments` | `attachments` | n/a |
| map entry | socket name → model path | object: socket name key, path value | `{ [socket: string]: string }` | table `{ [socket] = path }` | n/a |

No PRL section, no netcode wire change — attachments are descriptor data materialized locally on both host and client.

## Script syntax examples

```ts
// Proposed design
export const grunt = defineEntity({
  canonicalName: "anim_demo_grunt",
  components: {
    mesh: {
      model: "models/rodin_sci-fi_trooper_m/scene.gltf",
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk", loop: true },
      },
      defaultState: "idle",
      // Model ships the socket tag (glTF extras `socket: "hand_r"` on the
      // right-hand joint); the descriptor ships what mounts there.
      attachments: {
        hand_r: "models/props/pipe_wrench/scene.gltf",
      },
    },
    health: { max: 50, zoneMultipliers: { head: 2.0 } },
  },
});
```
