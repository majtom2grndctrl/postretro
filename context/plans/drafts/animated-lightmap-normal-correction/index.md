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

- [ ] On a normal-mapped surface lit by a single `style=2` animated light, the
      animated direct term varies with normal-map detail (bump-corrected),
      visibly matching how the same surface responds under an equivalent no-style
      static light.
- [ ] A/B parity: a `style=2` light and a no-style light of equal color and
      intensity on the same normal-mapped surface produce comparable peak
      brightness (the styled light no longer reads as multiples dimmer). The
      animated light still pulses on its curve.
- [ ] The animated correction uses the same grazing-angle floor and maximum
      brightness cap as the static term; neither term can spike unbounded on
      near-backfacing geometry.
- [ ] Existing v2 PRLs load and render with no re-bake. No new section, no
      version change.
- [ ] A map with no animated weight maps renders identically to before: the new
      atlas binds the zero-fallback view. A map that has animated weight maps but
      where `lm_anim` is zero this frame hits the gated no-op. Both render
      identically to before.
- [ ] The group-4 bind-group-layout contract test covers the new binding.
- [ ] `cargo build -p postretro` and `cargo test` pass.

## Tasks

### Task 1: Fuse and emit the animated dominant-direction atlas

In the compose pass (`animated_lightmap_compose.wgsl` +
`crates/postretro/src/render/animated_lightmap.rs`): create a second
`Rgba16Float` storage atlas the same size as the irradiance atlas, add its
compose-side storage binding at binding 8, and expose a forward view plus a zero
fallback for the empty-map path, mirroring the existing irradiance atlas and its
`dummy_view`. Adding binding 8 requires: growing `compute_bgl_entries` from
`[...; 8]` to `[...; 9]` (animated_lightmap.rs:519) and adding the 9th compose
bind-group entry (animated_lightmap.rs:348-381); deleting/rewriting the
`compose_shader_has_no_dominant_direction_atlas` regression test
(animated_lightmap.rs:1089-1105) and the "intentionally absent" comment
(compose.wgsl:101-104) that currently assert the slot stays empty; and creating
the second storage view + atlas texture with a second `forward_view`. In
`compose_main`, for each contributing
texel light, decode `direction_oct_packed` to a unit vector and accumulate it
weighted by that light's current radiance contribution (the luminance of the
same `c * b * entry.weight` that drives the irradiance accum; see Rough sketch).
Normalize the sum and store
it; if the fused length is below `1e-4` (opposing lights cancel, or no
coverage) store zero. Uncovered texels stay zero (atlas zero-init).

### Task 2: Wire the atlas through the group-4 BGL

In `crates/postretro/src/lighting/lightmap.rs`: add a new group-4 texture
binding (next free slot, 5) for the animated-direction atlas as a non-filterable
float texture (sampled through the existing nearest sampler at binding 2 —
directions must not be linearly interpolated). The existing group-4 BGL slots
are: 0 irradiance, 1 direction, 2 nearest sampler, 3 animated atlas, 4 linear
sampler; the new animated-direction is slot 5. Note that this group-4 binding
(5) and the compose-side storage binding (8) are independent numbering spaces
for the same atlas — not a contradiction. Bind the compose output view, and bind
a 1×1 zero texture in the no-animated-weight-maps path (same fallback the
animated irradiance atlas uses). Grow and update the BGL contract test.

### Task 3: Correct `lm_anim` in the forward pass

In `forward.wgsl`: declare the new `@group(4) @binding(5)` direction texture.
Sample it at `lightmap_uv` using the same sample call/intrinsic as the static
direction atlas — the animated direction atlas stores a raw normalized vec3, so
read `.xyz` and re-normalize; do NOT pass the sample through
`decode_lightmap_direction` (that oct decode is only for the static atlas).
Derive the bumped/mesh NdotL ratio against the fused animated direction, and
build a `scale_anim` with the same epsilon guard and 4.0 cap as the static
`scale`, gated off when `lm_anim` is negligible (its direction is then
unreliable). Combine as `static_direct = lm_irr * scale + lm_anim *
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
  `>> 16u` (y), each `u16` → `/ 65535.0` → `[0,1]` → `* 2 - 1` → the same
  octahedral decode as `decode_lightmap_direction` (forward.wgsl:304-320). Add
  the decode helper to the compose shader.
- **Radiance weighting.** Reuse the per-light `c * b * entry.weight` already
  computed for `accum`; weight the decoded direction by the luminance of that
  `c * b * entry.weight` contribution (a scalar is required since `c` is a color
  vec3) so the brightest-this-frame light dominates the fused direction,
  consistent with the irradiance.
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
