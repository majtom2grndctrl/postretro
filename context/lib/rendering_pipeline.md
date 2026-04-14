# Rendering Pipeline

> **Read this when:** implementing or modifying the renderer, level loading, lighting, or any visual pass.
> **Key invariant:** renderer owns all wgpu calls. Other subsystems never touch GPU types. Level loaders produce handles; renderer consumes them.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) §4.1, §4.3

---

## 1. Frame Structure

Each frame runs five stages in fixed order. Later stages depend on results from earlier ones.

| Stage | Work |
|-------|------|
| **Input** | Poll events, update input state |
| **Game logic** | Fixed-timestep update: entity movement, collision, game rules |
| **Audio** | Update listener position, trigger sounds from game events |
| **Render** | Determine visible set (leaves via portal traversal), draw visible geometry, dynamic lights, sprites, post-processing |
| **Present** | Swap buffers |

Game logic runs at a fixed timestep decoupled from render rate. Renderer interpolates between the last two game states for smooth visuals at variable framerates. Simulation is deterministic at any refresh rate; rendering never blocks or drives the simulation clock.

**View vs. sim split.** View rotation (yaw, pitch) updates at render rate — once per frame, from mouse displacement and gamepad look velocity, before the fixed-tick loop. Player position updates inside the tick loop and is interpolated between tick states; view angles bypass interpolation and are read directly from the camera at render time. This mirrors id Tech 3: client viewangles update per frame; simulation ticks at a fixed rate. Evanescent inputs (mouse delta) are consumed at render rate so they are never lost on zero-tick frames. See `input.md §3`.

**Edge cases:** On the first frame, only one game state exists — duplicate it so interpolation produces the initial state with no blending. After a long stall (alt-tab, disk I/O), clamp the accumulator (e.g., 250ms max) to prevent dozens of catch-up ticks. On a stall catch-up (e.g., 5 ticks in one frame), view angles update once at render rate before the tick loop; all 5 ticks use the same freshest view direction.

---

## 2. Visibility and Traversal

Visibility is **computed per frame from baked portal geometry**. This is the id Tech 4 (Doom 3, 2004) approach, not Quake 1's precomputed-PVS model — Carmack's reasoning for the break still applies: precomputed PVS lengthens compile cycles, fights with dynamic geometry, and per-frame portal traversal is trivially cheap at modern leaf counts.

Portals are the primary path. The `--pvs` fallback is deprecated and will be removed once portal generation is reliable on every supported map type. New feature work targets portal traversal; do not extend the deprecated path.

### Portal traversal

Single-pass flood-fill with Sutherland-Hodgman clip-and-narrow at each hop. Depth-first, per chain.

At each portal, clip the portal polygon against the current frustum. An empty clip result (fewer than 3 vertices after clipping) rejects the portal. A non-empty clip result confirms visibility and drives frustum narrowing: the new frustum is built from the portal plane (near), one edge plane per clipped edge through the camera position, and the current far plane. Recurse into the neighbor leaf with the narrowed frustum. Solid leaves block traversal.

**Strict-subset invariant.** The clipped polygon lies entirely inside the current frustum by construction, so the edge planes derived from it form a cone strictly inside the current cone. By induction from the camera's initial frustum, every narrowed frustum reachable through any portal chain is a strict subset of the camera frustum. Every leaf the flood-fill marks visible lies inside the camera's view cone — the clip-and-narrow step replaces a separate per-leaf AABB cull in one operation.

**Per-chain cycle tracking.** Cycle prevention keys on portals crossed in the current chain, not on leaves reached globally. Keying on leaves would drop any chain after the first to arrive, losing whichever carried the widest sub-frustum. The visible cell set is the union across chains.

**Clipping robustness.** Floating-point clipping uses a small inclusive epsilon at half-space boundaries; over-inclusion at the boundary cannot violate the invariant because the next hop's edge planes discard any genuinely-outside slop. Degenerate clipped polygons — those touching the frustum only at a single point or edge — take the empty-case rejection path.

### PVS fallback

When a PRL file was built with `--pvs`, the Portals section is absent and a precomputed PVS bitset replaces runtime traversal. The renderer descends to the camera leaf, looks up its bitset, and draws every empty leaf that survives per-leaf AABB frustum culling.

### Non-portal fallback paths

PVS, solid-leaf, exterior-camera, and missing-visibility paths all use per-leaf AABB frustum culling instead of clip-and-narrow.

**Missing visibility data:** when neither portals nor PVS is present (PRL without a visibility section), draw all empty leaves with frustum culling only. Slower but correct.

**Camera outside playable space:** camera in exterior or solid leaf. Frustum-cull all interior leaves. Back-face culling hides the level shell — face winding is front-facing from inside, back-facing from outside. Same cull mode serves both cases.

See `build_pipeline.md` §Runtime visibility for the compile-side picture.

---

## 3. Level Loading Pipeline

Loader parses PRL via the `postretro-level-format` crate. The heavy work happened at compile time in prl-build; runtime load is buffer hand-off and texture matching. Load uploads the global vertex/index buffer and the BVH node[] + leaf[] arrays (§5) to GPU storage buffers, matches PNG textures by name string (checkerboard placeholder for missing albedo, neutral normal (0,0,1) for missing normal map), and allocates one permanent indirect buffer slot per BVH leaf. At load time the Rust side scans the sorted leaf array once to derive per-bucket `(first_slot, count)` ranges. The renderer performs all GPU uploads and returns opaque handles; the loader never touches wgpu and raw PRL types do not cross into renderer code.

---

## 4. Lighting

Lighting has two components: **dynamic direct illumination** (clustered forward+ with shadow maps) and **baked indirect illumination** (SH irradiance volume sampled per fragment). Both are evaluated in the world shader during the opaque geometry pass — no deferred stages, no lightmap atlas.

**Direct illumination.** Dynamic lights (point, spot, directional) are built into a clustered light list each frame by a compute prepass. The fragment shader reads the cluster for its screen-space tile and accumulates contributions from lights whose volume reaches that fragment. Shadow-casting lights write to shadow maps (cascaded shadow maps for directional, cube shadow maps for point and spot) before the main pass; the fragment shader samples them during accumulation. Light sources originate from FGD entities (`light`, `light_spot`, `light_sun`) and from gameplay effects (muzzle flashes, explosions).

**Indirect illumination.** prl-build bakes a regular 3D grid of SH L2 probes over the level's empty space, evaluating incoming radiance at each probe by raycasting against static geometry with canonical lights as sources. The runtime samples the probe grid via trilinear interpolation in the fragment shader. Missing probe section falls back to flat white ambient.

**Normal maps.** Tangent-space normal maps perturb the per-fragment normal before both direct and indirect evaluation. Tangents are packed into the vertex format (§6) at compile time.

**Light entity authoring.** Mappers place light entities in TrenchBroom. The compiler's translation layer converts mapper-facing FGD properties to an internal canonical format, applying validation rules (falloff distance required, spotlight direction verified, intensity bounds checked). Canonical lights feed both the SH baker and the runtime direct-lighting path. See `build_pipeline.md` §Custom FGD.

Full spec: `context/plans/drafts/lighting-foundation/`

---

## 5. Cells, BVH, and Draw Leaves

**Cell** = opaque visibility unit. The compiler assigns one cell per empty BSP leaf; `cell_id` is the BSP leaf index, used as an opaque identifier at runtime. **Cluster** = screen-space light-culling grid (§7.1 step 4), never spatial. Rule: cell = world space, cluster = screen space.

World geometry is organized into a global BVH at compile time. Each **BVH leaf** covers one `(face, material_bucket)` pair:

| Field | Content |
|-------|---------|
| `cell_id` | Opaque cell identifier (BSP leaf index) |
| `aabb` | World-space bounds of this leaf's triangle set |
| `index_offset` | Start of this leaf's triangles in the shared index buffer |
| `index_count` | Number of indices |
| `material_bucket_id` | `(albedo, normal_map)` pair the indices reference |

Leaves are sorted by `material_bucket_id` in the flat leaf array so each bucket owns a contiguous slot range. Each leaf's position in the array is its permanent indirect buffer slot. No atomic counter; overflow is architecturally impossible.

BVH nodes (40 bytes each) are stored in DFS order with a `skip_index` per node — the node to jump to on AABB reject. Left child is always at `current_index + 1`. Internal nodes carry an AABB; leaf nodes additionally carry a `left_child_or_leaf_index` pointing into the leaf array.

Flow: portal traversal (§2) produces a visible-cell bitmask (128 `u32` words, 512 bytes) → BVH traversal compute (§7.1 step 2) walks the tree, tests each leaf AABB and its cell bitmask bit, writes/zeros the corresponding indirect buffer slot → opaque pass (§7.2) issues one `multi_draw_indexed_indirect` call per material bucket against its contiguous slot range.

---

## 6. Vertex Format

Custom vertex format used for all world geometry. Packed for cache efficiency — non-position attributes are quantized where the precision loss is imperceptible at the target aesthetic.

| Attribute | Content | Purpose |
|-----------|---------|---------|
| Position | `f32 × 3` world-space coordinate (Y-up, engine meters) | Geometry placement |
| Base UV | `f32 × 2` texture-space coordinate, normalized by texture dimensions | Diffuse and normal-map texture sampling |
| Normal | Octahedral-encoded `u16 × 2` | Per-fragment shading normal (pre-normal-map) |
| Tangent | Octahedral-encoded `u16 × 2` plus sign bit | Tangent-space basis for normal-map sampling |

UVs are computed from face projection data (s-axis, t-axis, offsets) during compilation. The GPU sampler uses repeat addressing — UVs outside [0, 1] tile correctly.

Octahedral encoding preserves direction to visually-indistinguishable precision at half the storage of `f32 × 3`. The tangent's bitangent sign rides in a spare bit so the vertex shader can reconstruct the full TBN matrix. Both are generated in prl-build's brush-side projection stage — normals from the face plane, tangents from the UV projection axes.

No per-vertex lighting channel: direct light and SH indirect both accumulate per fragment (§4).

---

## 7. Rendering Stages

Clustered forward+ pipeline. Each frame runs a small set of compute prepasses that build culling and lighting state, then a single opaque geometry pass that consumes it, then post-processing.

### 7.1 Visibility and Culling Prepasses

1. **Portal traversal** (CPU) — §2 flood-fill produces the visible cell set.
2. **BVH traversal** (compute, *Milestone 4*) — reads the visible-cell bitmask (128 `u32` words) produced by portal DFS; walks the global BVH via flat skip-index DFS (no stack, no depth cap, single invocation); tests each leaf AABB against the frustum and the leaf's `cell_id` against the bitmask; writes or zeros the leaf's permanent indirect buffer slot.
3. **Clustered light list** (compute, *Phase 4*) — builds per-cluster light index lists from the dynamic light set. Cluster grid is screen-space tiles × depth slices.

### 7.2 World Geometry

Single opaque pass. CPU issues one `multi_draw_indexed_indirect` call per material bucket against its slice of the indirect buffer built in §7.1 — typically 10–50 calls per frame. Collapsing to one call would need bindless descriptor arrays, not baseline in wgpu. Per-fragment shading:

- Sample base texture and normal map at the UV coordinate. Reconstruct world-space normal from the TBN and normal-map sample.
- Sample the SH L2 irradiance volume at fragment position (trilinear) for indirect lighting.
- Walk the fragment's cluster light list; for each light, evaluate direct contribution and sample the associated shadow map.
- Output = `albedo × (indirect_sh + Σ direct_lights)`.

Depth testing (Less, write enabled) and back-face culling (counter-clockwise front face) are permanent from this phase forward.

Shadow maps, billboards, emissive bypass, fog volumes, and post-processing attach to this pipeline in later phases. See the roadmap and per-phase plans.

---

## 8. Boundary Rule

All wgpu calls live in the renderer module. Map loader, game logic, audio, and input never import wgpu types. Data crosses the boundary as engine-defined types; the renderer translates to GPU operations. Per-subsystem contracts live with the subsystem: vertex format §6, cell chunks §5, lighting data §4.

---

## 9. Camera

Projection and view parameters for rendering and visibility.

### Coordinate System

Right-handed, Y-up. Matches glam's default conventions and wgpu's NDC expectations. Forward is -Z.

### Projection Defaults

| Parameter | Default | Rationale |
|-----------|---------|-----------|
| Horizontal FOV | 100° | Modern boomer shooter default. Configurable 60°–130°. Vertical FOV derived from aspect ratio. |
| Near clip | 0.1 units | Close enough for weapon models without z-fighting artifacts |
| Far clip | 4096.0 units | Covers the full coordinate range for large maps |
| Aspect ratio | Derived from window dimensions | Updated on window resize |

### View Matrix

Camera position and orientation produce a view matrix each frame. The view matrix feeds:

- Visibility (§2) — camera position seeds the portal-traversal flood-fill (default) or the PVS lookup (`--pvs` fallback)
- Frustum culling — view-projection matrix defines the clip volume
- All draw calls — view-projection uniform uploaded once per frame

---

## 10. Non-Goals

- **Deferred rendering** — clustered forward+ is sufficient for the target light count and aesthetic. Deferred adds complexity without benefit here.
- **Baked lightmaps** — indirect lighting lives in the SH irradiance volume. No lightmap atlas, no per-face lightmap UVs, no lightmap bake stage.
- **PBR materials** — albedo + normal map is the full material vocabulary. Metallic/roughness workflows are out of scope.
- **Hardware ray tracing** — not available in baseline wgpu. Shadow maps cover dynamic shadowing; the SH volume covers indirect illumination.
- **Mesh shaders** — not baseline in wgpu. GPU-driven culling uses compute + `draw_indexed_indirect` instead.
- **Runtime level compilation** — maps are compiled offline by prl-build. The engine is a consumer, not a compiler.
- **Multiplayer / networking** — single-player engine. Out of project scope.
