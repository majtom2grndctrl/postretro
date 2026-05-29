# Research — SDF per-light shadows

Investigation notes behind `index.md`. Decisions live in the spec; this is the
trail that produced them.

## How we got here (diagnosis chain, this session)

Started reviewing the `sdf-shadow-unshadowed-direction` draft (an animated-light
bake-mode fix). Manual testing showed **no shadows on any light**, not just
animated. Investigation, in order:

1. **Bake state ruled out.** Shell history confirms `occlusion-test.prl` was baked
   `--unshadowed-lightmap --bake-sdf`, so the static SDF factor *should* apply
   (`render/mod.rs:2722-2723` sets `SDF_SHADOW_FLAG_STATIC` for `Unshadowed`). Not a
   build/flag problem.
2. **Open-space early-out gates the trace.** The SH `E[d]` skip
   (`sdf_shadow.wgsl` `sample_open_distance` / threshold) returns FULLY_LIT before
   marching when `open > threshold × cell`. Visualize mode at skip threshold 0 vs 8
   changes dramatically; the default 2.5 suppressed much of the trace. But maxing it
   did **not** restore visible shadows in normal rendering.
3. **Composite only modulates the direct term.** `forward.wgsl:675`:
   `static_direct = lm_irr * static_sdf + lm_anim * animated_sdf`; `total_light =
   ambient + indirect + static_direct` (`:677`). Indirect (SH/DDGI) is unmodulated.
   In indirect-dominated views the shadow washes out.
4. **The shadow is absent even in the raw factor.** In DirectOnly *and* in Visualize
   mode, there is **no dark spot** where the block's cast shadow belongs. So it is
   not masking — the factor itself is "lit" there.
5. **Root cause: single dominant-direction ray.** `trace_shadow(world, static_dir)`
   fires one ray toward a per-texel luminance-weighted *mean* of all static lights.
   With multiple lights the mean points at no single light, so the occluder is never
   hit. This is the `sdf_shadows.md` "mean direction — accepted approximation," and
   it is exactly what prevents per-light cast shadows. The baked lightmap traces each
   light independently at bake time, which is why it just works (image #9 reference).

## Four-spec churn audit (verdict: chasing in circles, partially)

| Spec | Role |
|---|---|
| `sdf-static-occluder-shadows` (done) | Foundation; shipped screen-UV direction, coarse-only trace, a `* brick_world_size` unit bug, asymmetric animated cull. |
| `sdf-shadow-lightmap-uv-prepass` (done) | Fixes the screen-UV direction read. The one clean, necessary addition. |
| `sdf-shadow-fine-atlas-trace` (in-progress, uncommitted) | Fixes coarse-only no-shadows + the unit bug. De-interleave math verified correct against bake+upload byte layout. |
| `sdf-shadow-unshadowed-direction` (draft) | Patches the animated-cull asymmetry. |

Specs 2/3/4 are corrective patches over spec 1's under-built foundation. The fine
field, chunk-grid cull, i16 quant, and bilateral upsample are sound and worth
keeping; the **dominant-direction trace** is the part that can't reach the goal.

## Runtime light infrastructure (grounded against source)

- All v1-authored lights are static (`is_dynamic == false`; `filter_dynamic_lights`,
  `render/mod.rs:3838`; comment `:3861-3870` "after Task 2c every authored light is
  static, `level_lights` goes empty for v1"). Forward dynamic loop is empty in v1.
- Static lights ride a culled per-fragment loop: `spec_lights` (`SpecLight`,
  `forward.wgsl:93`, `@group(2) @binding(2)`), `ChunkGridInfo` (`:101`, binding 3),
  `chunk_offsets` (binding 4), `chunk_indices` (binding 5). Loop at `:707-731` —
  **specular-only today** (`blinn_phong`). Built by `pack_spec_lights`
  (`render/mod.rs:2133`).
- Static diffuse comes from the baked `lm_irr` (`lightmap_irradiance`, `@group(4)
  @binding(0)`, sampled `:646`). Animated from `lm_anim` (`animated_lm_atlas`,
  `:649`).
- SDF factor today: separate half-res compute pass `SdfShadowPass`
  (`render/sdf_shadow.rs:120`, `HALF_RES_SCALE = 2`), writes a 2-channel
  `shadow_factor`, consumed in forward as `sdf_shadow_factor` (`@group(5)
  @binding(3)`), bilateral-upsampled via `upsample_shadow_factor` (`:486`).

## Why per-light, and the topology consequence

Per-light cast shadows require per-light visibility applied to per-light
contributions. `lm_irr` is a summed term and can't be un-summed at runtime, so the
direct static term must be evaluated per light at runtime — which the existing
`spec_lights` loop already half-supports (it iterates the culled static lights;
just needs diffuse + visibility added). N lights cannot pack into the 2-channel
factor, so the half-res pass becomes a K-slice per-light visibility producer, or
the trace moves inline into forward. The slice picks K-slice half-res to preserve
the existing cost amortization; the gate decides if that holds.

## Perf is the gate (fail-floor)

The original SDF was perf-marginal with one mean ray. K rays per (half-res) pixel is
K× the march. The fail-floor (`sdf_shadows.md`): if SDF can't beat baked on cost AND
quality on the 2020 MBP, baked wins and SDF reverts. The slice's perf measurement is
therefore the primary deliverable — a clean visible shadow that blows the frame
budget is still a fail-floor result.

## Hybrid model + authoring rule (owner guidance, this session)

SDF is not "the" shadow tech for every light — it's one member of a per-light hybrid:

- **Spot lights are SDF's worst case, shadow maps' best.** A spot is one frustum =
  one cheap 2D depth render + a one-tap compare; SDF would march per-fragment per
  spot. The engine already has the spot shadow-map path (`spot_shadow_depth:
  texture_depth_2d_array`, `spot_shadow_compare`, `light_space_matrices`,
  `forward.wgsl:193-201`; ranked pool via `filter_entity_shadow_candidates` /
  `rank_lights`). So spots/dynamic/hero → shadow maps.
- **Truly fixed lights → baked** (free, the engine's "baked over computed" northstar).
  Dropping the lightmap would delete the cheap path and force every fill light to
  runtime cost — backwards. Keep baked as the default.
- **SDF's niche:** a *sparse* set of static lights that want runtime-tweakable cast
  shadows without a re-bake. Owner authoring rule: **≤ 2 SDF shadows per surface**;
  more is allowed but risks tanking perf; the intended pattern is a blend of SDF +
  dynamic (shadow-mapped) lights.

This yields per-light tech selection (`_shadow_tech baked|sdf|dynamic`) and the clean
double-count story: the bake already filters its light set
(`!is_dynamic && animation.is_none()`, `lightmap_bake.rs:110`); adding the `sdf`/
`dynamic` exclusion makes `lm_irr` and the runtime SDF set disjoint by construction.

### Authoring-surface grounding

- `MapLight` (`prl.rs:137`): `light_type`, `intensity`, `is_dynamic` (:154),
  `casts_entity_shadows` (:159). Per-light authoring-flag pattern established.
- FGD `_cast_entity_shadows` parsed at `quake_map.rs:279` — the live KVP precedent the
  new `_shadow_tech` mirrors. `_dynamic` was retired (`quake_map.rs:247,1207`); the
  `dynamic` tech value re-introduces that routing through one key.
- Lightmap bake light filter: `StaticBakedLights` = `!is_dynamic && animation.is_none()`
  (`lightmap_bake.rs:110`). The disjoint-set exclusion extends this.
- FGD file: `sdk/TrenchBroom/postretro.fgd`.
