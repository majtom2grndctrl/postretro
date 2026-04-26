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
- Material bind group (group 1) gains a normal-map texture binding. Existing material sampler is reused.
- Forward fragment shader samples the normal map, builds the TBN from the interpolated tangent / bitangent_sign / mesh normal, and produces a world-space `N_bump` used for direct, indirect, and specular evaluation.
- Bumped-Lambert correction reinstated for the static lightmap term using the dominant-direction texture already bound on group 4.
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
- Format negotiation: only Rgba8Unorm two-channel-octahedral *or* tangent-space RGB — pick one in the implementation; spec only constrains sampler behavior.

## Acceptance criteria
- [ ] Loading a level with a material whose diffuse is `wall.png` and which has a sibling `wall_n.png` produces visibly normal-mapped shading on that surface; toggling the file off (rename the sibling) falls back to flat shading without warnings beyond the standard "no normal map" debug log.
- [ ] A test map with a single static `light` and a high-frequency normal map shows direction-correlated highlights and shadows on the bumped surface that move correctly as the light entity is moved at compile time.
- [ ] The same surface lit only by SH indirect shows the normal-map response (top of bumps brighter than crevices for a sky-dominant probe set).
- [ ] Dynamic lights produce normal-mapped highlights at runtime.
- [ ] Specular highlights (existing `_s.png` path) follow the perturbed normal, not the mesh normal.
- [ ] Surfaces with no `_n.png` render identically to today's mesh-normal path (within float tolerance) — the neutral placeholder must be a true no-op.
- [ ] `RUST_LOG=info` reports normal-map texture loads alongside existing diffuse/spec loads; missing siblings emit at most one log line per material at level load.
- [ ] No regressions in `cargo test -p postretro -p postretro-level-compiler -p postretro-level-format`.
- [ ] `assets/maps/test.prl` runs without validation errors at all 9 lighting-isolation modes.

## Tasks

### Task 1: Loader and material binding
Add a normal-map scan to the texture loader: for each material, look for `<diffuse_stem>_n.png` in the same collection directory. On hit, decode and upload as the material's normal map; on miss (or dimension mismatch with diffuse) fall back to the shared neutral placeholder. Allocate the placeholder once at renderer init alongside the existing checkerboard / black-spec placeholders. Extend the material bind group layout with a normal-map binding and update bind-group creation for every material. The existing material sampler is reused.

The placeholder must encode "tangent-space +Z" in whichever decoding scheme Task 2 chooses. Coordinate concretely with Task 2 on the encoding before writing pixel bytes.

### Task 2: Fragment shader normal perturbation
Sample the normal map at `in.uv`, decode to a tangent-space vector, build the TBN from `in.world_tangent`, `in.world_normal`, and `in.bitangent_sign`, and compute `N_bump = normalize(TBN * n_ts)`. Replace the current `let N = normalize(in.world_normal);` with `N_bump`. All downstream consumers — dynamic light loop, specular accumulation, SH indirect call (`sample_sh_indirect(... , N_bump)`) — use `N_bump`. Remove the stub comments at lines around 428–430 and 487–492 of `forward.wgsl`.

Pick one normal-map encoding (tangent-space RGB in Rgba8Unorm with `xyz = sample.rgb * 2 - 1`, reconstructing z from `sqrt(1 - x²-y²)` if a two-channel scheme is preferred) and document the choice in `resource_management.md` §4.3 at promotion. The placeholder texel from Task 1 must round-trip through this decode to `(0, 0, 1)`.

### Task 3: Bumped-Lambert correction for static lightmap
Restore the correction for the static lightmap term. Sample the dominant-direction texture (group 4) using `decode_lightmap_direction` (already present in the shader). The baked irradiance carries the mesh-normal NdotL pre-multiplied. Divide that out and remultiply with the bumped NdotL, guarding the divide against the zero / negative cases:

```
let dom = decode_lightmap_direction(textureSample(lightmap_dir, lightmap_sampler, in.lightmap_uv));
let n_dot_l_mesh = max(dot(in.world_normal, dom), 0.0);
let n_dot_l_bump = max(dot(N_bump, dom), 0.0);
let scale = select(0.0, n_dot_l_bump / max(n_dot_l_mesh, EPS), n_dot_l_mesh > EPS);
static_direct_corrected = lm_irr * scale + lm_anim;  // animated atlas stays uncorrected for now
```

Choose `EPS` so the correction degrades gracefully on grazing texels; document the chosen value inline. The animated lightmap atlas (`lm_anim`) is not corrected in this plan — it is pre-shaded against the mesh normal and any correction is a separate decision (see Open questions).

### Task 4: Documentation and content
Update `rendering_pipeline.md` §3 (loader) and §7.3 (fragment behavior) to describe the live path rather than the stub. Update `resource_management.md` §4.3 to drop the "Not yet implemented" marker, name the encoding, and state the neutral-placeholder contract. Author or commit one example normal map alongside an existing test texture so `assets/maps/test.prl` (or a tiny sibling map) exercises the full path on `cargo run`.

## Sequencing

**Phase 1 (sequential):** Task 1 — bind-group layout change is consumed by every later task. Task 1 and Task 2 must agree on the placeholder's encoding before Task 1 writes pixel bytes; resolve that contract first, then implement Task 1.
**Phase 2 (concurrent):** Task 2, Task 3 — both edit `forward.wgsl` but at distinct sites (Task 2 owns `let N = ...` and downstream substitutions; Task 3 owns the `static_direct` block). If file-merge churn appears, run them sequentially in this order.
**Phase 3 (sequential):** Task 4 — docs and content reflect the shipped behavior.

## Rough sketch
- Loader entry: `postretro/src/render/` material/texture loading. Add a sibling-scan helper next to the existing diffuse / `_s.png` paths.
- Bind group 1 grows from 4 to 5 bindings; pipeline-layout creation in `render/mod.rs` updates accordingly.
- Shader edits are confined to `postretro/src/shaders/forward.wgsl`. Vertex stage already emits `world_tangent` and `bitangent_sign` — no vertex-side change.
- `sample_sh_indirect` signature already takes a normal — pass `N_bump` at the call site, no helper change.
- The existing dominant-direction texture is bound on group 4 binding 1 (or wherever `lightmap_dir` lives today); the bumped-Lambert correction reuses that sampler. Confirm the binding name during Task 3.

## Open questions
- **Encoding choice.** Tangent-space RGB (3-channel) is simpler and matches the documented `_n.png` "tangent-space RGB" convention in `resource_management.md` §4.3. Two-channel reconstructed-Z saves storage but costs a `sqrt`. Default to RGB unless storage budget forces otherwise; decide in Task 2 and bake into Task 1's placeholder bytes.
- **Animated lightmap correction.** The animated atlas is pre-shaded with the mesh normal and would need an analogous dominant-direction channel to receive the same correction. Not in scope — note as a follow-up if the visual mismatch is noticeable on animated-lit bumped surfaces.
- **Texture color space.** Diffuse PNGs are currently sampled as sRGB. Normal maps must be sampled as linear (`Rgba8Unorm`, not `Rgba8UnormSrgb`). Confirm the loader path can pick the format per role.
- **Bumped-Lambert grazing artifacts.** Dividing by `n_dot_l_mesh` near zero amplifies noise. The `EPS` guard is the simple defense; if it produces visible seams at grazing angles, fall back to a clamped lerp toward zero. Resolve during implementation, not now.
