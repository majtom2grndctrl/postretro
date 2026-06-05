# Research — Animated Lightmap Normal-Map Correction

Investigation notes behind the spec. Not durable; delete on ship.

## Reported symptom

Yellow `style=2` (animated) lights render significantly dimmer than equivalent
no-style lights on the same surfaces. Turning off `style` makes a light "much
brighter, and it stays much brighter." Recent (~8 days). Surfaces are
normal-mapped. The dim light reads as roughly constant, not visibly pulsing.

## Root cause (confirmed)

The normal-maps feature added a bump-correction factor to the **static** lightmap
term only:

```
static_direct = lm_irr * scale + lm_anim;   // forward.wgsl:732
```

`scale = clamp(n_dot_l_bump / n_dot_l_mesh, _, 4.0)` (forward.wgsl:725), derived
from the static directional lightmap's per-texel dominant direction. `lm_anim`
(animated lights) is added uncorrected — comment at forward.wgsl:710 ("lm_anim
is not corrected. See normal-maps/ Task 4"). The normal-maps plan deferred this
explicitly:

- `context/plans/done/normal-maps/index.md:34` — "Animated lightmap normal-map
  correction — requires a separate dominant-direction channel for the animated
  atlas; deferred to a follow-up plan."
- `:88` — `static_direct_corrected = lm_irr * scale_capped + lm_anim; // animated atlas stays uncorrected for now`
- `:91` — confirms deferral.

On normal-mapped surfaces a no-style light gets boosted up to 4×; its `style=2`
sibling does not. Hence the A/B result. Reads as "constant & dim" because the
small uncorrected animated pulse sits beside a now-4×-brighter static neighbour.

## Ruled out (earlier investigation)

- **BC6H irradiance compression** (`af0ae10`) — touches only the static
  irradiance blob; animated atlas stays `Rgba16Float`. Wrong brightness
  direction (would dim static).
- **Per-light layers + per-group SH** (`976197b`) — refactors the static bake;
  animated weight-map bake byte-identical before/after (verified: same `.prl`
  bytes at HEAD-cold, HEAD-warm, and pre-window parent for a `style=2` fixture).
- **Animated atlas size mismatch** (`ANIMATED_ATLAS_SIZE = 1024` vs static atlas
  up to 8192) — real latent bug since day one (`9d39149`), big-maps only, not
  this regression. Worth a separate fix.
- **Clock unification** (`56b9542`, script_time) — only affects spawn-phase, not
  persistent dimness.

## Scope-collapsing discovery

The "separate dominant-direction channel" the normal-maps plan called for
**already exists**:

- `AnimatedLightWeightMapsSection` is version 2; each `TexelLight` carries a
  baked `direction_oct: [u16; 2]` (per-texel incoming direction toward the
  light), `crates/level-format/src/animated_light_weight_maps.rs:44-64`.
- The bake still populates it from the contribution calculation,
  `crates/level-compiler/src/animated_light_weight_maps.rs:284-298`.
- It is packed into the GPU buffer as `direction_oct_packed: u32` (low 16 = x,
  high 16 = y), `crates/postretro/src/render/animated_lightmap.rs:682-685`, and
  declared in the compose shader's `TexelLight`,
  `animated_lightmap_compose.wgsl:60-68`.

It went unread when the per-light SDF dominant-direction trace was removed
(sdf-per-light-shadows Task 1); compose binding 8 is "intentionally absent"
(`animated_lightmap_compose.wgsl:101-104`). So this plan re-consumes existing
baked data — no PRL format change, no re-bake, no version bump.

## Existing direction-correction path (to mirror)

- Static direction atlas: `Rgba8Unorm`, octahedral in rg, sampled NEAREST
  (binding 2, linear lerp of oct vectors ≠ slerp), decoded by
  `decode_lightmap_direction` (forward.wgsl:304-320).
- Group-4 BGL (fixed contract with forward.wgsl `@binding`s) lives in
  `crates/postretro/src/lighting/lightmap.rs`: irradiance@0, direction@1,
  nearest sampler@2, animated_atlas@3, linear sampler@4
  (`bind_group_layout_entries` → `[...; 5]`, lightmap.rs:161).
- Compose output atlas: `Rgba16Float`, created at animated_lightmap.rs:257,
  written via storage binding 6 (`compute_bgl_entries` → `[...; 8]`,
  animated_lightmap.rs:519); `forward_view` (animated_lightmap.rs:160) is bound
  into group-4 binding 3. Empty maps bind a 1×1 zero texture (animated atlas)
  and a dummy view (animated_lightmap.rs:218, 235).
