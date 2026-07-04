# Static-Light Shadowmask — Entity Shadows onto World Surfaces

> **Depends on:** `static-light-entity-shadows` (promotion machinery, selected-light set, promoted-set contract, weights, pool slots). Do not start before that plan ships.

## Goal

Complete the static-light entity-shadow picture: an entity standing under a promoted static light casts a crisp shadow onto world geometry. World direct lighting stays fully baked — the runtime never re-shadows static geometry onto static geometry; a per-light baked occlusion mask union-combined with the pool shadow map darkens only what runtime occluders newly block. The reconstructed per-light direct term exists solely as a subtraction magnitude — world direct light is never a runtime-added term.

## Scope

### In scope

- Per-light baked occlusion masks for selected lights (lightmap-space D2Array atlas, RGBA channels, ≤ 4 overlapping selected lights per texel via channel assignment).
- Forward-pass union term: per promoted light, subtract the light's reconstructed direct contribution × `max(0, baked_visibility − shadow_map_visibility) × w`. In the fully-lit and fully-shadowed limits this is nonzero only where a runtime occluder blocks a texel the bake left lit; inside baked soft penumbrae the soft-vs-hard-PCF mismatch can leave a residual — driving that residual to ~zero with no entity present is a hard gate condition (Task 1), with `shadow_map_vis` biasing toward the baked ramp as the committed remedy.
- A prototype gate before the bake work: validate the union subtraction against the bumped-Lambert directional lightmap reconstruction on a fixture map, including the entity-absent penumbra case.

### Out of scope

- Everything the parent plan ships (promotion, entity receipt, SH LOD handoff, the promoted-set contract).
- Reselecting shadow-pool lights. This plan consumes the parent `EntityShadowLights` set only. Lights authored with `_bake_only 1` have no runtime presence and must never receive mask entries, promoted records, or shadow-pool slots.
- Runtime static→static shadows (explicit non-goal — the lightmap owns them; the Task 1 penumbra gate is the enforcement).
- Lightmap array-consolidation refactor (texture budget fits without it; consolidation remains the banked fallback if a future feature needs the headroom).
- More than 4 overlapping selected lights per texel (compiler warns, drops selected-light masks globally until the overlap graph is 4-colorable, and marks dropped selection indices `0xFF`).
- Dynamic-light interaction changes (dynamic lights keep their existing multiply-in-loop shadowing).

## Acceptance criteria

Tags name the producing task(s).

- [x] (T1) Prototype gate passes both conditions on a fixture map: (a) the union term darkens an entity-shaped region under a selected light with no visible ringing or direction artifacts from the bumped-Lambert reconstruction, and overlapping baked + entity shadow shows no double-darkening; (b) with the promoted light active and NO entity present, world output over a baked soft penumbra shows no perceptible change (~zero union residual) — if the raw max() fails (b), the ramp-bias remedy must pass it before the gate counts as passed. A failed gate stops the plan and the failure mode is written back into this spec; a passed gate writes the matched reconstruction formula into Task 3's paragraph before Task 3 starts.
- [x] (T2) Compiler bakes per-light raw `soft_visibility` occlusion masks for selected lights and assigns channels; a fixture with > 4 overlapping selected lights (fixtures authored in T2) compiles with a warning and drops the excess globally and deterministically (compiler tests). Multi-layer lightmap fixtures round-trip `ShadowmaskAtlas` payload sizing and loader exposure.
- [x] (T3) An entity inside a promoted static light's cone casts a visible crisp shadow on floor/wall geometry; the shadow fades with the light's promotion weight `w` (manual verification on `campaign-test.prl`).
- [x] (T3) Where the entity's shadow crosses a baked static shadow, the darker of the two wins — no additive darkening (visual check with a union-term or baked-vs-runtime visibility isolation mode added by T3).
- [x] (T3) Degradation pinned by tests: a promoted light whose mask channel is `0xFF` (dropped) contributes no union term — entity receipt from the parent plan still works; a PRL with valid `EntityShadowLights`, `DirectShVolume`, and `DirectShDeltaVolumes` sections but no `ShadowmaskAtlas` section disables the union term entirely without disabling promotion/entity receipt.
- [x] (T3) With nothing promoted, forward output is unchanged from the parent plan's baseline; a shader/layout test pins `promoted_count = 0` to zero union subtraction and no stale promoted-record reads.
- [x] (T3) Forward sampled-texture count rises by exactly one (the mask atlas) and stays ≤ 16 on both cube-array variants; the budget guard test pins the new counts.
- [x] (T2) Lightmap irradiance/direction bake output for existing maps is unchanged at the pre-BC6H composited seam, or decoded irradiance/direction compare within existing tolerances; mask emission does not mutate Lightmap section fields.

## Tasks

### Task 1: Prototype gate — union vs directional lightmap reconstruction

Spike, not shipped code. On a fixture map with one selected spot over a flat lightmapped floor featuring at least one baked soft penumbra: hand-bake (or stub) a per-light visibility value, and in `forward.wgsl` subtract `per_light_direct × max(0, baked_vis − shadow_map_vis)` from the lit result, where `per_light_direct` is reconstructed from the light record (position/color/falloff/cone from `spec_lights`) with the same bumped-Lambert normal response the lightmap term uses. Two gate conditions: (a) entity present — an entity-shaped occluder region darkens correctly, no ringing or direction artifacts, no double-darkening where it overlaps the baked shadow; (b) entity absent — the baked penumbra shows no perceptible net change (the soft-baked-vs-hard-PCF mismatch would otherwise harden the static penumbra, a prohibited runtime static→static shadow). If raw max() fails (b), apply the committed remedy — bias `shadow_map_vis` toward the baked ramp (e.g. blur/widen the comparison kernel for promoted slots) — and re-test; the gate passes only with (a) and (b) both green. Deliverable: go/no-go note in this plan folder with screenshots. On pass, write the matched reconstruction formula (and the ramp-bias parameters if used) into Task 3's paragraph before Task 3 starts; on fail, record the failure mode in this spec and stop — the parent plan stands alone.

### Task 2: Per-light occlusion mask bake + PRL section

Compiler: for each selected light (the `EntityShadowLights` set, SectionId 40 from the parent plan), bake a lightmap-space visibility mask: the same area-sampled raw `soft_visibility` the lightmap bake computes per light, stored per texel instead of folded into irradiance. Do not reselect lights from `AlphaLights`; `_bake_only 1` lights have no runtime presence and must not gain mask entries. Do not treat the existing per-light layer output as raw visibility; add a sibling raw-visibility mask bake, or extend/cache-bump layer data to carry `soft_visibility` separately, while reusing chart placement, texel seed, segment-clear, and soft-visibility plumbing. Pack up to 4 selected lights per texel into RGBA channels. Channel assignment is per selected light with a graph-coloring pass over spatial overlap: two selected lights sharing any texel get different channels, and a selected light uses one channel across all atlas texels. On > 4-way overlap, drop selected-light masks globally, lowest-intensity first with deterministic tie-breaks, until the graph is 4-colorable; dropped selection indices get the `0xFF` channel sentinel and write no atlas contribution. Emit as PRL section `ShadowmaskAtlas` (SectionId 42): atlas dimensions and `layer_count` matching the lightmap irradiance atlas, `Rgba8Unorm` texel payload (`layer_count × width × height × 4` bytes), plus a per-selected-light channel table indexed by selection index (position in `EntityShadowLights` order — the parent plan's promoted-set contract carries the same index). Add `ShadowmaskAtlas = 42` to the SectionId registry, `from_u32(42)`, and ID pin tests in the same change. Loader exposes texture data, layer count, and channel table. Author test fixtures covering: single-light mask matches raw `soft_visibility` on the same texels; channel collision; > 4-way global overflow; `layer_count > 1` round-trip and loader exposure.

### Task 3: Forward union term

Renderer: upload the mask atlas as one new `texture_2d_array<f32>` sampled texture in the lightmap group, sampled with `in.lightmap_layer`; use a 1×1×1 fully-visible dummy texture when the section is absent. Update the budget guard counts. Upload the per-selected-light channel table. Build the forward-pass promoted-set upload from the parent plan's published per-frame CPU promoted set (global light index, selection index, pool kind + slot, `w` — the parent publishes a CPU structure and a compose-pass weight buffer, NOT a forward-bindable buffer; creating one is this task's job). Take `w` from the promoted set, not from the light-buffer color (the parent premultiplies `w` into GpuLight color for the mesh path; the union term needs raw `w` and reconstructs direct from `spec_lights`). `spec_lights` is indexed by compacted baked-tier order: build `selection_index → spec_lights_index` from the full AlphaLights list by counting prior non-dynamic lights, never by using `global_light_index` directly; carry that index in the uploaded record and pin it with a test. For each promoted record: skip if its channel is `0xFF` or the `ShadowmaskAtlas` section is absent (union disabled, entity receipt unaffected); otherwise read `baked_vis` from the mask channel at the fragment's lightmap UV and array layer, and read `shadow_map_vis` from the light's pool slot (spot or cube sampler, existing helpers in `shadow_sample.wgsl`). Reconstruct the light's direct term as `L = normalize(light_position - world_position)`, `atten = max(1 - distance / range, 0)`, `cone = smoothstep(cos_outer, cos_inner, dot(-L, cone_axis))`, `direct_mesh = light_color_intensity × atten × cone × max(dot(mesh_normal, L), 0)`, `bump_scale = min(max(dot(bump_normal, L), 0) / max(max(dot(mesh_normal, L), 0), 0.01), 4)`, and `direct = direct_mesh × bump_scale`. Subtract `direct × max(0, baked_vis − shadow_map_vis) × w`. For promoted spot slots, bias `shadow_map_vis` toward the baked ramp with a widened receiver-side PCF kernel: 5×5 taps at 2.0 shadow-map texel spacing. Point/cube promoted slots keep the existing cube PCF unless Task 3 visual checks show they need separate tuning. The reconstruction is a subtraction magnitude only — never added to the lit result. Clamp the running lit result at zero. Loop bound is the promoted count (small), influence-volume early-out first. Do not add a new fragment-visible storage buffer unless the budget test proves it fits; expected path is folding promoted entries into an existing group-2 buffer's tail with a count uniform. Pin shader/CPU byte layout with the existing budget/layout test pattern. Add a dev-tools isolation or visualization mode for the union term or baked-vs-runtime visibility so darker-wins checks are repeatable.

## Sequencing

**Phase 1 (sequential):** Task 1 — go/no-go gate; blocks everything; on pass it amends Task 3's paragraph.
**Phase 2 (sequential):** Task 2 — bake + section; Task 3 consumes its channel table and atlas.
**Phase 3 (sequential):** Task 3.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Shadowmask atlas | `ShadowmaskAtlasSection` | PRL SectionId 42 | n/a |

No authoring surface: masks bake automatically for the parent plan's selected lights.

## Wire format

- **ShadowmaskAtlas (42)**: little-endian. Header: atlas width, height, `layer_count`, selected-light count (`u32` each; width/height/layer count must equal the lightmap irradiance atlas dimensions), then per selected light one `u8` channel index (0–3, `0xFF` = dropped/no mask) in selection order, padded to 4-byte alignment; then `layer_count × width × height × 4` bytes `Rgba8Unorm` texel payload (255 = fully visible). Payload is a D2Array atlas sampled with the world fragment's `lightmap_layer`. Section omitted when the selection is empty.

## Open questions

- The penumbra residual is now a gate condition rather than an open risk; what remains open is only the ramp-bias implementation shape (receiver-side kernel widening vs a baked-ramp-aware compare), decided inside Task 1.
- Whether billboards should also receive the union on their static specular term — deferred until the mesh/world result is judged.
