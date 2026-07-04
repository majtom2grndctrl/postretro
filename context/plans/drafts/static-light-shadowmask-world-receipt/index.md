# Static-Light Shadowmask — Entity Shadows onto World Surfaces

> **Depends on:** `static-light-entity-shadows` (promotion machinery, selected-light set, promoted-set contract, weights, pool slots). Do not start before that plan ships.

## Goal

Complete the static-light entity-shadow picture: an entity standing under a promoted static light casts a crisp shadow onto world geometry. World direct lighting stays fully baked — the runtime never re-shadows static geometry onto static geometry; a per-light baked occlusion mask union-combined with the pool shadow map darkens only what runtime occluders newly block. The reconstructed per-light direct term exists solely as a subtraction magnitude — world direct light is never a runtime-added term.

## Scope

### In scope

- Per-light baked occlusion masks for selected lights (lightmap-space, RGBA channels, ≤ 4 overlapping selected lights per texel via channel assignment).
- Forward-pass union term: per promoted light, subtract the light's reconstructed direct contribution × `max(0, baked_visibility − shadow_map_visibility) × w`. In the fully-lit and fully-shadowed limits this is nonzero only where a runtime occluder blocks a texel the bake left lit; inside baked soft penumbrae the soft-vs-hard-PCF mismatch can leave a residual — driving that residual to ~zero with no entity present is a hard gate condition (Task 1), with `shadow_map_vis` biasing toward the baked ramp as the committed remedy.
- A prototype gate before the bake work: validate the union subtraction against the bumped-Lambert directional lightmap reconstruction on a fixture map, including the entity-absent penumbra case.

### Out of scope

- Everything the parent plan ships (promotion, entity receipt, SH LOD handoff, the promoted-set contract).
- Runtime static→static shadows (explicit non-goal — the lightmap owns them; the Task 1 penumbra gate is the enforcement).
- Lightmap array-consolidation refactor (texture budget fits without it; consolidation remains the banked fallback if a future feature needs the headroom).
- More than 4 overlapping selected lights per texel (compiler warns and drops the lowest-intensity mask assignment).
- Dynamic-light interaction changes (dynamic lights keep their existing multiply-in-loop shadowing).

## Acceptance criteria

Tags name the producing task(s).

- [ ] (T1) Prototype gate passes both conditions on a fixture map: (a) the union term darkens an entity-shaped region under a selected light with no visible ringing or direction artifacts from the bumped-Lambert reconstruction, and overlapping baked + entity shadow shows no double-darkening; (b) with the promoted light active and NO entity present, world output over a baked soft penumbra shows no perceptible change (~zero union residual) — if the raw max() fails (b), the ramp-bias remedy must pass it before the gate counts as passed. A failed gate stops the plan and the failure mode is written back into this spec; a passed gate writes the matched reconstruction formula into Task 3's paragraph before Task 3 starts.
- [ ] (T2) Compiler bakes per-light occlusion masks for selected lights and assigns channels; a fixture with > 4 overlapping selected lights (fixtures authored in T2) compiles with a warning and drops the excess deterministically (compiler tests).
- [ ] (T3) An entity inside a promoted static light's cone casts a visible crisp shadow on floor/wall geometry; the shadow fades with the light's promotion weight `w` (manual verification on `campaign-test.prl`).
- [ ] (T3) Where the entity's shadow crosses a baked static shadow, the darker of the two wins — no additive darkening (visual check with isolation modes).
- [ ] (T3) Degradation pinned by tests: a promoted light whose mask channel is `0xFF` (dropped) contributes no union term — entity receipt from the parent plan still works; a PRL with a nonempty `EntityShadowLights` section but no `ShadowmaskAtlas` section disables the union term entirely.
- [ ] (T3) With nothing promoted, forward output is unchanged from the parent plan's baseline.
- [ ] (T3) Forward sampled-texture count rises by exactly one (the mask atlas) and stays ≤ 16 on both cube-array variants; the budget guard test pins the new counts.
- [ ] (T2) Lightmap irradiance/direction bake output for existing maps is unchanged (masks are additive data).

## Tasks

### Task 1: Prototype gate — union vs directional lightmap reconstruction

Spike, not shipped code. On a fixture map with one selected spot over a flat lightmapped floor featuring at least one baked soft penumbra: hand-bake (or stub) a per-light visibility value, and in `forward.wgsl` subtract `per_light_direct × max(0, baked_vis − shadow_map_vis)` from the lit result, where `per_light_direct` is reconstructed from the light record (position/color/falloff/cone from `spec_lights`) with the same bumped-Lambert normal response the lightmap term uses. Two gate conditions: (a) entity present — an entity-shaped occluder region darkens correctly, no ringing or direction artifacts, no double-darkening where it overlaps the baked shadow; (b) entity absent — the baked penumbra shows no perceptible net change (the soft-baked-vs-hard-PCF mismatch would otherwise harden the static penumbra, a prohibited runtime static→static shadow). If raw max() fails (b), apply the committed remedy — bias `shadow_map_vis` toward the baked ramp (e.g. blur/widen the comparison kernel for promoted slots) — and re-test; the gate passes only with (a) and (b) both green. Deliverable: go/no-go note in this plan folder with screenshots. On pass, write the matched reconstruction formula (and the ramp-bias parameters if used) into Task 3's paragraph before Task 3 starts; on fail, record the failure mode in this spec and stop — the parent plan stands alone.

### Task 2: Per-light occlusion mask bake + PRL section

Compiler: for each selected light (the `EntityShadowLights` set, SectionId 40 from the parent plan), bake a lightmap-space visibility mask — the same area-sampled `soft_visibility` the lightmap bake computes per light, stored per texel instead of folded into the sum; the per-light layer cache in `lightmap_layer.rs` already isolates per-light contributions, reuse its plumbing. Pack up to 4 selected lights per texel into RGBA channels; channel assignment is per light with a graph-coloring pass over spatial overlap (two selected lights sharing any texel get different channels); on > 4-way overlap, drop the lowest-intensity light's mask with a compile warning (`0xFF` channel sentinel). Emit as PRL section `ShadowmaskAtlas` (SectionId 42, reserved by the parent plan's registry update): atlas dimensions matching the lightmap atlas, `Rgba8Unorm` texel payload, plus a per-selected-light channel table indexed by selection index (position in `EntityShadowLights` order — the parent plan's promoted-set contract carries the same index). Update the SectionId registry in the same change. Loader exposes texture data + channel table. Author test fixtures covering: single-light mask matches the lightmap's own shadowing; channel collision; > 4-way overflow.

### Task 3: Forward union term

Renderer: upload the mask atlas (one new sampled texture in the lightmap group; update the budget guard counts) and the per-selected-light channel table. Build the forward-pass promoted-set upload from the parent plan's published per-frame CPU promoted set (global light index, selection index, pool kind + slot, `w` — the parent publishes a CPU structure and a compose-pass weight buffer, NOT a forward-bindable buffer; creating one is this task's job). Take `w` from the promoted set, not from the light-buffer color (the parent premultiplies `w` into GpuLight color for the mesh path; the union term needs raw `w` and reconstructs direct from `spec_lights`, which is indexed by a compacted baked-tier order — resolve selection index → spec_lights slot once at load and carry it in the uploaded record). For each promoted record: skip if its channel is `0xFF` or the `ShadowmaskAtlas` section is absent (union disabled, entity receipt unaffected); otherwise read `baked_vis` from the mask channel, `shadow_map_vis` from the light's pool slot (spot or cube sampler, existing helpers in `shadow_sample.wgsl`), reconstruct the light's direct term with the formula written into this paragraph by Task 1, and subtract `direct × max(0, baked_vis − shadow_map_vis) × w`. The reconstruction is a subtraction magnitude only — never added to the lit result. Clamp the running lit result at zero. Loop bound is the promoted count (small), influence-volume early-out first. Wire the promoted-set buffer into the forward bind groups without exceeding per-stage storage-buffer budgets — if a new buffer doesn't fit, fold the promoted entries into an existing group-2 buffer's tail with a count uniform. Pin shader/CPU byte layout with the existing budget/layout test pattern.

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

- **ShadowmaskAtlas (42)**: little-endian. Header: atlas width, height (`u32` each, must equal the lightmap atlas dimensions), selected-light count `u32`, then per selected light one `u8` channel index (0–3, `0xFF` = dropped/no mask) in selection order, padded to 4-byte alignment; then `width × height × 4` bytes `Rgba8Unorm` texel payload (255 = fully visible). Layer/array handling mirrors the lightmap atlas sectioning (one mask layer per lightmap layer). Section omitted when the selection is empty.

## Open questions

- The penumbra residual is now a gate condition rather than an open risk; what remains open is only the ramp-bias implementation shape (receiver-side kernel widening vs a baked-ramp-aware compare), decided inside Task 1.
- Whether billboards should also receive the union on their static specular term — deferred until the mesh/world result is judged.
