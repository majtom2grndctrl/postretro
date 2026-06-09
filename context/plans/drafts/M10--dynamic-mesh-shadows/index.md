# Dynamic Mesh Shadow Casting

> **Status:** draft. Milestone 10 render-foundation track. Follows *Mesh render pass + MeshComponent*.
> **Related:** `context/lib/rendering_pipeline.md` §4 (lighting), §7.1 (shadow passes), §9 (skinned model pipeline) · `shadow-cone-cull` (shipped: the per-slot cone-culled spot depth pass + `lighting/cone_frustum.rs` predicate this builds on) · `lighting--entity-direct-sh` (the SH-direct term that motivates this) · sibling `research.md` (code anchors + algorithm/feasibility citations).

## Goal

Animated entity meshes cast real-time shadows from runtime dynamic lights, grounding enemies that currently read as lit-but-floating. Entities already get crisp lit/dark contrast from baked SH-direct + SH-indirect; the missing piece is the shadow they throw onto the world. Adds a skinned depth-only variant of the mesh pass that renders entity occluders into the shadow pool — spot lights into the existing 2D-array pool, point lights into a new cube-array pool.

## Authoring model — static vs dynamic lights

Authors choose per light whether it is **static** (baked) or **dynamic** (runtime-direct — the `light_dynamic*` classes). For any shadow involving an entity, that choice is the deciding factor, because only a dynamic light has a separable runtime forward term that can be per-light shadowed without double-counting a baked lightmap:

- **Static (baked) light** — its only effect on an entity is the baked SH-direct term: a soft, probe-coarse, occlusion-tested approximation of world→entity shadow. It can neither cast a crisp shadow *from* an entity nor receive one *onto* an entity per-light; its contribution is folded into the all-lights lightmap with no runtime term to attenuate.
- **Dynamic light** — reach for one whenever you want a *crisp runtime* shadow involving an entity:
  - **entity → world** (an enemy's shadow on the floor) — **this feature**.
  - **world → entity** (static geometry's crisp shadow cast onto an enemy) — **deferred** to *Dynamic mesh direct lighting*. Entities are not lit by dynamic lights yet, so the mesh shader has no runtime per-light term to attenuate (group 2 unallocated).

Author's rule of thumb: a dynamic light is what grounds a moving entity in shadow; baked lights give only the soft SH-direct approximation. This feature ships the entity→world half today; the world→entity half follows when meshes gain a runtime light loop. This static/dynamic choice is orthogonal to the two-axis cleanup below (`shadow_type` × `casts_entity_shadows`), which governs *how* a light shadows once dynamic.

## Scope

### In scope
- A depth-only **skinned mesh pass** reusing the mesh pass's `skin_matrix` kernel, projecting by a per-slot/per-face light-space matrix passed as a parameter (so spot and cube-face renders share one pipeline).
- Hoist the bone-palette + per-instance buffer upload ahead of the shadow depth passes (they currently upload inside the mesh pass, which is encoded *after* the shadow passes).
- **Spot:** render entity occluders into each occupied spot-pool slot's depth layer, after the world geometry draw. World forward already samples the pool, so entity→world shadows appear with no forward-shader change for spots.
- **Point:** a new `Depth32Float` **cube-array** shadow pool; dynamic point lights render entity occluders into 6 faces; the world forward shader gains a direction-vector cube-sampling path with per-face bias.
- **Per-light caster culling:** an entity instance is drawn into a light's shadow map only if its bounds intersect the light's cone frustum (spot) or sphere / per-face frustum (point). Reuse the shipped `lighting/cone_frustum.rs` predicate (`cone_frustum_planes` + `aabb_intersects_frustum`) on the CPU — entities aren't in the BVH, so this is a per-instance CPU test, distinct from `shadow_cull`'s GPU world-geometry cull.
- **Tunable-radius PCF** on entity-shadow sampling, dialed to match the softened baked lightmap shadows. Single radius parameter.
- **Authoring two-axis cleanup:** retire the dead `cast_shadows` field; redefine `casts_entity_shadows` as the per-light "cast shadows from dynamic entities" toggle, valid only on runtime-direct (dynamic-tier) lights. Clean model: static-geo shadow technique (`shadow_type` = lightmap | sdf) × entity-shadow (`casts_entity_shadows` = on | off).

### Out of scope
- **Entities *receiving* shadows** — entity→entity and runtime world→entity. The skinned-mesh shader has no runtime per-light loop to attenuate (group 2 is unallocated). Deferred to *Dynamic mesh direct lighting*. Baked world→entity is already handled by occlusion-tested SH-direct, soft and probe-coarse.
- **Entity shadows from baked (`static_light_map`) or `sdf` lights.** Only `is_dynamic` lights cast — they alone have a separable runtime direct term in the forward loop to multiply by the shadow factor. A baked light's contribution is folded into the all-lights lightmap and can't be per-light attenuated.
- **World geometry in point-light cube faces.** Point cube faces hold entity occluders only; static geometry does not self-shadow under a dynamic point light in v1. (Spot slots keep their world depth from `shadow-cone-cull`.)
- **Cached-static / recompute-dynamic depth split.** An optimization for moving lights; lights are fixed today.
- **Moving-light authoring entity.** A future, separately-tuned entity type.
- Soft penumbra / variance-family filtering (VSM/ESM/MSM). Hard, PCF-softened edges only.

## Acceptance criteria
- [ ] A skinned enemy spawned under a runtime dynamic **spot** light casts a visible shadow onto the floor and walls in a dev map; the shadow moves with the animated pose (it is skinned depth, not a static blob).
- [ ] A skinned enemy under a runtime dynamic **point** light casts a shadow that is correct in every direction (cube-mapped, no missing quadrant, no hard seam across cube faces).
- [ ] An enemy outside a light's cone (spot) / influence sphere (point) is not drawn into that light's shadow map. Verified by a CPU-side submitted-instance counter, mirroring `shadow-cone-cull`'s counter pattern — no GPU readback.
- [ ] Entity occluders render only for lights with `casts_entity_shadows` (which the cleanup makes imply `is_dynamic`); baked-only, `static_light_map`, and `sdf` lights cast no entity shadow, and a dynamic light with the toggle off casts none either while still casting its world shadow. Verified by a unit test on the entity-occluder-eligibility predicate.
- [ ] Entity shadow edges are anti-aliased, not single-texel stair-stepped; the PCF radius is set by one parameter and changing it visibly softens/sharpens the edge.
- [ ] The `cast_shadows` field is removed everywhere: the PRL light record (`alpha_lights` wire + `pack`/`prl` codecs), the compile-time `MapLight` and bake branches, runtime light packing, and the scripting surface including `LightComponent.castShadows` in `postretro.d.ts`/`.d.luau`. No code, test, or SDK type references it.
- [ ] `casts_entity_shadows` set on a non-`is_dynamic` light is rejected at compile time (warning logged, value cleared); on a dynamic-tier light it toggles entity-shadow casting. Verified by level-compiler unit tests.
- [ ] When the count of shadow-casting dynamic point lights exceeds the cube-array capacity, the lowest-ranked are dropped gracefully (no panic, no validation error). Verified by a unit test on point-pool allocation.
- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test -p postretro`, and `cargo test -p postretro-level-compiler` pass.

## Tasks

### Task 1: Skinned depth-only pass + palette hoist + model bounds
Add a depth-only render pipeline that skins vertices with the mesh pass's `skin_matrix` and projects by a **light-space matrix supplied as a parameter** (group 0 = a per-render light-space uniform; the spot path can reuse the existing `shadow_vs_bind_group` + per-slot dynamic offset; group 3 = the palette + per-instance SSBO the mesh pass already owns). Position/joints/weights only — drop color attributes, mirroring `depth_prepass.wgsl`'s relationship to `forward.wgsl`. Build it cube-ready: nothing in the pipeline or shader may assume one slot per light or a 2D target.

Move pose sampling + palette/instance upload ahead of the shadow loop: today `MeshPass::render_frame` computes the frame plan (`plan_mesh_frame`), samples animation into the palette, `write_buffer`s palette+instances, *and* draws — all after the shadow passes. Split the plan+sample+upload into a step the renderer runs after `update_dynamic_light_slots` and before the spot depth loop; the mesh pass and the skinned-depth pass then both read the populated buffers (moving the `write_buffer` calls alone is insufficient — the data they write must be sampled first).

**Per-model bounds (new):** the cull in Tasks 2/4 needs a per-instance world bound, which does not exist today (`SkinnedMesh`/`UploadedModel` carry none; `PlannedInstance` has only `transform`). Compute a local-space bound (AABB or bounding sphere) per model at glTF load (from vertex positions, or the glTF accessor min/max), store it on the uploaded model, and expose it on `PlannedInstance` (or derive it as bound × `transform`) so the shadow culls have something to test.

### Task 2: Spot entity-shadow integration
Render entity occluders into each occupied spot-pool slot's depth pass, after the shipped `shadow_cull.draw_slot_indirect` world draw, within the same per-slot "Spot Shadow Depth Pass" render pass (same depth layer, no clear — `CompareFunction::Less` composites world + entity). Set the skinned-depth pipeline (Task 1) and entity instance draws after the world indirect draw in that pass. The slot's light-space matrix is the `slot_cone_matrices` entry already stashed on `SpotShadowPool` in `update_dynamic_light_slots`. Render entity occluders only for slots whose light has `casts_entity_shadows` (Task 3) — a dynamic light with the toggle off keeps its world shadow but draws no entities. Cull entity instances per slot on the CPU: transform each instance's model bound (Task 1) by its `transform` and test against the slot's cone frustum via `cone_frustum_planes` + `aabb_intersects_frustum`. Add tunable-radius PCF to `sample_spot_shadow`. World forward sampling is unchanged — verify entity shadows appear on world receivers for dynamic spots.

### Task 3: Two-axis authoring cleanup
Two parts; one task because both edit the shared light record and the FGD.

**(a) `casts_entity_shadows` semantics + FGD (this feature needs it).** Redefine `casts_entity_shadows` as "this light casts shadows from dynamic entities," valid only when `is_dynamic`. The level compiler warns and clears it on any non-`is_dynamic` light (`quake_map.rs`). Keep pool-*slot* eligibility on `is_dynamic` (a dynamic light still casts its world shadow with the toggle off); `casts_entity_shadows` gates only entity-occluder rendering (Task 2/4). Update the FGD (`sdk/TrenchBroom/postretro.fgd`): expose `_cast_entity_shadows` on the dynamic-light classes (default on), remove it from the baked `light`/`light_spot`/`light_sun` base; document the static-geo (`shadow_type` = lightmap | sdf) × entity-shadow (`casts_entity_shadows` = on | off) model in help text.

**(b) Retire the dead `cast_shadows` flag (cross-crate + SDK contract + PRL wire — fully enumerated).** The flag is hardcoded `true` for authored maps and read only at bake time. Removing it touches, in step:
- **PRL wire / format:** the light record in `crates/level-format/src/alpha_lights.rs`, packed in `crates/level-compiler/src/pack.rs`, decoded in `crates/postretro/src/prl.rs`. A breaking PRL-record change — see Wire format.
- **Compile-time:** the `MapLight` field + always-true assignment in `crates/level-compiler/src/format/quake_map.rs`; the dead `cast_shadows == false` occlusion-skip branches in `crates/level-compiler/src/lightmap_bake.rs` and `sh_bake.rs`.
- **Runtime:** `crates/postretro/src/lighting/mod.rs` (light packing copy-throughs that ignore it).
- **Scripting surface (primitive contract):** `crates/postretro/src/scripting/components/light.rs`, `primitives/light.rs`, `systems/light_bridge.rs`, `refresh_plan.rs`, `builtins/data_archetype.rs`, and the SDK types `sdk/types/postretro.d.ts` + `postretro.d.luau` (`LightComponent.castShadows`). Drop `castShadows` from the component shape and any validation/constructor that names it.

Land both parts behavior-neutral except the intended `casts_entity_shadows` redefinition; no authored map changes shadow behavior beyond the new toggle semantics.

### Task 4: Cube-array point-shadow pool
Add a renderer-owned `Depth32Float` cube-array depth pool (`TextureViewDimension::CubeArray`), capacity budgeted for the 4 GB floor GPU (≈24 MB per 1024² cube — cap the point-light count; 512² faces are the fallback). Query `DownlevelFlags::CUBE_ARRAY_TEXTURES` at init; absence disables point shadows (spot path unaffected). Rank dynamic point lights into cube slots (analogous to `rank_lights`, scored by influence; lowest-ranked dropped past capacity). For each cube slot, render entity occluders into all 6 faces using per-face 90° frustum culling (reuse the AABB-vs-frustum predicate). Skinned-depth pass (Task 1) renders each face with that face's light-space matrix.

### Task 5: Point/cube forward sampling
Add a `texture_depth_cube_array` binding + comparison sampler to the forward shader (+1 sampled texture → 14/16 on Metal). In the per-light loop's point-light case, sample the cube pool with the light→fragment direction vector via `textureSampleCompareLevel`, applying tunable PCF in an orthonormal basis around the direction (Bevy's pattern) and a per-face linear-distance bias tuned separately from the spot bias. Multiply the point light's attenuation by the shadow factor. Gate on the point light owning a cube slot (sentinel for "no slot").

## Sequencing

**Phase 1 (sequential):** Task 1 — the skinned depth-only pass + palette hoist; every later task consumes it.
**Phase 2 (concurrent):** Task 2 (spot integration — the critical north star) ‖ Task 3 (authoring cleanup — disjoint files: FGD, level-compiler, `prl.rs`).
**Phase 3 (sequential):** Task 4 — cube-array pool + 6-face rendering; consumes Task 1's parameterized projection.
**Phase 4 (sequential):** Task 5 — forward cube sampling; consumes Task 4's pool.

Spot shadows (north star: enemy casts a shadow) land at end of Phase 2. Point work (Phases 3–4) is an additive, cuttable tail.

## Rough sketch

**Depth-reusable pass.** `skinned_mesh.wgsl:197 skin_matrix(joints, weights, base)` is the shared kernel; the depth variant calls it then projects by the parameter matrix — no color, no SH, no material group. Precedent: `depth_prepass.wgsl` (vertex-only, group 0 camera) and `spot_shadow.wgsl` (light-space projection of position). The new shader is their union plus skinning.

**Spot composition (no double-count).** A dynamic spot's direct term lives only in the forward light loop (`forward.wgsl:913`+), never baked. Its shadow factor (`sample_spot_shadow`, `forward.wgsl:413`) multiplies that term alone. Adding entity depth to the slot changes the slot's depth *contents*, not which term consumes it — the invariant holds by construction. Entity-only is impossible to double-count because the light isn't in any lightmap.

**Two gates, kept separate.** Pool-*slot* eligibility (does this light get a shadow map for its **world** shadow) stays `is_dynamic`, unchanged — `rank_lights` for spots, a sibling ranker for points. Entity-*occluder* rendering into that map is the new gate: `casts_entity_shadows` (Task 3 makes it imply `is_dynamic`). So a dynamic light with the toggle off still casts a world shadow but no entity shadow. Spot uses `LightType::Spot` + the existing pool; point uses `LightType::Point` + the cube pool.

**Cube pool (point).** Depth cube-array comparison sampling is supported on Metal/DX12/Vulkan via wgpu (Bevy ships it); WebGL is the only gap and not a target. Option A (dedicated cube-array) over packed-faces-in-2D-array: hardware seamless filtering, trivial direction-vector sampling, +1 binding. Render to one face = a 2D-array render view at `baseArrayLayer = slot*6 + face`. See `research.md` for citations.

**Palette ordering.** Today the spot depth passes (`render/mod.rs` ≈ "Spot Shadow Depth Pass") run before the mesh pass, but the mesh pass plans the frame, samples poses, and uploads palette+instances *after* them. Move plan+sample+upload to a step after `update_dynamic_light_slots` and before the shadow depth loop; the shadow passes and the mesh pass then read the same buffers (the upload alone can't move without its sampled data — see Task 1).

**Key modules.** Render: `crates/postretro/src/render/mesh_pass.rs`, `render/mesh_instances.rs`, `shaders/skinned_mesh.wgsl`, `lighting/spot_shadow.rs`, a new sibling cube pool in `lighting/`, `lighting/cone_frustum.rs` (reused), `shaders/forward.wgsl`, `render/mod.rs` (orchestration), `model/gltf_loader.rs` + `model/mesh.rs` (model bounds). Authoring (Task 3): `sdk/TrenchBroom/postretro.fgd`, `crates/level-compiler/src/format/quake_map.rs`, `map_data.rs`, `pack.rs`, `lightmap_bake.rs`, `sh_bake.rs`, `crates/level-format/src/alpha_lights.rs`, `crates/postretro/src/prl.rs`, `lighting/mod.rs`, the scripting light stack (`scripting/components/light.rs`, `primitives/light.rs`, `systems/light_bridge.rs`, `refresh_plan.rs`, `builtins/data_archetype.rs`), `sdk/types/postretro.d.ts` + `postretro.d.luau`.

## Boundary inventory

| Name | Rust | Wire / PRL | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| entity-shadow toggle | `MapLight.casts_entity_shadows: bool` | existing light-record field | `castsEntityShadows`* | `castsEntityShadows`* | `_cast_entity_shadows` (0/1) |
| static-geo technique | `MapLight.shadow_type: ShadowType` | existing | n/a | n/a | `_shadow_type` (`static_light_map`/`sdf`) |
| dead flag (removed) | `MapLight.cast_shadows` (gone) | **removed** from `alpha_lights` record | `LightComponent.castShadows` (removed) | `LightComponent.castShadows` (removed) | none (never authored) |

\*Semantics change only; not currently in the scripting `LightComponent` shape (only `castShadows` is). Whether the redefined toggle gains a scripting field is a Task 3 sub-decision — default: keep it map-authored (`_cast_entity_shadows`) only, no new SDK field. The first table row maps the FGD↔compile path; the SDK columns apply only if a script field is added.

## Wire format

Removing `cast_shadows` drops one field from the PRL light record. The record layout lives in `crates/level-format/src/alpha_lights.rs`, is packed in `crates/level-compiler/src/pack.rs`, and decoded in `crates/postretro/src/prl.rs` — update all three in step. This is a **breaking change to the light record**: existing compiled `.prl` files and `.prl-cache` entries must be rebuilt (`prl-build`); no in-place migration. `casts_entity_shadows` stays in the record (semantics change only, layout unchanged). No new section. The cube-array pool is a runtime GPU resource — no PRL data. Point-light entity-shadow eligibility derives from the existing `is_dynamic` + `LightType::Point` fields; no new wire surface.

## Open questions
- **Point-light count budget.** Exact cube-array capacity for the 4 GB floor (face resolution × max point lights). Decide 1024² with a low cap vs 512² with a higher cap during Task 4, measured against VRAM headroom alongside the 256 MB spot pool and lightmap atlases.
- **World self-shadow under dynamic point lights.** Out of scope here (cube faces are entity-only). If dynamic point lights over bare static geometry look wrong, a follow-up adds world geometry to the cube faces (6× world cull) — kept cube-ready so it's additive.
- **PCF radius parity.** Baked lightmap shadows filter in world space; entity-shadow PCF filters in light-space texels. Matching perceived softness is an art-tuning pass, not a formula — the tunable radius is the knob.
