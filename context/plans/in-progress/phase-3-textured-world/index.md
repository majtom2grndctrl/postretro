# Phase 3: Textured World

> **Status:** ready
> **Depends on:** Phase 1 (BSP loading, wireframe renderer, PVS/frustum culling), Phase 2 (fixed-timestep loop, action-mapped camera).
> **Related:** `context/lib/rendering_pipeline.md` · `context/lib/resource_management.md` · `context/lib/development_guide.md`

---

## Goal

Replace wireframe rendering with textured solid geometry using flat uniform lighting. This proves the visual data path: BSP texture name to PNG on disk, sampled per fragment in a single-texture render pipeline. Material derivation from texture name prefixes establishes the hook for footstep sounds, impact effects, and emissive rendering in later phases. Depth buffer and back-face culling introduced here are permanent. CSG face clipping eliminates z-fighting from overlapping PRL brushes.

**Phase 4 adds lighting.** Phase 3 ships with flat uniform white lighting — no lightmaps, no probes. That work is the Phase 4 decision gate.

---

## Scope

### In scope

- Vertex format: position (vec3), base_uv (vec2), vertex_color (vec4). No lightmap UVs.
- Base texture UV computation via `PlanarTextureProjection::project()`, normalized by texture dimensions
- PNG texture loading via `image` crate, matched by BSP texture name strings
- Checkerboard placeholder for missing textures
- Material derivation from texture name prefix (first `_`-delimited token)
- Depth buffer (Depth24Plus or Depth32Float)
- Back-face culling (front = counter-clockwise after coordinate transform)
- Single-texture render pipeline: vertex shader (MVP transform), fragment shader (base texture * flat uniform lighting)
- Per-texture bind groups, draw calls grouped by texture
- CSG face clipping in the PRL compiler to remove faces inside solid brush space (eliminates z-fighting)
- BSP path only for textured rendering (see PRL gap note below)

### Out of scope

- Lightmap atlas (deferred to Phase 5 if Phase 4 probe lighting falls short)
- Lightmap UVs in vertex format (no lightmap_uv attribute)
- BSPX LIGHTING, RGBLIGHTING, DECOUPLED_LM parsing (Phase 4+)
- Light probes / LIGHTGRID_OCTREE (Phase 4)
- World texture atlas batching (optimization, unscheduled)
- Normal maps (Phase 5+)
- Animated lightmap styles (Phase 5+)
- Emissive rendering behavior (material flag is set; bypass is Phase 5)
- Dynamic lights (Phase 5+)
- Fog volumes (Phase 5+)
- Billboard sprites (Phase 5+)
- Texture hot-reload

### PRL gap

The PRL compiler parses texture names from `.map` files but discards them at pack time. The PRL format has no texture data section. Phase 3 textured rendering targets BSP exclusively. PRL levels fall back to wireframe or solid-unlit rendering. Texture data in PRL is a future task, unscheduled.

CSG face clipping (Task 01) is PRL-only — it runs in the compiler to fix z-fighting in PRL geometry. BSP already handles this via BSP tree construction.

---

## Shared Context

Cross-task decisions. Each task file restates its relevant subset — this is the single source.

### Coordinate transform

Quake BSP uses right-handed Z-up. Engine uses Y-up (glam default). Phase 1 applies: swap Y and Z, negate the new Z. UV computation operates on pre-transform BSP vertices (Z-up) because `PlanarTextureProjection::project()` expects Quake-space coordinates. Apply the coordinate transform after UV computation, or pass untransformed positions to the projection and transform separately.

### Vertex format

| Attribute | Type | Size | Content |
|-----------|------|------|---------|
| position | vec3 f32 | 12 bytes | World-space coordinate (Y-up, post-transform) |
| base_uv | vec2 f32 | 8 bytes | Texture-space UV, normalized by texture dimensions |
| vertex_color | vec4 f32 | 16 bytes | RGBA per-vertex tint (white default; Phase 5 uses for dynamic lights) |

Total stride: 36 bytes. Vertex buffer layout must match exactly across loader and shader.

No lightmap_uv attribute. Phase 4 evaluates probe-based lighting; lightmap UVs are only needed if Phase 5 falls back to lightmap atlas.

### Flat uniform lighting

Phase 3 fragment shader applies a flat ambient factor rather than sampled lightmap data. A uniform `vec3` (e.g., `[1.0, 1.0, 1.0]`) multiplied with the base texture sample. This is intentionally simple — Phase 4 replaces it with probe-sampled lighting.

### Texture name matching

BSP stores texture names in `BspMipTexture.header.name`. The texture loader receives a texture root path (resolved by the engine from the map file's asset root — e.g., `assets/textures/` when loading `assets/maps/test.bsp`). It searches the texture root recursively for `<name>.png` (case-insensitive match on filename stem). The collection subdirectory is not stored in BSP — search all collections under the texture root.

### Missing data degradation

| Condition | Behavior |
|-----------|----------|
| Missing texture PNG | Checkerboard placeholder (magenta/black, 64x64). Log warning per missing texture. |
| Unknown material prefix | Default material. Log warning per unique unknown prefix. |
| PRL level loaded | Wireframe or solid-unlit fallback. No texture data available in PRL format. |

---

## Task List

| ID | Task | File | Dependencies | Description |
|----|------|------|-------------|-------------|
| 01 | CSG Face Clipping | `task-01-csg-face-clipping.md` | none | Clip PRL faces against overlapping brush volumes at compile time using existing `geometry_utils.rs` clipping. Eliminates z-fighting. |
| 02 | Vertex Format and Base Texture UVs | `task-02-vertex-format.md` | none | Replace wireframe vertex format with textured format (no lightmap_uv). Compute base texture UVs. Extend face metadata. Update LevelGeometry. |
| 03 | Texture Loading | `task-03-texture-loading.md` | none | Load PNGs matched by BSP texture names. Checkerboard fallback. CPU-side RGBA8 output to renderer. |
| 04 | Material Derivation | `task-04-material-derivation.md` | 02 | Parse texture name prefix from face metadata, map to material enum, attach to per-face metadata. |
| 05 | Render Pipeline | `task-05-render-pipeline.md` | 02, 03 | Replace wireframe with solid single-texture pipeline. Depth buffer, back-face culling, flat uniform lighting. |

---

## Execution Order

### Dependency graph

```
  +------------------+     +------------------+     +------------------+
  | 01 CSG face      |     | 02 Vertex format |     | 03 Texture       |
  | clipping         |     | and base UVs     |     | loading          |
  | (compiler)       |     | (engine)         |     | (engine)         |
  +------------------+     +--------+---------+     +--------+---------+
                                    |                        |
                           +--------v---------+              |
                           | 04 Material      |              |
                           | derivation       |              |
                           +--------+---------+              |
                                    |                        |
                                    +------------------------+
                                    |
                           +--------v---------+
                           | 05 Render        |
                           | pipeline         |
                           +------------------+
```

### Concurrency rules

| Wave | Tasks | Notes |
|------|-------|-------|
| Wave 1 (parallel) | 01, 02, 03 | Zero overlap. Task 01 is compiler-only (`postretro-level-compiler`). Task 02 touches `bsp.rs`/`render.rs`. Task 03 creates new files only. |
| Wave 2 | 04 | Reads face metadata (texture names) added by Task 02. Sequential to avoid FaceMeta merge conflicts. |
| Wave 3 | 05 | Rewrites `render.rs` using vertex format (02) and loaded textures (03). Must be last. |

### Orchestrator notes

- Task 01 (CSG) runs in `postretro-level-compiler` — different crate, zero file overlap with engine tasks. Safe to parallelize with anything.
- Task 02 requires a test map with PNG textures under `textures/` for verification. The BSP does not need recompilation — flat uniform lighting requires no baked data. Simple solid-color 64x64 PNGs suffice.
- Task 03 (texture loading) creates a new module and adds the `image` crate. No existing file modifications beyond `Cargo.toml` and `main.rs` (mod declaration).
- Task 04 (material derivation) reads `FaceMeta` fields that Task 02 adds. Running it in Wave 2 avoids both agents extending the same struct.
- Task 05 is the integration point. Rewrites `render.rs` — removes wireframe infrastructure, adds textured pipeline. The implementing agent needs familiarity with the Phase 1 renderer.
- Task 01 runs in the PRL compiler, not the engine. Independent of the BSP rendering tasks. Can run in any wave.

---

## Acceptance Criteria

1. **Textured rendering works.** `cargo run -- assets/maps/test.bsp` shows textured BSP geometry with uniform flat lighting. No wireframe.
2. **Uniform lighting.** All surfaces lit at the same constant level. No per-surface variation, no shadows. (Phase 4 adds probe lighting.)
3. **Missing textures degrade.** Remove a PNG — affected surfaces render with checkerboard. Warning logged. No crash.
4. **Depth buffer works.** No z-fighting. Near geometry occludes far geometry correctly.
5. **Back-face culling works.** Interior room faces visible; exterior back-faces culled.
6. **Materials are derived.** `RUST_LOG=info` shows material assignments. Unknown prefixes produce warnings.
7. **Module boundary holds.** BSP loader and texture loader contain zero wgpu imports. Renderer does not parse BSP structures or PNG files.
8. **CSG clipping compiles.** Compiling a `.map` with overlapping brushes to PRL produces geometry without z-fighting in the output.
9. **PRL levels still load.** PRL levels render in wireframe or solid-unlit. No crash, no missing texture errors (expected — PRL has no texture data).

---

## Open Questions

Identified during plan review (2026-04-08). Resolve before moving to ready.

### Completeness gaps

**Q1 — Task 01: Brush index on Face struct.**
The compiler's `Face` struct has no `brush_index` field. Task 01 says "clip against all brushes except the one that generated this face" but doesn't mention adding this field. Should the task say "add a `brush_index: usize` field to `Face` during parsing" and leave details to the implementer, or is there a different way to track face-to-brush origin?

- [x] Resolved — No `brush_index` needed. Classic CSG approach: clip each face against *all* brush volumes including its own. A face on its own brush's boundary plane is not geometrically "inside" that brush (it sits on the plane, not behind all half-planes), so no self-clipping occurs. Epsilon for the inside test must be strictly negative. This matches the id Tech approach from Quake through Doom 3. AABB early-out recommended for performance. Task 01 updated.

**Q2 — Task 02: `load_bsp` texture data access.**
`load_bsp` currently drops the `BspData` after extracting vertex positions. Accessing `tex_info`, `textures`, and `get_texture_name()` requires significant restructuring of that function. Should the task include explicit guidance about this, or is it enough to flag that BSP data needs to remain accessible during face iteration and trust the implementing agent?

- [x] Resolved — Restructuring `load_bsp` to retain texture metadata access during face iteration is expected and in scope. Task 02 updated with explicit permission to refactor and three guardrails: don't clone or store `BspData` as runtime data, don't explode struct fields into separate function parameters, don't add wgpu types to the BSP loader.

**Q3 — Task 02: `None` handling for texture data.**
`texture_idx` is `TextureIdxField(Option<u32>)` and `bsp.textures` is `Vec<Option<BspMipTexture>>` — entries can be `None`. Proposed default: treat as missing texture → checkerboard fallback, log warning. Correct?

- [x] Resolved — Confirmed. `None` in `texture_idx` or `bsp.textures` entry is treated as missing texture — checkerboard fallback, log warning. Consistent with the Missing data degradation table. Task 02 updated.

**Q4 — Task 02: `FixedStr<16>` → String conversion.**
BSP names are `FixedStr<16>`, not `String`. The `BspData::get_texture_name()` helper exists and returns `Option<TextureName>` where `TextureName = FixedStr<32>`. Plan should reference this helper. No question — will add.

- [x] Resolved — Not a question — added reference to `BspData::get_texture_name()` helper in Task 02. Returns `Option<TextureName>` (`FixedStr<32>`). Task 02 updated.

### Suggestions

**Q5 — Task 03: Texture base path resolution.**
Where should the texture loader search? Relative to CWD? Relative to the BSP file? `assets/textures/` to match existing `assets/maps/` convention? Current `cargo run` examples use `assets/maps/test.bsp`.

- [x] Resolved — The texture loader takes a base path parameter (the "texture root"), not a hardcoded location. The engine resolves the texture root from the map file's parent directory: given `assets/maps/test.bsp`, the texture root is `assets/textures/`. Convention: `<asset_root>/textures/<collection>/<name>.png`, searched recursively from the texture root. The texture root is a parameter so that a future sample-game crate or alternative asset layout can pass its own root without engine changes. Phase 3 default: sibling `textures/` directory relative to the map file's `<asset_root>`. Task 03 updated.

**Q6 — Task 02 AC 6: Wireframe backward compatibility.**
Task 02's acceptance criterion 6 says "Existing wireframe rendering still works with the new vertex format (or a debug flag toggles it)." This is ambiguous. Proposal: drop wireframe entirely in Task 05; don't require Task 02 to maintain it. Task 02 can break wireframe since Task 05 replaces it in the same phase.

- [x] Resolved — Keep wireframe working through Task 02. The new vertex format is a superset — wireframe shader ignores UV and color attributes, only the vertex buffer layout stride needs updating to match the new 36-byte format. Task 05 replaces wireframe with the textured pipeline. Task 02 AC 6 clarified.

**Q7 — Task 05: PVS + per-texture draw grouping.**
The plan says "integrate with PVS and frustum culling: only draw faces in the visible set. Group visible faces by texture, then draw." This is non-trivial — the implementer needs to decide between pre-sorting the index buffer by texture at load time, maintaining separate index ranges per texture per leaf, or rebuilding a draw list each frame. Add guidance, or leave as a design decision for the implementing agent?

- [x] Resolved — Sort by **(leaf, texture_index)**, not by texture_index alone. This preserves the PRL invariant that faces are contiguous per leaf (PVS depends on `face_start + face_count`) while enabling texture batching within each leaf. Pre-compute per-leaf texture sub-ranges at load time. The draw loop becomes: for each visible leaf, for each texture sub-range in that leaf, set bind group + `draw_indexed()`. One draw call per (visible_leaf, texture) pair.

  Implementation guardrails (antipatterns to avoid):
  - **Don't sort globally by texture.** A global texture sort breaks leaf contiguity, which breaks PVS face collection. The sort key is `(leaf_index, texture_index)`.
  - **Don't rebuild the draw list per frame.** Pre-compute `Vec<TextureSubRange>` per leaf at load time (texture_index, index_offset, index_count). At runtime, PVS selects visible leaves; the renderer iterates their pre-built sub-ranges. No per-frame sorting, hashing, or allocation.
  - **Don't issue one draw call per face.** Merge contiguous same-texture faces within a leaf into one `draw_indexed()` range. The sort-by-(leaf, texture) guarantees same-texture faces are adjacent within each leaf's range.
  - **Don't duplicate index buffer data.** The global index buffer remains shared. Per-leaf texture sub-ranges are metadata (offset + count into the shared buffer), not copies.

  Task 05 updated to replace the "sort by texture index" guidance with the (leaf, texture) scheme.

---

## What Carries Forward

| Output | Consumed by |
|--------|-------------|
| Vertex format (position, base UV, vertex color) | Phase 4 extends with probe-sampled lighting via vertex_color or a new per-frame uniform. |
| Texture loading pipeline | Every phase that adds visual assets. |
| Material enum and prefix derivation | Phase 4 (footsteps), Phase 5 (emissive bypass), Phase 7 (surface interaction). |
| Depth buffer | Every phase — solid rendering requires depth testing. |
| Flat uniform lighting pipeline | Phase 4 replaces uniform factor with probe-sampled value. |
| CSG face clipping | Permanent compiler step for PRL geometry. |

### Replaced in later phases

| Phase 3 artifact | Replaced by | When |
|------------------|-------------|------|
| Flat uniform lighting | Probe-sampled per-surface lighting | Phase 4 |
| Per-texture individual bind | World texture atlas (batched draw calls) | Optimization pass (unscheduled) |
| Material derivation (stub behaviors) | Material behaviors with real consumers | Phase 4 (footsteps), Phase 5 (emissive) |
