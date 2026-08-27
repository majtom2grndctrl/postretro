# Kinematic Assemblies & Instanced Groups — research

> Source-grounded current-state map for the epic hub (`index.md`). Findings, not decisions. Every identifier here was read from current source this session; line numbers drift on the next edit — cite by symbol when it matters.

## 1. How grouped brushes are handled today

**Adapter predicate.** `crates/level-compiler/src/format/quake_map.rs` — `is_editor_group_classname(classname)` returns true only for `"func_group"` (its doc comment: editor groups are "authoring containers, not runtime entities: their brushes should participate in the static world exactly as if they had remained under `worldspawn`").

**Flatten call site.** `crates/level-compiler/src/parse.rs`, inside the single entity-walk loop of `parse_map_file` (`for entity_id in geo_map.entities.iter()`): a `func_group` entity's `brush_ids` are appended to `world_brush_ids` and the loop `continue`s. `world_brush_ids` is seeded with worldspawn's brushes before the loop and consumed once into `MapData.brush_volumes` (built via `build_brush_volumes_with_ids`). Grouped brushes are therefore indistinguishable from worldspawn geometry from BSP onward. The same `world_brush_ids.extend(...)` pattern is reused for `switch` desugaring.

**`_tb_*` metadata is discarded.** `is_runtime_map_entity_key(key)` = `!RESERVED_MAP_ENTITY_KEYS.contains(key) && !key.starts_with("_tb_")`. Applied only when building a point entity's generic `key_values` bag. Consequences:
- `func_group` entities never build a `key_values` bag — flattened/`continue`d earlier — so their `_tb_id`/`_tb_name`/`_tb_linked_group_id`/`_tb_transformation` are read by nothing.
- A grouped **point entity** (e.g. `prop_mesh` carrying `_tb_group "1"`) becomes a `MapEntityRecord` with `_tb_group` stripped — membership lost.
- A grouped **light** is worse: lights route through the light-translation path into `MapLight` (typed fields only, no `key_values` bag), so there is nowhere for `_tb_group` to live even if it survived the strip. A grouped light becomes a plain top-level `MapLight` at its authored world origin, with no back-reference to its group.

Regression tests lock the current contract: `trenchbroom_func_group_brushes_are_flattened_into_static_world`, `empty_trenchbroom_func_group_is_not_a_runtime_entity`, `map_entity_key_values_strip_trenchbroom_metadata` (all in `parse.rs`).

**FGD.** `sdk/TrenchBroom/postretro.fgd` does **not** declare `func_group` — it is a TrenchBroom built-in classname emitted on save. No FGD change is needed to *recognize* groups; only to author new explicit-binding KVPs.

## 2. The on-disk `.map` format for groups (from repo fixtures)

Fixtures with group markup: `content/dev/maps/{campaign-test,kinematic-platform,movement-feel}.map`. No `_tb_layer` appears in any repo `.map` — only groups are exercised.

Two nesting forms TrenchBroom emits:

1. **Brushes nested in the group entity.** One `func_group` entity whose `{ }` braces directly contain the brushes:
   ```
   { "classname" "func_group" "_tb_type" "_tb_group" "_tb_name" "dome_ceiling"
     "_tb_id" "8" "_tb_linked_group_id" "{e18d05b4-...}"
     "_tb_transformation" "1 0 0 0 0 1 0 0 0 0 1 -2000 0 0 0 1"
     { <brush 0> } { <brush 1> } }
   ```
2. **Sibling entities back-referencing the group id.** The group entity carries no brushes; member point entities live as top-level siblings, each with `_tb_group "<id>"`:
   ```
   { "classname" "func_group" "_tb_name" "Phase Lights" "_tb_id" "2" ... }
   { "classname" "light" "origin" "-1800 1896 188" ... "_tb_group" "2" }
   ```

**Linked groups (instances).** `_tb_linked_group_id` is a GUID (with or without braces on disk); groups sharing one GUID are linked copies. `_tb_transformation` is a 16-value row-major 4×4 affine (translations and rotations both seen). **Critical:** TrenchBroom applies `_tb_transformation` to each copy's brush coordinates *on save* — on disk every instance is fully-expanded concrete geometry. The GUID + matrix are recorded only so the editor can re-sync; the compiler sees concrete duplicated brushes, not references. Reconstructing a shared template requires recognizing the GUID and re-deriving the template, not reading a reference.

**No true linked-group fixture exists.** The nearest — two `func_group`s both named `pink_ambient_lights` in `kinematic-platform.map` — carry *distinct* GUIDs, i.e. unlinked copies. An instancing spec must author a real shared-GUID fixture.

## 3. What the kinematic substrate already does (do not re-spec)

- **Multi-brush movers: exist.** `MapKinematicMover` (`crates/level-compiler/src/map_data.rs`) already carries `brush_volumes: Vec<BrushVolume>`. `encode_kinematic_geometry_section` (`kinematic_geometry.rs`) flattens all brushes' sides into one list and `extract_kinematic_mover_geometry` (`geometry.rs`) triangulates them into one origin-local vertex/index blob — `brush_index: 0` hard-coded for every face, no per-sub-brush identity preserved. So "N brushes move as one mover" is shipped; a kinematic *group of brushes* reuses this, it is not new geometry work.
- **Mover motion runtime: exists.** `KinematicMoverComponent` (`crates/entities/src/components/kinematic_mover.rs`) stores motion state only (waypoints, speed, spin, blocking, events, live phase) — **no geometry**. Geometry stays in the loaded PRL record. `run_kinematic_mover_tick` advances phase and writes the mover entity's `Transform`; the renderer reads the interpolated `Transform` to place the baked-local geometry. One entity per mover — `spawn_loaded_kinematic_movers` does exactly one `registry.try_spawn` per mover and attaches one component. **No child/member entities are spawned per mover.**
- **PRL sections:** `KinematicGeometry` id 43 (version 4), `TriggerVolumes` id 44, `MapEntity` id 29. Section versions are exact-match epochs; adding fields is a version bump with a loader migration (the id-43 loader already carries v1–v4 back-compat).

## 4. Mover-carried lights: net-new

`index.md §Agent Router` line "mover-attached dynamic lights" is **misleading** — those `rendering_pipeline.md` sections describe only how a mover *receives* lighting (baked direct-SH, promoted-static shadow slots), not a light parented to a mover.

The one "follow" primitive that exists: `LightComponent.follow_transform: bool` (`crates/entities/src/components/light.rs`). Runtime-only, set `true` at exactly two sites (both projectile spawn: `weapon_stage/commands.rs`, `netcode/projectile_presentation.rs`). `follow_transform_position` (`scripting/systems/light_bridge.rs`) reads the pose from the **same entity's** `SpriteVisual`/`Mesh`/`Transform`. `LightComponent` carries no `mover_id` and no parent reference. There is no FGD field, no KVP, and no compiler path linking a light to a mover.

So a mover-carried authored light needs, net-new: (a) an authoring binding (explicit reference and/or func_group membership), (b) a compiler path that peels the light off with its mover linkage and local offset instead of emitting a top-level `MapLight`, (c) a runtime parent→child transform composition updating the light's world pose each tick after the mover tick and before render.

## 5. Render instancing: only billboards truly instance

- **Movers draw one transform per draw.** `record_draws` in `renderer/src/render/kinematic_brush.rs` issues `draw_indexed(range, 0, instance_index..instance_index + 1)` per mover per material range. There is a shared `instance_buffer` of per-mover transforms, but it indexes one transform per draw — not an instanced multi-draw. The rigid shadow-occluder path (`rigid_occluder_depth.rs`) is the same.
- **Static world** draws per-cell/per-bucket via `multi_draw_indexed_indirect` (instance-count-1 indirect draws keyed by leaf).
- **Only particle billboards** issue a real `instance_count = N` draw from one shared quad (`scripting.md §11`). The mesh/`MeshComponent` path is described as "instance-friendly" (roadmap Epic 10) but is a planned per-instance-transform+palette-index shape, not a shipped multi-transform-per-draw instancer.

**Implication:** kinematic-tier instancing (one template geometry drawn at N transforms in one draw) is a **net-new render path**. Reusing the existing "one geometry blob + one transform per mover" model gives correctness with N× the geometry, no dedup — the fallback if the instanced draw path is cut.

## 6. Roadmap gates this epic touches

- **E17-F (Visibility-bearing moving world)** — owns "doors-as-occluders, dynamic portals, sector-graph updates, kinematic clusters/sub-worlds, and any chunk-primitive consolidation." Deferred: "Start only for a concrete set-piece or measured performance need." `E17--doors-as-occluders` is in-progress. This epic opens the *non-visibility-bearing* slice of "kinematic clusters" (assemblies that move but don't rewrite the portal graph — exactly what movers already do); it must not build dynamic portals or sector-graph updates, which stay E17-F.
- **Chunk Primitive (Future/Infra)** — "unify static world geometry, kinematic clusters, and dynamic debris into one record type (mesh + collider + transform + sector membership). Deferred until two or more of those consumers exist." An instanced-template record is chunk-primitive-shaped; the instancing spec forces the "converge or defer" decision.
- **Sector Graph (Future/Infra)** — "Prerequisite for kinematic clusters that need their own sector graphs." Assemblies in this epic do **not** carry internal visibility, so they do not need this — that is the scope line that keeps sector-graph out.
- **E10 render foundation** is "instance-friendly" (per-instance transform + shared buffer, GPU-driven indirect draw) — the substrate the instancing spec builds on.
- **E21 bone sockets + attachments** (`context/plans/done/E21--bone-sockets-attachments/index.md`, shipped) — the parent→child-follows-transform relation, and the strongest existing mechanism for a mover-carried member. Verified facts:
  - Render relation: one extra rigid mesh instance whose `transform = holder's interpolated entity transform × posed joint matrix`; reuses the single-bone rigid path, no GPU/shader changes.
  - Attachments follow the *rendered* pose (a modifier-applied world-pose sampler), inherit the holder's visibility verbatim (same `forward_visible`, same shadow retention, no second cell lookup), and re-derive on the client with no new wire (the resolved binding is `#[serde(skip)]`-transient, re-resolved at load; the remote path resolves through the same collector).
  - Binding is a kind-explicit enum: `SkinnedJoint(topo idx)` (sample a posed joint) vs. `RigidRest(Mat4)` (a static matrix, no pose sample). A mover has no skeleton, so a mover-carried member is the `RigidRest(Mat4)` case with the mover's `Transform` as the holder pose.
  - **Deliberately no per-attachment data offset** (out-of-scope, index line 25): "The prop's own authored origin is the grip point; the socket joint poses the prop. Art fixes placement in the prop or the socket joint, not in data." A mount needing an offset rides a child node whose local TRS *is* the offset — i.e. the offset lives in geometry, not authored KVP data.
  - **"Entity-as-attachment" explicitly reserved** (out-of-scope, index line 24): "This plan renders model handles, not entities; the relation can grow that later without changing the socket vocabulary." A carried *light* (a live component/entity, not a model handle) is exactly that reserved growth.
  - Offset derivation for a light (no mesh origin to encode the mount): the light's authored world position expressed in the mover's local frame is the offset — the same origin-subtraction `extract_kinematic_mover_geometry` already applies to mover brush geometry (§3), keeping the offset out of authored data, consistent with E21's stance.
- **canonicalName rename (Future/Infra)** — source formats translate their identifier to a canonical name at compile time; adapter-side recognition of func_group aligns with this direction (grouping is source vocabulary translated at the boundary).

## 7. Observers × lifecycle (for the carried-member consumer)

The carried dynamic light is observed from more than one vantage; the specs owe each, not just the flow:

| Vantage | Concern |
|---|---|
| Single-player / host | Light world pose = mover pose ∘ authored local offset, recomputed each tick after `run_kinematic_mover_tick`, before render. |
| Connected client | Mover phase already replicates (E17-A) and the client re-derives the mover `Transform`; the carried light must derive from the *client's* mover Transform, adding no new wire traffic — a work-eliminating claim to warrant against the replication path, not assert. |
| Renderer (interpolated) | Render reads interpolated mover Transform; the light pose must use the same interpolated pose, or the light lags the geometry it lights. |
| Dynamic vs. baked light | Only dynamic-tier lights can move; a baked light carried by a mover is a contradiction (its contribution is baked static). The adapter/compiler must reject or warn on a baked light bound to a mover. |

## 8. Orderings (for the carried-member consumer)

| Scenario | Ordering | Expected |
|---|---|---|
| Mover blocked / reversed | Light follows mover through reversal and stop | Pose tracks at reversal and at rest, not only mid-travel. |
| Mover completes (`once`) | Light holds at terminus | No snap-back to authored origin. |
| Zero fixed ticks in a frame | Render interpolates; no mover tick ran | Light pose still matches interpolated mover pose. |
| Two fixed ticks in a frame | Two mover ticks before render | Light pose composed from final tick pose. |
| Member light bound to a missing/renamed mover | Binding unresolved at compile | Named compile warning + documented fallback (top-level light at authored origin), not a silent drop or a crash. |
