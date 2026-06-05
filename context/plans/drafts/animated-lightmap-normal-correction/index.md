# Animated Lightmap Normal-Map Correction

## Goal

Apply the same bumped-Lambert normal-map correction to the animated lightmap
term (`lm_anim`) that the static term (`lm_irr`) already receives. On
normal-mapped surfaces, `style`-animated lights are currently up to 4× dimmer
than equivalent static lights because only the static term responds to
normal-map detail. This closes the deferred follow-up from
`context/plans/done/normal-maps` (§Out of scope, plan lines 34/88/91).

The per-texel incoming-light direction this needs is **already baked, already in
the wire format (v2), and already on the GPU** — it was baked for a since-removed
SDF trace and left unread. This plan re-consumes it. See `research.md`.

## Scope

### In scope

- Fuse the existing per-texel-light baked directions into a per-texel dominant
  direction in the animated-lightmap compose pass, weighted by each light's
  current per-frame radiance.
- Write that dominant direction into a new runtime animated-direction atlas
  (compute storage output, sized and dispatched like the existing animated
  irradiance atlas).
- Bind the new atlas through the group-4 lightmap BGL, with a zero fallback for
  maps that have no animated weight maps.
- Apply the bumped-Lambert correction to `lm_anim` in the forward pass using the
  fused direction, reusing the static path's grazing-angle guard and 4.0 cap.

### Out of scope (non-goals)

- **PRL format / bake / version changes.** The direction is already baked
  (`direction_oct`, section v2) and uploaded (`direction_oct_packed`). No
  re-bake, no `ANIMATED_LIGHT_WEIGHT_MAPS_VERSION` bump.
- **The `ANIMATED_ATLAS_SIZE = 1024` vs static-atlas size mismatch.** Separate
  pre-existing latent bug; the new direction atlas mirrors whatever the existing
  animated irradiance atlas does.
- **Per-light directional correction.** One fused dominant direction per texel,
  matching the static lightmap's single-direction model — not per-light.
- **SDF/dynamic-light or shadow-map shading.** Those carry their own shadowing
  and are untouched.

## Acceptance criteria

Verification split: the BGL-contract and `cargo build`/`cargo test` criteria are
machine-checked. **The visual criteria — bump response, direction-sense, A/B
parity, grazing/cap behavior, and the no-op regression — are manual
run-the-engine checks** (this project verifies rendered pixels by running the
engine, not in CI). Exercise them on a test map with a single `style=2` animated
light over a normal-mapped surface, A/B'd against an equivalent no-style static
light. A green `cargo test` alone does not satisfy the visual criteria.

- [ ] On a normal-mapped surface lit by a single `style=2` animated light, the
      animated direct term varies with normal-map detail (bump-corrected),
      visibly matching how the same surface responds under an equivalent no-style
      static light.
- [ ] **Direction-sense:** a single off-axis `style=2` light produces a bump
      highlight biased toward the light — as the light moves around the surface,
      the brightened normal-map facets track its direction. This proves the fused
      direction is correct, not merely non-zero (a directionally-wrong fusion can
      still pass the brightness ACs below).
- [ ] A/B parity: a `style=2` light and a no-style light of equal color and
      intensity on the same normal-mapped surface produce comparable peak
      brightness — peak within ~15% at the curve's maximum (the styled light no
      longer reads as multiples dimmer). The animated light still pulses on its
      curve.
- [ ] The animated correction uses the same grazing-angle floor and maximum
      brightness cap as the static term; neither term can spike unbounded on
      near-backfacing geometry. (Verifiable by inspection: the shared `NDOTL_EPS`
      and 4.0 cap constants.)
- [ ] Existing v2 PRLs load and render with no re-bake. No new section, no
      version change.
- [ ] A map with no animated weight maps renders identically to before (capture
      a before/after frame of the same view): the new atlas binds the
      zero-fallback view. A map that has animated weight maps but where `lm_anim`
      is zero this frame hits the Task 3 forward gate (the `lm_anim`-magnitude
      gate makes the correction a no-op regardless of the atlas direction
      content). Both render identically to before.
- [ ] The group-4 bind-group-layout contract test covers the new binding.
- [ ] `cargo build -p postretro` and `cargo test` pass.

## Tasks

### Task 1: Fuse and emit the animated dominant-direction atlas

In the compose pass (`animated_lightmap_compose.wgsl` +
`crates/postretro/src/render/animated_lightmap.rs`): create a second
`Rgba16Float` storage atlas (name it `animated_lm_direction_atlas`) the same
size as the irradiance atlas, add its
compose-side storage binding at binding 8, and expose a forward view plus a new 1×1
zero `dummy_view` for the empty-map path, modeled on the irradiance atlas's
`dummy_view` (Task 2 binds it). Adding binding 8 requires: growing `compute_bgl_entries` from
`[...; 8]` to `[...; 9]` (animated_lightmap.rs:519) and adding the 9th compose
bind-group entry (animated_lightmap.rs:348-381); deleting/rewriting the
`compose_shader_has_no_dominant_direction_atlas` regression test
(animated_lightmap.rs:1089-1105) and the "intentionally absent" comment
(animated_lightmap_compose.wgsl:101-104) that currently assert the slot stays
empty; and creating the second storage view + atlas texture with a second
`forward_view`. (That test asserts the absence of `@group(1) @binding(8)`,
`encode_oct_to_rg`, and `animated_lm_direction_atlas`. The first and third now
must exist; the compose writes a **raw normalized vec3** — there is no oct
*encode* on the compose side — so drop the `encode_oct_to_rg` assertion rather
than inverting it.) In `compose_main`, for each contributing
texel light, decode that light's `entry.direction_oct_packed` (the same
per-texel-light `entry` already read for the irradiance accum) to a unit vector and accumulate it
weighted by that light's current radiance contribution (the luminance of the
same `c * b * entry.weight` that drives the irradiance accum; see Rough sketch).
Compute the sum's
length; if it is below `1e-4` (opposing lights cancel, or no coverage) store
zero, else store `sum / length` (test length **before** normalizing to avoid a
divide-by-zero). Uncovered texels stay zero (atlas zero-init).

### Task 2: Wire the atlas through the group-4 BGL

In `crates/postretro/src/lighting/lightmap.rs`: add a new group-4 texture
binding (next free slot, 5) for the animated-direction atlas as a non-filterable
float texture (sampled through the existing nearest sampler at binding 2 —
directions must not be linearly interpolated). The existing group-4 BGL slots
are: 0 irradiance, 1 direction, 2 nearest sampler, 3 animated atlas, 4 linear
sampler; the new animated-direction is slot 5. Note that this group-4 binding
(5) and the compose-side storage binding (8) are independent numbering spaces
for the same atlas — not a contradiction. Bind the compose output view, and bind
the Task 1 direction-atlas `dummy_view` in the no-animated-weight-maps path (the
same role the animated irradiance atlas's dummy view plays). Grow and update the
BGL contract test.

### Task 3: Correct `lm_anim` in the forward pass

In `forward.wgsl`: declare the new `@group(4) @binding(5)` direction texture.
Sample it at `lightmap_uv` using the same sample call/intrinsic as the static
direction atlas — the animated direction atlas stores a raw normalized vec3, so
read `.xyz` and re-normalize; do NOT pass the sample through
`decode_lightmap_direction` (that oct decode is only for the static atlas).
Derive the bumped/mesh NdotL ratio against the fused animated direction,
reusing the same `mesh_n` and `N_bump` already in scope (only the direction
vector differs), and build a `scale_anim` with the same `NDOTL_EPS` guard and
4.0 cap as the static `scale`. Mirror the static `use_correction` gate exactly
(forward.wgsl:721-722): `dot(lm_anim, lm_anim) >= LM_ANIM_EPS * LM_ANIM_EPS &&
n_dot_l_mesh_anim > NDOTL_EPS` with `LM_ANIM_EPS = 1e-4` (matching `LM_IRR_EPS`);
when ungated, `lm_anim`'s direction is unreliable so `scale_anim = 1.0`. Combine as `static_direct = lm_irr * scale + lm_anim *
scale_anim`, replacing the assignment at forward.wgsl:732. Update the now-stale
"lm_anim is not corrected" comment at forward.wgsl:707-710.

## Sequencing

**Phase 1 (sequential):** Task 1 — produces the direction atlas + forward view the rest consume.
**Phase 2 (sequential):** Task 2 — binds Task 1's view into the group-4 contract.
**Phase 3 (sequential):** Task 3 — consumes the Task 2 binding.

(Tightly coupled by one shader-binding contract spanning compose + two BGLs +
forward; cleanest as one coherent change even though split for review.)

## Rough sketch

- **Direction encoding in the new atlas.** Store the fused **normalized vec3
  directly** in `.rgb` of the `Rgba16Float` atlas (full precision; no oct
  round-trip needed since the compose writes it at runtime). Forward reads
  `.xyz` and re-normalizes. `.a = 1.0`, unused. This differs from the static
  atlas (oct in `Rgba8Unorm` rg) — acceptable because the animated atlas is
  compute-generated, not baked.
- **Oct decode in WGSL.** `direction_oct_packed` unpacks as `& 0xFFFFu` (x) and
  `>> 16u` (y), each `u16` → `/ 65535.0` → `[0,1]` → `* 2 - 1` gives the oct
  `[-1,1]²`; then apply the same octahedral z-reconstruction + normalize as
  `decode_lightmap_direction` (forward.wgsl:304-320) — only the channel-remap
  step differs (a packed `u32` here vs an `Rgba8Unorm` texture sample there).
  Add the decode helper to the compose shader.
- **Radiance weighting.** Reuse the per-light `c * b * entry.weight` already
  computed for `accum`; weight the decoded direction by the luminance of that
  `c * b * entry.weight` contribution (a scalar is required since `c` is a color
  vec3 — use the same luminance the renderer uses elsewhere, Rec.709
  `dot(x, vec3(0.2126, 0.7152, 0.0722))`) so the brightest-this-frame light
  dominates the fused direction, consistent with the irradiance.
- **Sampler.** Bind the new direction texture to the nearest sampler (group-4
  binding 2), as the static direction is, to avoid interpolating directions.
- **Correction reuse.** `n_dot_l_mesh`/`n_dot_l_bump`, `NDOTL_EPS = 1e-2`, the
  4.0 cap, and the `use_correction` gate already exist for the static term
  (forward.wgsl:712-725); the animated branch mirrors them with `lm_anim` as the
  magnitude gate instead of `lm_irr`.

## Wire format

No new or changed binary section. The consumed direction is the existing
`AnimatedLightWeightMapsSection` v2 `TexelLight.direction_oct` field.

## Open questions

- Radiance-weighted (per-frame, shifts as the dominant light pulses) vs
  weight-only (static) direction fusion. Spec assumes radiance-weighted for
  correctness and near-zero added cost; revisit if it causes visible direction
  swimming as multiple animated lights cross-fade.
- Whether to also fix the `ANIMATED_ATLAS_SIZE` mismatch opportunistically while
  touching the atlas-creation code, or keep it strictly separate. Spec keeps it
  separate.
