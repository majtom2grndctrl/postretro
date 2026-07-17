# Research: Pool-Shadow Receiver Acne

Diagnosis notes behind the spec. Not implementation guidance.

## Symptom

Fine texel-grid striping ("atlas projection") on surfaces lit by promoted
static lights: the kinematic door and its adjacent wall on
`closet-reveal.map`, and a crosshatch pattern on rounded skinned characters.
Flat, pixel-textured characters show the same quantized self-shadow but it
reads as on-brand.

## Root cause

Shadow-map self-comparison ("acne") from the promoted static spot pool.
The `static-light-shadowmask-world-receipt` and mover pool-casting work
introduced the first receivers that sample a pool depth map **containing
their own geometry**:

- **World surfaces** sample the promoted slot's cached world depth through
  the forward union term (`shadowmask_union_subtraction`). Where the bake
  says lit (`baked_vis = 1`) but depth-compare noise pulls
  `shadow_map_vis < 1`, the union subtracts the light's full reconstructed
  direct term in texel-shaped stripes — the subtraction amplifies compare
  noise into visible darkening.
- **Movers** render into the live pool as rigid occluders every present
  frame (docked included) and re-sample the same map in the kinematic
  fragment loop.
- **Skinned characters** always self-referenced the pool, but curved
  smooth-shaded surfaces sweep gradually through the compare threshold, so
  the hardware bias tuned for them shows crosshatch on rounded models.

Three receiver classes, three shading paths, one shared artifact — the pool
sampling in `shadow_sample.wgsl` is the common cause. The spot path's
compare reference is raw projected NDC z with **no receiver-side bias**;
only the caster-side hardware `DepthBiasState` (constant 2, slope 1.5,
identical across world/rigid/skinned depth pipelines) absorbs error. The
cube path already carries a receiver-side world-space bias
(`POINT_SHADOW_DEPTH_BIAS`), with a comment noting the spot path was tuned
assuming hardware bias alone.

## Why receiver-side normal-offset (not more caster bias)

- Raising hardware slope-scale is global: it degrades the already-working
  skinned contact shadows and still cannot cover the coplanar
  door-against-wall contact case.
- Front-face culling in depth passes inverts tuning for skinned occluders
  and risks leaks at the door/wall contact seam; the rigid occluder
  pipeline comment pins all caster classes depth-identical to avoid seams.
- A constant receiver depth bias needs retuning per light distance/angle;
  normal-offset scales with the shadow-texel footprint structurally.

## Aesthetic finding

Quantized self-shadow on flat pixel-art characters is desirable. One
engine-wide skinned constant cannot serve both flat and rounded models →
per-model authoring scalar, defaulting to current appearance.

## Discriminators used

- Door samples no lightmap (SH + pool only) yet stripes like the wall →
  rules out lightmap/shadowmask atlas content as the source.
- Stripe frequency matches the 1024² spot map texel footprint at close
  range, far finer than lightmap texel density.
- Character crosshatch is gradual across curvature → bias territory, not
  PCF/resolution.
