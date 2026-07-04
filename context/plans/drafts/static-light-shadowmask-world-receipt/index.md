# Static-Light Shadowmask — Entity Shadows onto World Surfaces

> **Depends on:** `static-light-entity-shadows` (promotion machinery, selected-light set, weights, pool slots). Do not start before that plan ships.

## Goal

Complete the static-light entity-shadow picture: an entity standing under a promoted static light casts a crisp shadow onto world geometry. World direct lighting stays fully baked — the runtime never re-shadows static geometry onto static geometry; a per-light baked occlusion mask union-combined with the pool shadow map darkens only what the entity newly occludes.

## Scope

### In scope

- Per-light baked occlusion masks for selected lights (lightmap-space, RGBA channels, ≤ 4 overlapping selected lights per texel via channel assignment).
- Forward-pass union term: per promoted light, subtract the light's reconstructed direct contribution × `max(0, baked_visibility − shadow_map_visibility) × w` — nonzero only where an entity (or other runtime occluder in the slot's depth map) blocks a texel the bake left lit. No double-darkening where baked and runtime shadows overlap, by construction of the max.
- A prototype gate before the bake work: validate the union subtraction against the bumped-Lambert directional lightmap reconstruction on a fixture map.

### Out of scope

- Everything the parent plan ships (promotion, entity receipt, SH LOD handoff).
- Runtime static→static shadows (explicit non-goal — the lightmap owns them).
- Lightmap array-consolidation refactor (texture budget fits without it; consolidation remains the banked fallback if a future feature needs the headroom).
- More than 4 overlapping selected lights per texel (compiler warns and drops the lowest-intensity mask assignment).
- Dynamic-light interaction changes (dynamic lights keep their existing multiply-in-loop shadowing).

## Acceptance criteria

- [ ] Prototype gate passes: on a fixture map, the union term darkens an entity-shaped region under a selected light with no visible ringing or direction artifacts from the bumped-Lambert reconstruction, and overlapping baked + entity shadow shows no double-darkening (screenshot comparison). A failed gate stops the plan and the failure mode is written back into this spec.
- [ ] Compiler bakes per-light occlusion masks for selected lights and assigns channels; a fixture with > 4 overlapping selected lights compiles with a warning and drops the excess deterministically (compiler tests).
- [ ] An entity inside a promoted static light's cone casts a visible crisp shadow on floor/wall geometry; the shadow fades with the light's promotion weight `w` (manual verification on `campaign-test.prl`).
- [ ] Where the entity's shadow crosses a baked static shadow, the darker of the two wins — no additive darkening (visual check with isolation modes).
- [ ] With nothing promoted, forward output is unchanged from the parent plan's baseline.
- [ ] Forward sampled-texture count rises by exactly one (the mask atlas) and stays ≤ 16 on both cube-array variants; the budget guard test pins the new counts.
- [ ] Lightmap irradiance/direction bake output for existing maps is unchanged (masks are additive data).

## Tasks

### Task 1: Prototype gate — union vs directional lightmap reconstruction

Spike, not shipped code. On a fixture map with one selected spot over a flat lightmapped floor: hand-bake (or stub) a per-light visibility value, and in `forward.wgsl` subtract `per_light_direct × max(0, baked_vis − shadow_map_vis)` from the lit result, where `per_light_direct` is reconstructed from the light record (position/color/falloff/cone from `spec_lights`) with the same bumped-Lambert normal response the lightmap term uses. Compare against ground truth (the same map re-baked with a static occluder proxy where the entity stands). Deliverable: go/no-go note in this plan folder with screenshots, plus the reconstruction formula that matched. If the directional-atlas interaction proves intractable, record the failure and stop — the parent plan stands alone.

### Task 2: Per-light occlusion mask bake + PRL section

Compiler: for each selected light (the `EntityShadowLights` set), bake a lightmap-space visibility mask — the same area-sampled `soft_visibility` the lightmap bake computes per light, stored per texel instead of folded into the sum; the per-light layer cache in `lightmap_layer.rs` already isolates per-light contributions, reuse its plumbing. Pack up to 4 selected lights per texel into RGBA channels; channel assignment is per light with a graph-coloring pass over spatial overlap (two selected lights sharing any texel get different channels); on > 4-way overlap, drop the lowest-intensity light's mask with a compile warning. Emit as a new PRL section (`ShadowmaskAtlas`: atlas dimensions matching the lightmap atlas, `Rgba8Unorm` texel payload, plus per-selected-light channel index; SectionId from the `build_pipeline.md` registry, updated in the same change). Loader exposes texture data + channel table. Compiler tests: mask matches the lightmap's own shadowing for a single-light fixture; channel collision and overflow fixtures.

### Task 3: Forward union term

Renderer: upload the mask atlas (one new sampled texture in the lightmap group; update the budget guard counts) and the per-selected-light channel table. Extend the per-frame promoted-set upload (from the parent plan) so the forward pass can iterate promoted lights: for each, read `baked_vis` from the mask channel, `shadow_map_vis` from the light's pool slot (spot or cube sampler, existing helpers in `shadow_sample.wgsl`), reconstruct the light's direct term from `spec_lights` with the Task 1 formula, and subtract `direct × max(0, baked_vis − shadow_map_vis) × w`. Clamp the running lit result at zero. Loop bound is the promoted count (small), influence-volume early-out first. Wire the promoted-set buffer into the forward bind groups without exceeding per-stage storage-buffer budgets — if a new buffer doesn't fit, fold the promoted entries into an existing group-2 buffer's tail with a count uniform. Pin shader/CPU byte layout with the existing budget/layout test pattern.

## Sequencing

**Phase 1 (sequential):** Task 1 — go/no-go gate; blocks everything.
**Phase 2 (sequential):** Task 2 — bake + section; Task 3 consumes its channel table and atlas.
**Phase 3 (sequential):** Task 3.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Shadowmask atlas | `ShadowmaskAtlasSection` | PRL section, new id | n/a |

No authoring surface: masks bake automatically for the parent plan's selected lights.

## Wire format

- **ShadowmaskAtlas**: little-endian. Header: atlas width, height (`u32` each, must equal the lightmap atlas dimensions), selected-light count `u32`, then per selected light one `u8` channel index (0–3, `0xFF` = dropped/no mask), padded to 4-byte alignment; then `width × height × 4` bytes `Rgba8Unorm` texel payload (255 = fully visible). Layer/array handling mirrors the lightmap atlas sectioning (one mask layer per lightmap layer). Section omitted when the selection is empty.

## Open questions

- Penumbra mismatch: baked visibility is soft area-light penumbra, the pool map is hard PCF — inside a baked penumbra the max() can under- or over-darken slightly. Task 1 measures whether it reads acceptably; if not, the fallback is biasing `shadow_map_vis` toward the baked ramp.
- Whether billboards should also receive the union on their static specular term — deferred until the mesh/world result is judged.
