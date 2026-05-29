# SDF Per-Light Shadows — Architecture Map

> **Read this when:** working any task in this plan, or any later SDF shadow work —
> it is the map of how the pieces connect so you don't get lost mid-task.
> **Key invariant:** shadowing is a per-light author choice across three **disjoint**
> techs — a light is shadowed by exactly one of baked / SDF / dynamic.
> **Status:** draft-local. Rewrites and replaces the old dominant-direction
> `sdf_shadows.md`; promotes to `context/lib/sdf_shadows.md` once the perf gate holds.

## The model in one line

Each light carries a tech tag. **Baked** lights bake their shadow into the lightmap
(free, fixed). **SDF** lights are traced per-light at runtime (tweakable, sparse).
**Dynamic** lights use shadow maps (spots, moving, hero). The three sets never
overlap, so no light is shadowed twice and no contribution is double-counted.

## Pipeline

```
AUTHOR  (TrenchBroom .map)
   light entity:  _shadow_tech = baked | sdf | dynamic     (default: baked)
        │
        ▼
COMPILER  (prl-build)
   parse _shadow_tech → MapLight tag, then route by tag — DISJOINT SETS:
        baked    → lightmap bake          → shadow baked into irradiance (lm_irr)
        sdf      → excluded from bake      → emitted to light buffer, sdf-flagged
        dynamic  → is_dynamic = true       → shadow-map path
        │
        ▼
PRL  (light entry carries the tech tag)
        │
        ▼
RUNTIME LOAD
   light buffer (spec_lights) + chunk_grid spatial index (per-tile light lists)
   fine SDF field atlas (static occluders)
        │
        │   ┌──────────── shared K-selection: per tile, pick the K nearest/brightest
        │   │             sdf-flagged lights from chunk_grid. The visibility pass and
        │   │             the forward MUST select the same lights in the same order.
        ▼   ▼
   HALF-RES VISIBILITY PASS  ──►  K-slice half-res target
        per tile: trace one shadow ray per selected sdf light against the
        fine SDF field → one visibility factor per slice
        │
        ▼
FORWARD SHADER  (per fragment, accumulate)
   indirect            : SH / DDGI                         — always, unmodulated
   baked-tag direct    : lm_irr (+ lm_anim, until migrated) — shadow already baked in
   sdf-tag direct      : Σ over the same K sdf lights of
                           diffuse(light) × upsample(visibility_slice[light])
   dynamic-tag direct  : shadow-mapped light loop
   specular            : spec_lights (all static lights)
        │
        ▼
   composite → lit pixel
```

## Boundary contracts (the seams)

Each seam is a place a future change can silently break the whole chain. Each is
pinned by a test — the test is the durable contract record, not the prose.

| Seam | Contract | Pinned by |
|---|---|---|
| FGD → compiler | `_shadow_tech` parses to a tag; `baked` default; unknown value errors | FGD-parse test |
| compiler routing | baked-tag → lightmap bake set; sdf/dynamic-tag → excluded; sets are disjoint | disjoint-set bake test |
| dynamic mapping | `dynamic`-tag ⇒ `is_dynamic == true`; baked/sdf ⇒ `false` | tag→is_dynamic test |
| PRL → runtime | the tech tag survives serialize/deserialize; legacy entries decode `baked` | wire round-trip test |
| K-selection parity | visibility pass and forward select the **same** sdf lights, same order, per tile | K-selection parity test |
| visibility → forward | each sdf light's diffuse is multiplied by *its own* upsampled slice | shader naga-validation test |
| no double-count | with visibility forced to 1.0, total direct light == pre-change baseline | SDF-forced-to-1 baseline test |

The K-selection parity seam is the load-bearing one: a shared selection helper both
sides call (keyed on the `chunk_grid` cell) is the only thing that keeps the
visibility slice aligned with the diffuse term. Break it and shadows attach to the
wrong lights with no compile error.

## What this slice delivers vs defers

- **Delivers:** the sdf-tag runtime path (diffuse × per-light visibility), the tag
  contract, disjoint bake sets, the K budget, the perf gate.
- **Keeps as-is:** baked `lm_irr` and animated `lm_anim` (animated lights stay on the
  weight-map bake for now); the dynamic/shadow-map machinery.
- **Defers (follow-on, gated on the perf result):** migrating animated lights onto
  runtime per-light curves, which then retires the **Unshadowed** lightmap bake mode
  entirely — with SDF lights out of the lightmap, baked-tag lights want their shadow
  baked in (Shadowed mode), so Unshadowed loses its last consumer.

## Invariants (durable)

1. **Disjoint sets.** A light is baked **or** sdf **or** dynamic — never two. Enforced
   at the compiler routing seam. This is what makes "no double-count" hold by
   construction rather than by runtime arithmetic.
2. **SDF never does indirect visibility.** Indirect/GI stays on DDGI/SH. SDF resolves
   direct static-occluder shadows only. This bound is why the per-fragment march cost
   is affordable at all; pointing the trace at indirect re-creates the original SDF
   perf disaster.
3. **K is the SDF shadow budget.** K sdf-shadow rays per fragment is both the perf
   knob and the author guideline (≤ ~2 SDF shadows per surface). Beyond K overlapping
   sdf lights, extras drop (treated lit); the compiler warns.
4. **Right tool per light.** Fixed lighting → baked (free). Sparse runtime-tweakable
   static lights → SDF. Spots / moving / hero → shadow maps. SDF earns its march only
   where baking can't follow runtime tweaks and shadow maps don't scale.

## Fail-floor

The trade is gated on a perf/quality measurement against baked shadows on the 2020
MBP. If the per-fragment march can't make budget for the intended sparse-SDF case,
baked wins on cost and quality and SDF reverts. Optimization past that point is sunk
cost on a losing trade.
