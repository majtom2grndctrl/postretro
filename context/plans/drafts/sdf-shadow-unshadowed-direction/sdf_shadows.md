# SDF Static-Occluder Shadows — EXPERIMENTAL

> **SUPERSEDED — do not implement.** This dominant-direction contract is replaced by
> the per-light model in `context/plans/drafts/sdf-per-light-shadows/architecture.md`.
> Retained in git history only.

> **Read this when:** working on the SDF shadow trace, the per-texel dominant-direction bake, or the lightmap shadowed/unshadowed mode.
> **Key invariant:** the direction bake's visibility must match the lightmap's bake mode. `Unshadowed` lightmap ⇒ unshadowed direction bake.
> **Status:** EXPERIMENTAL, A/B-gated against `main`'s baked shadows. May be reverted (see *Fail-floor*). Not yet canonical.
> **Related:** `rendering_pipeline.md` §7 · `testing_guide.md`

## What it is

Runtime shadows for static lights, traced against a baked signed-distance field of
static occluders. `main`'s alternative bakes shadows into the lightmap — free at
runtime, fully fixed. SDF is the experimental trade: pay a per-fragment march to
gain runtime-resolved static-occluder shadows that the early SDF attempt couldn't
afford. The revival is viable now because DDGI carries indirect light, freeing SDF
to specialize.

The trace sphere-marches a fine per-voxel field (`sample_fine_distance`, ~0.5 m
voxels driven by `sdf_meta.voxel_size_m`), falling back to the coarse per-brick
field only in empty bricks.

## Boundary — what SDF does not do

SDF resolves **direct** static-occluder shadows only. Indirect global illumination
and its visibility stay on DDGI. **SDF never performs indirect visibility.** This
division is load-bearing: it is why the per-fragment trace cost is bounded enough
to consider at all. A change that points the SDF trace at indirect visibility has
left this design.

## Data flow

```
        static lights                          animated lights
     (!is_dynamic, no anim)                  (!is_dynamic, animated)
            │                                        │
            ▼                                        ▼
   ┌────────────────────┐                 ┌──────────────────────┐
   │ static lightmap     │                 │ animated weight-map   │
   │ bake  [bake mode]   │                 │ bake  [bake mode]     │
   └────────────────────┘                 └──────────────────────┘
       │           │                          │            │
   irradiance   dominant dir              irradiance    dominant dir
   (lm_irr)   (lightmap_direction,        (lm_anim)   (animated_lm_direction_atlas,
               full octahedral rg)                      full octahedral rg)
       │           │                          │            │
       └─── SEAM ──┴──── visibility ≡ bake mode ───┴─── SEAM ──┘
                            │
                            ▼
                  PRL sections → GPU bind
                            │
                            ▼
        trace marches the SDF of static occluders from each lit texel
        toward its baked dominant direction (static ray, animated ray)
                            │
              shadow-factor atlas: R = static aggregate factor,
                                   G = animated aggregate factor
                            ▼
       composite:  lm_irr × R-factor   +   lm_anim × G-factor
```

Both branches — static and animated — carry the **same** seam contract. The
left/right split is the only structural difference; the visibility rule is shared.

## Invariants

**1. Direction-bake visibility ≡ lightmap bake mode.** *(load-bearing)*
The lightmap bakes in one of two modes:
- `Shadowed` — visibility baked into the irradiance. The trace is **not** applied
  (doing so would double-shadow).
- `Unshadowed` — full irradiance baked; the runtime trace supplies visibility.

The dominant-direction bake **must** use the same mode. An `Unshadowed` lightmap
requires an unshadowed direction bake: if the direction bake culls occluded
(texel, light) pairs while the lightmap doesn't, the direction is missing at
exactly the texels that should be shadowed. The trace then marches the world-up
default into open space and finds no occluder — **no shadow, indistinguishable
from "the feature didn't work."** This invariant must hold for the static and
animated branches alike.

**2. SDF factor multiplies only baked light terms (static and animated-baked).**
At `forward.wgsl:675` the composite is `lm_irr * scale * static_sdf + lm_anim *
animated_sdf`. Dynamic (shadow-mapped) lights carry their own occlusion and are
never multiplied by the SDF factor.

## Two direction failure modes — do not conflate

- **Mean direction for overlapping static lights** — a texel lit by several static
  lights points toward their luminance-weighted mean and casts one shadow that way.
  This is the **accepted approximation**, working as specified. Not a bug.
- **World-up default at a shadowed texel** — the texel has no usable baked
  direction and falls back to the world-up axis. This is a **bug**: a degenerate or
  missing direction (e.g. invariant 1 violated). Fix it in the direction bake.

## Fail-floor

The trade is gated on a perf/quality A/B against `main` on the 2020 MBP target. If
holding the frame budget forces the trace to degrade shadow quality back toward the
coarse field, `main`'s free baked shadows win on both cost and quality, and SDF is
reverted to the last pre-SDF commit. Optimization past that point is sunk cost on a
losing trade.
