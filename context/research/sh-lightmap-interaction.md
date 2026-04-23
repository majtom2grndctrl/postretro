# SH Volume + Lightmap Interaction on Static Surfaces

**Date investigated:** 2026-04-23

## What we found

The SH irradiance volume illuminates static world surfaces at runtime. In `forward.wgsl:533`, `sample_sh_indirect(world_position, N)` is sampled unconditionally and added to the lightmap's `static_direct` contribution (line 562). A texel sitting in full lightmap shadow (direct ≈ 0) still receives SH ambient fill and appears visibly lit.

This is intended behavior — bounce light fills shadowed areas and prevents them from going black. But it means **lightmap shadows will always appear lighter than the lightmap alone would predict.**

## Are lightmaps and SH volumes double-lighting anything?

No. The bakers are correctly split:

- **Lightmap** (`lightmap_bake.rs:707–715`): direct irradiance from each static light via shadow ray. No bounce.
- **SH probe** (`sh_bake.rs:674–693`): from each probe, rays trace to nearby surfaces and evaluate direct light at the *hit point*, multiplied by 0.5 albedo. The probe accumulates light that has bounced once off surrounding geometry — not light arriving directly from the source.

Different photon paths. Additive, not double-counted.

## Knobs for tuning shadow darkness

If shadows read as "not dark enough":

| Lever | File | Effect |
|-------|------|--------|
| SH baker albedo constant | `sh_bake.rs:37` | Lower from 0.5 → 0.3 to reduce per-bounce amplification |
| SH gain in forward pass | `forward.wgsl` near line 562 | Multiply SH result by a uniform < 1.0 before adding |
| `ambient_floor` | engine uniform | Reduces fallback fill outside probe coverage |

Treating static surfaces as black holes for SH would darken shadows but kill bounce lighting everywhere — wrong trade-off.

## Also found this session: animated SH loop was the perf bottleneck

The forward-pass per-fragment loop over `animated_light_count` (`forward.wgsl:437–475`) was costing ~34ms at 4 animated lights due to 72 scattered `anim_sh_data` reads per light per fragment. Zeroing the loop brought frame time from ~39ms to ~5ms. The compose pass already handles direct animated lighting via the atlas; the SH loop was adding animated *indirect* bounce, which is not perceptible at this aesthetic. The loop should be removed permanently.

See `context/plans/` for any follow-up plan on the loop removal.
