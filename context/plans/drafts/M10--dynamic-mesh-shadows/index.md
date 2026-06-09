# Dynamic Mesh Shadow Casting

> **Status:** draft. Milestone 10 render-foundation track. Follows *Mesh render pass + MeshComponent*.
> **Related:** `context/lib/rendering_pipeline.md` §4 (lighting), §7.1 (shadow passes), §9 (skinned model pipeline) · `shadow-cone-cull` (shipped: the per-slot cone-culled spot depth pass + `lighting/cone_frustum.rs` predicate this builds on) · `lighting--entity-direct-sh` (the SH-direct term that motivates this) · sibling `research.md` (code anchors + algorithm/feasibility citations).

## Goal

Animated entity meshes cast real-time shadows from runtime dynamic lights, grounding enemies that currently read as lit-but-floating. Entities already get crisp lit/dark contrast from baked SH-direct + SH-indirect; the missing piece is the shadow they throw onto the world. Adds a skinned depth-only variant of the mesh pass that renders entity occluders into the shadow pool — spot lights into the existing 2D-array pool, point lights into a new cube-array pool.

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
- [ ] A skinned enemy under a runtime dynamic **point** light casts a shadow that is correct in every horizontal direction (cube-mapped, no missing quadrant, no hard seam across cube faces).
- [ ] An enemy outside a light's cone (spot) / influence sphere (point) is not drawn into that light's shadow map. Verified by a CPU-side submitted-instance counter, mirroring `shadow-cone-cull`'s counter pattern — no GPU readback.
- [ ] Baked-only and `static_light_map`/`sdf` lights cast no entity shadow; only `is_dynamic` lights do. Verified by a unit test on the caster-eligibility predicate.
- [ ] Entity shadow edges are anti-aliased, not single-texel stair-stepped; the PCF radius is set by one parameter and changing it visibly widens/narrows the penumbra.
- [ ] The `cast_shadows` field is removed from the canonical light, the runtime light, and the PRL light record; no code or test references it.
- [ ] `casts_entity_shadows` set on a non-`is_dynamic` light is rejected at compile time (warning logged, value cleared); on a dynamic-tier light it toggles entity-shadow casting. Verified by level-compiler unit tests.
- [ ] When the count of shadow-casting dynamic point lights exceeds the cube-array capacity, the lowest-ranked are dropped gracefully (no panic, no validation error). Verified by a unit test on point-pool allocation.
- [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test -p postretro`, and `cargo test -p postretro-level-compiler` pass.

## Tasks

### Task 1: Skinned depth-only pass + palette hoist
Add a depth-only render pipeline that skins vertices with the mesh pass's `skin_matrix` and projects by a **light-space matrix supplied as a parameter** (group 0 = a per-render light-space uniform, like the existing spot `shadow_vs_bind_group`; group 3 = the palette + per-instance SSBO the mesh pass already owns). Position/joints/weights only — drop color attributes, mirroring `depth_prepass.wgsl`'s relationship to `forward.wgsl`. Build it cube-ready: nothing in the pipeline or shader may assume one slot per light or a 2D target. Hoist the bone-palette and per-instance buffer writes (`MeshPass::render_frame`'s `write_buffer` calls) to run **before** the shadow depth passes in `render/mod.rs`, so the skinned-depth pass reads a populated palette. The mesh pass then reuses the same buffers for its own draw.

### Task 2: Spot entity-shadow integration
Render entity occluders into each occupied spot-pool slot's depth pass, after the shipped `shadow_cull.draw_slot_indirect` world draw, within the same per-slot "Spot Shadow Depth Pass" render pass (same depth layer, no clear — `CompareFunction::Less` composites world + entity). Set the skinned-depth pipeline (Task 1) and entity instance draws after the world indirect draw in that pass. The slot's light-space matrix is the `slot_cone_matrices` entry already stashed on `SpotShadowPool` in `update_dynamic_light_slots`. Cull entity instances per slot on the CPU against the slot's cone frustum via `cone_frustum_planes` + `aabb_intersects_frustum`. Add tunable-radius PCF to `sample_spot_shadow`. World forward sampling is unchanged — verify entity shadows appear on world receivers for dynamic spots.

### Task 3: Two-axis authoring cleanup
Retire `cast_shadows`: remove it from the compiler `MapLight`, the runtime `MapLight`, the PRL light record packing, the always-true assignment in `quake_map.rs`, and the dead `cast_shadows == false` bake branch. Redefine `casts_entity_shadows` as "this light casts shadows from dynamic entities," valid only when `is_dynamic`. The level compiler warns and clears `casts_entity_shadows` on any non-`is_dynamic` light. Update the FGD: expose `_cast_entity_shadows` on the dynamic-light classes (default on), remove it from the baked `light`/`light_spot`/`light_sun` base. Document the static-geo (`shadow_type`) × entity-shadow (`casts_entity_shadows`) model in the FGD help text.

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

**Caster eligibility.** Reuse `rank_lights`'s `is_dynamic || casts_entity_shadows` for pool entry, but entity *occluders* render only for `is_dynamic` lights (Task 3 makes `casts_entity_shadows` imply `is_dynamic`). Spot stays `LightType::Spot`; point is the new `LightType::Point` + cube pool.

**Cube pool (point).** Depth cube-array comparison sampling is supported on Metal/DX12/Vulkan via wgpu (Bevy ships it); WebGL is the only gap and not a target. Option A (dedicated cube-array) over packed-faces-in-2D-array: hardware seamless filtering, trivial direction-vector sampling, +1 binding. Render to one face = a 2D-array render view at `baseArrayLayer = slot*6 + face`. See `research.md` for citations.

**Palette ordering.** Today: spot depth passes (`render/mod.rs` ≈ "Spot Shadow Depth Pass") run before the mesh pass, but the palette is written inside the later mesh pass. Split palette+instance upload into a step the renderer runs after `update_dynamic_light_slots` and before the shadow depth loop; both the shadow passes and the mesh pass read the same buffers.

**Key modules.** `crates/postretro/src/render/mesh_pass.rs`, `crates/postretro/src/render/mesh_instances.rs`, `crates/postretro/src/shaders/skinned_mesh.wgsl`, `crates/postretro/src/lighting/spot_shadow.rs`, a new sibling cube pool in `lighting/`, `crates/postretro/src/shaders/forward.wgsl`, `crates/postretro/src/render/mod.rs` (orchestration), `sdk/TrenchBroom/postretro.fgd`, `crates/level-compiler/src/format/quake_map.rs`, `crates/level-compiler/src/map_data.rs`, `crates/postretro/src/prl.rs`.

## Boundary inventory

| Name | Rust (canonical/runtime) | Wire / PRL | FGD KVP | Notes |
|---|---|---|---|---|
| entity-shadow toggle | `MapLight.casts_entity_shadows: bool` | existing light-record byte | `_cast_entity_shadows` (0/1) | Semantics change, not layout. Valid only when `is_dynamic`. |
| static-geo technique | `MapLight.shadow_type: ShadowType` | existing | `_shadow_type` (`static_light_map`/`sdf`) | Unchanged; documented as the orthogonal axis. |
| dead flag (removed) | `MapLight.cast_shadows` | **removed from record** | none (never authored) | See Wire format. |

## Wire format

Removing `cast_shadows` drops one field from the PRL light record. The light record is packed in `crates/level-compiler/src/pack.rs` and decoded in `crates/postretro/src/prl.rs`; update both in step. No new section. The cube-array pool is a runtime GPU resource — no PRL data. Point-light entity-shadow eligibility derives from the existing `is_dynamic` + `LightType::Point` fields; no new wire surface.

## Open questions
- **Point-light count budget.** Exact cube-array capacity for the 4 GB floor (face resolution × max point lights). Decide 1024² with a low cap vs 512² with a higher cap during Task 4, measured against VRAM headroom alongside the 256 MB spot pool and lightmap atlases.
- **World self-shadow under dynamic point lights.** Out of scope here (cube faces are entity-only). If dynamic point lights over bare static geometry look wrong, a follow-up adds world geometry to the cube faces (6× world cull) — kept cube-ready so it's additive.
- **PCF radius parity.** Baked lightmap shadows filter in world space; entity-shadow PCF filters in light-space texels. Matching perceived softness is an art-tuning pass, not a formula — the tunable radius is the knob.
