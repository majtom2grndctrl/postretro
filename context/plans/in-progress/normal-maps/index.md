---
name: Normal Maps
description: Wire normal-map sampling and bumped-Lambert correction into the forward pass; the bake-time and vertex-stream halves already exist as stubs awaiting the runtime fill-in.
type: project
---

# Normal Maps

## Goal
Light world surfaces with per-fragment normals perturbed by tangent-space normal maps. Tangents are already baked into the vertex format and propagated through the vertex shader; the forward fragment pass currently ignores them. This plan finishes the path: load `_n.png` siblings, bind them per material, perturb the normal in the fragment shader, and reapply the bumped-Lambert correction so static lightmaps respond to normal-map detail.

## Scope

### In scope
- Texture loader scans for `<name>_n.png` siblings; missing maps fall back to a shared neutral-normal placeholder (1×1, encodes +Z).
- Runtime sampling formats are fixed by role: diffuse stays `Rgba8UnormSrgb`, `_n.png` loads as `Rgba8Unorm` (linear), `_s.png` loads as `R8Unorm` (linear). `prl-build` validates PNG color-space metadata against these roles at compile time.
- Material bind group (group 1) gains a normal-map texture binding. Existing material sampler is reused.
- Forward fragment shader samples the normal map, builds the TBN from the interpolated tangent / bitangent_sign / mesh normal, and produces a world-space `N_bump` used for direct, indirect, and specular evaluation.
- Bumped-Lambert correction (documented in §4 but absent from the current shader) implemented for the static lightmap term using the dominant-direction texture already bound on group 4.
- SH indirect lookup (`sample_sh_indirect`) called with the bumped normal.
- Dynamic light loop and specular term consume the bumped normal.
- Lighting isolation modes continue to work; no new diagnostic mode required.
- The depth pre-pass remains vertex-only and is untouched.

### Out of scope
- PBR / metallic-roughness materials — albedo + normal + spec stays the full vocabulary.
- Per-pixel parallax, height maps, or POM.
- Re-baking the static lightmap atlas with normal maps applied — bake stays mesh-normal; runtime correction handles the difference.
- Re-baking the SH irradiance volume against normal maps — runtime samples with `N_bump` only.
- Authoring tools or a CLI for generating normal maps from height maps.
- Hot-reload of normal maps mid-level.
- Format negotiation beyond color-space validation: encoding choice between Rgba8Unorm two-channel-octahedral and tangent-space RGB is settled in implementation, not negotiated per asset.
- Transcoding or auto-fixing mis-tagged PNGs in `prl-build` — the validator reports, content authors re-export.
- Animated lightmap normal-map correction — requires a separate dominant-direction channel for the animated atlas; deferred to a follow-up plan.

## Acceptance criteria
- [ ] Loading a level with a material whose diffuse is `wall.png` and which has a sibling `wall_n.png` produces visibly normal-mapped shading on that surface; toggling the file off (rename the sibling) falls back to flat shading without warnings beyond the standard "no normal map" debug log.
- [ ] A test map with a single static `light` and a high-frequency normal map shows direction-correlated highlights and shadows on the bumped surface. Rebaking with the light entity at a different position produces correspondingly shifted highlights and shadows on the bumped surface.
- [ ] The same surface lit only by SH indirect shows the normal-map response (top of bumps brighter than crevices). Test setup: the test map is compiled with a `light_sun` entity, which produces a sky-dominant SH probe set after baking. This is a constraint on the test map, not a runtime requirement.
- [ ] Dynamic lights produce normal-mapped highlights at runtime.
- [ ] Specular highlights (existing `_s.png` path) follow the perturbed normal, not the mesh normal.
- [ ] Surfaces with no `_n.png` render identically to today's mesh-normal path (within float tolerance) — the neutral placeholder must be a true no-op.
- [ ] `prl-build` fails the compile when an `_n.png` or `_s.png` sibling is flagged as sRGB in its PNG metadata. The error names the offending file and the expected color space.
- [ ] `RUST_LOG=info` reports normal-map texture loads alongside existing diffuse/spec loads; missing siblings emit at most one log line per material at level load.
- [ ] No regressions in `cargo test -p postretro -p postretro-level-compiler -p postretro-level-format`.
- [ ] `assets/maps/test.prl` runs without validation errors at all existing 9 lighting-isolation modes.

## Tasks

### Task 1: Loader and material binding
Add a normal-map scan to the texture loader: for each material, look for `<diffuse_stem>_n.png` in the same collection directory. On hit, decode and upload as the material's normal map with `Rgba8Unorm` (linear); on miss fall back to the shared neutral placeholder. A dimension mismatch with the diffuse texture logs a warning and falls back to the placeholder, matching the `_s.png` behavior. A corrupt or undecodable `_n.png` logs an error and falls back to the placeholder, matching the pattern of other texture loads. Allocate the placeholder once at engine start alongside the existing checkerboard / black-spec placeholders; it is engine-lifetime and survives level unload. Extend the material bind group layout with a normal-map binding and update bind-group creation for every material. The existing material sampler is reused.

The placeholder must encode "tangent-space +Z" in whichever decoding scheme Task 3 chooses. Coordinate concretely with Task 3 on the encoding before writing pixel bytes.

A defense-in-depth load-time sRGB check is unnecessary: Task 2 catches mis-tagged PNGs at compile time, and engine-loaded `.prl` archives only ever contain build-validated assets. The loader trusts the format choice it makes per role.

### Task 2: prl-build color-space validation
Extend `prl-build`'s existing PNG-reading pass to inspect color-space metadata for each texture sibling. The `png` crate (the lower-level decoder underlying `image`) exposes the PNG `sRGB` and `gAMA` chunks on its `Info` struct; read them without re-decoding pixel data. The `image` crate does not surface these chunks through its public API, so this task likely requires adding a direct `png` crate dependency (or accessing `image`'s underlying PNG decoder where exposed). Apply per-role rules:

| Suffix | Required color space | Action on violation |
|---|---|---|
| `_n.png` | Linear (no sRGB chunk, or gAMA ≈ 1.0) | Fail build |
| `_s.png` | Linear | Fail build |
| (diffuse, no suffix) | sRGB | No check — diffuse is sampled as `Rgba8UnormSrgb` and authored in sRGB |

The error message names the offending path, the detected color space, and the required color space. A misconfigured `_n.png` is the worst case: sRGB gamma applied to raw XYZ normals shifts directions non-linearly and silently breaks shading. Failing the build turns the silent bug into a clear diagnostic.

**Prerequisite — asset audit before activation.** Most image tools default to sRGB on PNG export, so existing `_s.png` siblings in the repo are likely sRGB-tagged and would fail the new check the moment it lands. Before activating the validator, walk the asset tree, inspect every existing `_n.png` and `_s.png` for color-space metadata, and re-export any that are sRGB-tagged as linear. `tools/gen_specular.py` (see `resource_management.md` §4.2) is the primary `_s.png` generator; verify whether it writes linear PNGs and, if not, fix the generator and regenerate its outputs in the same pass. Update §4.2 to state the linear-output guarantee once confirmed. The validator and the corrected assets land in the same commit so the tree is green at every revision.

This task ships independently of the runtime work — running it against the current asset tree validates existing content before any new normal maps land.

### Task 3: Fragment shader normal perturbation
Sample the normal map at `in.uv`, decode to a tangent-space vector, build the TBN from `in.world_tangent`, `in.world_normal`, and `in.bitangent_sign`, and compute `N_bump = normalize(TBN * n_ts)`. Replace the current `let N = normalize(in.world_normal);` with `N_bump`. All downstream consumers — dynamic light loop, specular accumulation, SH indirect call (`sample_sh_indirect(... , N_bump)`) — use `N_bump`. Remove the stub comments at lines around 428–430 and 487–492 of `forward.wgsl`.

**Degenerate-tangent guard.** Geometry with degenerate UVs decodes to a near-zero tangent vector. Normalizing zero produces NaN, and a single NaN propagates through every downstream lighting term on the affected fragment. Before TBN construction, test the decoded tangent length against the same `EPS` used by Task 4's bumped-Lambert correction. When the tangent is below threshold, skip the TBN entirely and set `N_bump = in.world_normal` (already unit-length from the vertex stage, modulo interpolation drift — renormalize in this branch only). The placeholder neutral normal must round-trip through the non-degenerate branch unchanged so untextured surfaces match the mesh-normal path bit-for-bit.

Pick one normal-map encoding (tangent-space RGB in Rgba8Unorm with `xyz = sample.rgb * 2 - 1`, reconstructing z from `sqrt(1 - x²-y²)` if a two-channel scheme is preferred) and document the choice in `resource_management.md` §4.3 at promotion. The placeholder texel from Task 1 must round-trip through this decode to `(0, 0, 1)`.

### Task 4: Bumped-Lambert correction for static lightmap
Implement the bumped-Lambert correction (documented in §4 but absent from the current shader) for the static lightmap term. Sample the dominant-direction texture (group 4) using `decode_lightmap_direction` (already present in the shader). The baked irradiance carries the mesh-normal NdotL pre-multiplied. Divide that out and remultiply with the bumped NdotL, guarding the divide against the zero / negative cases:

```
let dom = decode_lightmap_direction(textureSample(lightmap_direction, lightmap_sampler, in.lightmap_uv));
let n_dot_l_mesh = max(dot(in.world_normal, dom), 0.0);
let n_dot_l_bump = max(dot(N_bump, dom), 0.0);
let scale = select(0.0, n_dot_l_bump / max(n_dot_l_mesh, EPS), n_dot_l_mesh > EPS);
let scale_capped = min(scale, 4.0);
static_direct_corrected = lm_irr * scale_capped + lm_anim;  // animated atlas stays uncorrected for now
```

Choose `EPS` so the correction degrades gracefully on grazing texels; document the chosen value inline. Start with `EPS = 1e-3` and tune if grazing artifacts appear. Cap the scale at 4.0: when `N_bump` points toward the dominant light on a near-backfacing surface, `n_dot_l_mesh` is near zero while `n_dot_l_bump` is not, producing an unbounded brightness spike. A cap of 4.0 matches what a heavily-bumped surface on near-grazing geometry can physically receive. The animated lightmap atlas (`lm_anim`) is not corrected in this plan — it is pre-shaded against the mesh normal and correction is deferred (see Out of scope).

### Task 5: Documentation and content
Update `resource_management.md` §4.3: remove the `> Not yet implemented.` marker, add the encoding/placeholder/color-space contract (encoding scheme, neutral-placeholder round-trip guarantee, per-role color-space rules enforced by `prl-build`). Update the §3 loader text to reflect normal-map scanning alongside diffuse and `_s.png` loads. Author or commit one example normal map alongside an existing test texture so `assets/maps/test.prl` exercises the full path on `cargo run`.

**Acceptance-criterion alignment.** AC-3 requires a sky-dominant SH probe set, which only materializes when the source map for `assets/maps/test.prl` contains a `light_sun` entity. Inspect the source map: if `light_sun` is already present, note its presence in the task's commit message and move on; if absent, add one and recompile `test.prl` in the same change. The bumped surface from this task must sit in a leaf reached by that sun's SH contribution so AC-3 can be observed without authoring a second map.

## Sequencing

**Phase 1 (concurrent):** Task 1 and Task 2. Task 1's bind-group layout change is consumed by every later runtime task; resolve the placeholder encoding contract with Task 3 before writing pixel bytes. Task 2 edits `prl-build` only and shares no files with Task 1, so the two run in parallel.
**Phase 2 (sequential):** Task 3, then Task 4. Both edit `forward.wgsl`, and Task 4 consumes `N_bump` — the symbol Task 3 introduces. They may be drafted in parallel, but Task 4 cannot compile until Task 3's binding lands, so merge in order: Task 3 first, then Task 4.
**Phase 3 (sequential):** Task 5 — docs and content reflect the shipped behavior.

## Rough sketch
- Loader entry: `postretro/src/render/` material/texture loading. Add a sibling-scan helper next to the existing diffuse / `_s.png` paths.
- Bind group 1 grows by one binding; pipeline-layout creation in `render/mod.rs` updates accordingly.
- Shader edits are confined to `postretro/src/shaders/forward.wgsl`. Vertex stage already emits `world_tangent` (location 2) and `bitangent_sign` (location 3) as `VertexOutput` fields — `bitangent_sign` is unpacked from the tangent channel's sign bit in the vertex `main`. No vertex-side change.
- `sample_sh_indirect` signature already takes a normal — pass `N_bump` at the call site, no helper change.
- The existing dominant-direction texture is bound on group 4 binding 1; the bumped-Lambert correction reuses that sampler. The binding is named `lightmap_direction`.
