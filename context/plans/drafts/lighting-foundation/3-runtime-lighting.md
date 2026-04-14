# Sub-plan 3 — Runtime Lighting

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** all engine-side lighting work. SH volume loader and 3D texture upload, world shader extension for indirect sampling, normal map loading and TBN reconstruction, clustered forward+ light list compute prepass, fragment shader direct term, shadow map passes.
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 1 (canonical lights in `MapData`) **and** sub-plan 2 (SH PRL section in compiled maps).
> **Note on size:** this sub-plan is the largest of the three, and may need to split further once we drill in. Initial pass keeps it whole; we'll break it apart if any one body of work (e.g., shadow maps) gets big enough to deserve its own sub-plan.

---

## Description

Layer the runtime lighting pipeline onto the existing world rendering path. The world shader replaces flat ambient with SH irradiance sampling, perturbs normals via tangent-space normal maps, and accumulates direct light contributions from a per-cluster light list. Shadow-casting lights write to shadow maps before the opaque pass.

The structural choices are locked in `context/lib/rendering_pipeline.md` §4 and §7. This sub-plan is the implementation of those sections, plus normal map loading, plus the shadow map passes.

---

## Bodies of work

### 3a. SH volume sampling

- Loader parses the SH PRL section into a CPU-side probe grid.
- Renderer creates a 3D texture (or a slab of 3D textures, one per SH band slab) sized to grid dimensions, uploads probe data, binds in the world shader.
- Vertex shader computes probe-grid coordinates from world position.
- Fragment shader samples the SH texture(s) trilinearly and reconstructs irradiance via the SH L2 dot product per channel.
- Replaces flat ambient in the lighting equation. Missing SH section degrades to flat white ambient.

**Texture layout note.** 27 scalars per probe don't fit in one `Rgba16Float` texel (4 scalars). Need `ceil(27 / 4) = 7` texels minimum. Preferred layout (Unity/Frostbite/DDGI lineage): three slab textures per color channel (9 total), each slab holding three SH bands. Alternative: 7 textures interleaving all 27 scalars. Either is a renderer implementation detail.

### 3b. Normal map rendering

- Loader: pair albedo with normal map per material. Load as BC5 (RG, with Z reconstructed) when available, fallback to RG8. Missing normal map → neutral `(0, 0, 1)` in tangent space.
- Vertex shader: reconstruct TBN from packed normal + packed tangent + bitangent sign (already in the `GeometryV3` vertex format from Milestone 3.5).
- Fragment shader: sample normal map, decode tangent-space normal, transform via TBN to world space, use as the per-fragment shading normal for both indirect (SH sample) and direct (cluster walk) terms.

### 3c. Clustered forward+ direct lighting

- Define cluster grid: screen-space tiles × depth slices. Sizing refined during implementation (typical: 16×16 tiles, 24 depth slices).
- Compute prepass each frame:
  - Iterate active lights (canonical lights from `MapData::lights` + transient gameplay lights from the entity system, when one exists).
  - For each cluster, test light volumes against cluster AABB.
  - Write a packed per-cluster light index list to a storage buffer.
- Fragment shader:
  - Determine fragment's cluster from screen position + depth.
  - Walk the cluster's light index list.
  - For each light, evaluate Lambert/Phong direct contribution per the canonical falloff model.
  - Output = `albedo × (sh_indirect + Σ direct_lights)`.

### 3d. Shadow maps

- **Directional lights:** cascaded shadow maps (CSM). 3 or 4 cascades; resolution intentionally modest (e.g., 1024² per cascade) to match the aesthetic.
- **Point lights:** cube shadow maps rendered in a single pass via layered rendering where supported, or six passes otherwise.
- **Spot lights:** single shadow map per light.
- Not every dynamic light casts shadows. A `cast_shadows: bool` flag on the runtime light struct (not the canonical light) gates rendering a shadow map; static canonical lights derived from FGD may default to true, transient gameplay lights to false.
- Shadow passes run before the opaque pass each frame. The fragment shader samples the appropriate shadow map per light during the cluster walk.

---

## Acceptance criteria

### SH volume sampling

- [ ] SH PRL section parses into CPU-side probe grid at level load
- [ ] Renderer creates and uploads 3D texture(s) for SH coefficients
- [ ] World shader samples SH trilinearly and reconstructs irradiance via SH L2 dot product per channel
- [ ] Indirect term replaces flat ambient
- [ ] Missing SH section degrades cleanly to flat white ambient (matches pre-Milestone-5 behavior)

### Normal maps

- [ ] Albedo + normal map loaded per material; BC5 preferred, RG8 fallback, neutral fallback for missing normal map
- [ ] Vertex shader reconstructs TBN from packed normal + packed tangent + bitangent sign
- [ ] Fragment shader applies normal map perturbation before both indirect and direct shading

### Clustered direct lighting

- [ ] Cluster grid defined and parameterized
- [ ] Compute prepass builds per-cluster light index lists from active lights each frame
- [ ] Fragment shader walks fragment's cluster, accumulates direct contributions per canonical falloff model
- [ ] Lambert (and optionally Phong) direct evaluation per light type (Point / Spot / Directional)

### Shadow maps

- [ ] CSM passes render directional lights into cascade textures
- [ ] Cube shadow map passes render point lights
- [ ] Single shadow maps render spot lights
- [ ] Fragment shader samples the appropriate shadow map per shadow-casting light during cluster walk
- [ ] `cast_shadows: bool` flag on the runtime light struct gates shadow map rendering

### Validation

- [ ] Lighting test maps look correct: indirect light bleeds around corners; direct falloff matches the falloff model; shadows are crisp at the chosen resolution; normal-mapped surfaces show correct lighting across angles
- [ ] All `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

### SH volume sampling

1. SH volume loader: parse PRL section, upload to a 3D texture (or texture slab set).

2. World shader: trilinear SH sample → irradiance reconstruction → replaces flat ambient.

### Normal maps

3. Normal map loading: albedo + normal texture pair per material; BC5 preferred, fallback RG8, neutral fallback.

4. Vertex shader: reconstruct TBN from packed normal + tangent + bitangent sign.

5. Fragment shader: sample normal map, apply TBN transform, shade with SH irradiance.

### Clustered direct lighting

6. Clustered light list compute prepass: implement tile/slice grid, per-cluster index list build.

7. World shader direct term: cluster walk, Lambert/Phong direct evaluation per canonical falloff model.

### Shadow maps

8. CSM pass for directional lights.

9. Cube shadow map pass for point lights.

10. Single shadow map pass for spot lights.

11. Fragment shader integration: sample shadow maps per shadow-casting light during cluster walk.

### Validation

12. Author lighting test maps that exercise indirect bleed, direct falloff, shadow crispness, and normal-map angle variation.

13. Manual visual review walkthrough of all test maps.

---

## Notes for implementation

- **Likely split point.** If shadow maps alone become a multi-day body of work (which is plausible — CSM, cube maps, layered rendering, shadow filtering all have real complexity), pull tasks 8–11 into a fourth sub-plan `4-shadow-maps.md`. Decide when we get there.
- **No deferred rendering.** The pipeline is clustered forward+ throughout. All shading happens in the opaque pass fragment shader. Don't be tempted to add a G-buffer for "convenience" — the architectural commitment is forward+ with depth + albedo + normal map sampled per fragment.
- **Cluster walk vs. full light loop.** For very small light counts (e.g., test maps with 3 lights), a clustered light list is overkill — a flat per-fragment loop over all lights would work. Build the cluster path anyway, because real maps will need it and the architecture is what we're shipping.
- **Shadow map resolution.** Modest by design. 1024² CSM cascades, 512² cube map faces, 1024² spot maps. Chunky shadow edges are part of the aesthetic — not a bug to fix.
- **Test map authoring.** This is the validation surface. Plan to author a few maps that exercise specific lighting cases: a corner with bright indirect bleed, a long corridor with falloff, a normal-mapped wall under a moving spot light, a directional sun with CSM coverage across multiple cascades. These maps live in `assets/maps/` and become part of the regression suite for any future lighting changes.
