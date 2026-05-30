# Lighting Architecture — Full Map (per-light SDF shadows + hybrid GI)

> **Read this when:** working any lighting, shadow, or bake task — this is the
> complete map of how every lighting producer feeds every consumer.
> **Key invariant:** the primary split is **bake participation**. *Baked-tier*
> lights (`static_light_map`, `sdf`) are fixed-position and bake into at least one
> layer. *Dynamic-tier* lights (`light_dynamic`, `light_dynamic_spot`) are unbaked,
> runtime-only, and rationed (≤ 12 visible). Within the baked tier, **shadow type**
> decides only how a light's direct shadow resolves. A light is shadowed by exactly
> one source; no contribution is counted twice.
> **Completeness contract:** this map enumerates every lighting **pipeline** producer and
> consumer (bakes, passes, forward terms, GPU resources). If pipeline code touches lighting
> and is **not** on this map, treat it as a removal candidate — not as
> undocumented-but-load-bearing. This does **not** extend to the FGD light-entity KVP surface
> (defined by the FGD): this slice changes only `_shadow_type` (rename) and removes
> `_animated`; every other light KVP — `_tags`, `style`/`*_curve`, `_bake_only`,
> `_cast_entity_shadows`, geometry/intensity/color — is pre-existing and load-bearing.
> **Status:** draft-local. Promotes to `context/lib/sdf_shadows.md` once the
> perf gate holds. Supersedes the dominant-direction model.

## The model: two tiers, one shadow-type axis

Bake participation is the primary line.

**Baked tier — fixed-position lights that bake into at least one layer.** Authored
as `light` / `light_spot` / `light_sun`, carrying `_shadow_type ∈ {static_light_map,
sdf}` (default `static_light_map`):

- `static_light_map`: direct light + shadow baked into the lightmap; bounce baked into SH.
- `sdf`: direct shadow traced at runtime against a *baked* static-occluder atlas; bounce baked into SH.

Both are static-position, both feed SH indirect, both are amortized. They differ only
in **shadow type** — how the *direct* shadow resolves (baked lightmap vs runtime SDF trace).

**Dynamic tier — unbaked, runtime, rationed.** Authored as its own entities
(`light_dynamic` / `light_dynamic_spot`), carrying none of the bake KVPs. A dynamic
light participates in **no** bake — not the lightmap, not SH. It is evaluated entirely
at runtime and casts **pixel shadows via shadow maps**, paid for by a hard budget: a
**12-slot shadow-map pool** (`SHADOW_POOL_SIZE`) — lights ranked by influence, the rest
render unshadowed — flexible but rationed. Dynamic buys
flexibility the baked tier can't, for a few reasons:

- **Script-driven animation** — gameplay-driven intensity/color (e.g. arena lights reacting
  to wave state), evaluated live in the forward direct-light pass from a per-frame curve.
  **Working today — the one current dynamic use case.** A script curve can't be baked (the
  compiler never reads scripts), so these lights are inherently runtime/unbaked.
- **Movement** — the light's position changes at runtime *(planned, not yet implemented)*.
- **Dynamic-entity shadows** — runtime shadows from moving entities like enemies *(planned, near-future)*.

The capability boundary that justifies the tier: **only dynamic lights can shadow
moving entities.** Baked and SDF shadows resolve against *static* geometry only (SDF
traces a static occluder atlas). A shadow that must respond to a moving enemy has to
be dynamic.

"Animated" — a baked-tier light whose intensity/color rides a curve, position fixed —
is **not** a tier. It is a derived sub-property of a baked light (from `style`/curve
KVPs), orthogonal to shadow type. Its authoring cleanup is deferred (see Defers).

| Tier | Author entity | Bakes | Direct shadow | Runtime budget |
|---|---|---|---|---|
| baked · `static_light_map` | `light`/`light_spot`/`light_sun` | lightmap (direct) + SH (bounce) | baked into lightmap | amortized |
| baked · `sdf` | `light`/`light_spot`/`light_sun` | static-occluder atlas + SH (bounce) | runtime SDF trace (static occluders) | amortized (½-res, K-slice) |
| dynamic | `light_dynamic`/`light_dynamic_spot` | nothing | runtime shadow-map (incl. moving entities) | ≤ 12 visible |

Direct light reaches a surface through exactly **one** technique. Indirect reaches
every surface through exactly **one** path: SH (base + delta), indirect-only, fed by
all baked-tier (static-position) lights. Dynamic lights contribute neither baked
direct nor SH — they are wholly runtime.

## Pipeline

```
AUTHOR  (TrenchBroom .map)
   BAKED TIER   — light / light_spot / light_sun:
       _shadow_type = static_light_map | sdf        (default static_light_map)
       (animated when style / *_curve KVPs present — derived, position fixed)
   DYNAMIC TIER — light_dynamic / light_dynamic_spot:
       no bake KVPs; runtime-only; counts against the ≤12 visible budget
        │
        ▼
COMPILER  (prl-build) — route by tier, then by shadow type
   light-namespace seam (the load-bearing filter):
     StaticBakedLights    = baked-tier, steady     (!is_dynamic && no curve)
     AnimatedBakedLights  = baked-tier, animated    (!is_dynamic && curve)
     AlphaLightsNs        = all runtime light records (!bake_only)
   `is_dynamic` is set by the dynamic-tier CLASSNAME, not by a shadow-type value.
        │
        ├─ INDIRECT (baked tier — ALL static-position lights, shadow-type-agnostic)
        │     steady + animated      ─► base SH bake         → ShVolume          (indirect-only)
        │     animated               ─► delta SH bake        → DeltaShVolumes    (indirect-only, sparse per-light)
        │     animated               ─► animated-light chunks → AnimatedLightChunks (spatial index)
        │
        ├─ DIRECT lightmap (baked tier — static_light_map shadow type only)
        │     steady   · static_light_map ─► lightmap bake     → Lightmap                (direct, shadow baked)
        │     animated · static_light_map ─► weight-map bake    → AnimatedLightWeightMaps (direct, shadow baked, per-frame curve)
        │
        ├─ DIRECT runtime (sdf shadow type — no baked direct)
        │     all runtime lights      ─► AlphaLights pack       → AlphaLights / LightInfluence / LightTags
        │     static lights           ─► chunk light list       → ChunkLightList (spatial index, sdf flag carried)
        │
        └─ OCCLUDER FIELD (geometry only, no lights)
              static geometry         ─► SDF atlas bake (auto when sdf lights present) → SdfAtlas (signed distance bricks)

   DYNAMIC TIER → is_dynamic = true → excluded from every bake; routed to the shadow-map path.
        │
        ▼
RUNTIME LOAD
   spec_lights (all static lights, sdf-flagged) · chunk_grid spatial index
   SH base bands + delta CSR · lm_irr / lm_anim atlases · SDF atlas · shadow-map pool (≤12)
        │
        ▼
RUNTIME PASSES  (per frame, in order)
   1. animated-lightmap compose  → lm_anim atlas        (intensity curve × baked shadowed weights)
   2. SH compose                 → total SH bands        (base + Σ delta × intensity curve)
   3. spot shadow depth pass(es) → shadow-map array      (dynamic-tier lights, ≤12)
   4. depth pre-pass             → scene depth
   5. SDF visibility pass (½-res)→ K-slice visibility     (one ray per selected sdf light; K=4)
   6. forward                    → lit pixel
        │
        ▼
FORWARD COMPOSITION  (per fragment)
   total =  ambient_floor
          + indirect          (SH base+delta sample — always, unmodulated by SDF)
          + lm_irr            (static_light_map steady direct; shadow baked)
          + lm_anim           (static_light_map animated direct; shadow baked; × intensity curve)
          + Σ sdf lights      diffuse × per-light SDF visibility slice
          + specular          per-light; sdf lights × their visibility slice (baked: unshadowed)
          + Σ dynamic lights  shadow-mapped loop (≤12 visible)
        │
        ▼
   composite → lit pixel
```

## Boundary contracts (the seams)

Each seam is where a future change can silently break the chain. Each is pinned by a
test — the test is the durable record, not the prose.

| Seam | Contract | Pinned by |
|---|---|---|
| FGD → compiler | `_shadow_type` on baked entities parses to `static_light_map`/`sdf`, default `static_light_map`, unknown errors; dynamic entities set `is_dynamic` by classname | FGD-parse test |
| tier routing | dynamic-tier classnames ⇒ `is_dynamic == true`, excluded from every bake; baked-tier ⇒ `false` | tier→is_dynamic test |
| namespace filter | `StaticBakedLights`/`AnimatedBakedLights` filter on the **position** axis (`!is_dynamic`), never on shadow type | namespace-filter test |
| indirect routing | every baked-tier light (both shadow types) reaches the SH/delta bakes | indirect-coverage bake test |
| direct lightmap routing | only `static_light_map`-type lights reach `lm_irr`/`lm_anim`; `sdf` excluded | direct-disjoint bake test |
| PRL → runtime | shadow type + tier survive serialize/deserialize; legacy entries decode `static_light_map` | wire round-trip test |
| K-selection parity | SDF visibility pass and forward select the same sdf lights, same order, per tile | K-selection parity test (shared WGSL helper) |
| visibility → forward | each sdf light's diffuse **and specular** multiply by its own slice | shader naga-validation test |
| no double-count | each light: one direct technique + (if baked tier) one SH bounce; never two of either | direct-disjoint + SH-indirect-only tests; forced-1.0 render A/B (manual) |

The **namespace-filter** and **indirect-routing** seams caused the regression this map
corrects: filtering the shared namespaces on shadow type starved sdf and animated
lights of indirect bounce and dropped animated lights from the delta volume. Filter on
the position axis; push the shadow-type filter down to the direct lightmap consumers only.

## Invariants (durable)

1. **Bake participation is the primary split.** Baked-tier lights are fixed-position
   and bake into at least one layer (SH always; + lightmap for `static_light_map`;
   + static-occluder atlas for `sdf`). Dynamic-tier lights bake into nothing.
2. **Dynamic is rationed.** The shadow-map pool caps at **12 slots** (`SHADOW_POOL_SIZE`,
   `lighting/spot_shadow.rs`; the `LightSpaceMatrices` array in `forward.wgsl`) — the budget
   that makes the unbaked, fully-runtime tier affordable. Dynamic lights are ranked by
   influence; beyond the top 12 they render **unshadowed** (not culled). Dynamic is the
   *only* tier that can shadow moving entities. Script-driven (gameplay) lighting is the
   **current** dynamic use case (live forward-curve eval); movement and entity shadows are planned.
3. **Shadow type is a baked-tier sub-choice.** `static_light_map` vs `sdf` decides only
   how a baked light's *direct* shadow resolves. It never gates indirect, and it is
   independent of whether the light is animated.
4. **Direct is disjoint by technique.** Each light's direct term arrives through exactly
   one of: baked lightmap, runtime SDF, runtime shadow-map. Terms add in the forward; no
   re-weighting.
5. **Indirect is SH-only and indirect-only.** Base and delta SH both store bounce only.
   Every baked-tier light contributes its bounce regardless of shadow type. Folding
   direct into SH would double-count.
6. **One shadow per light.** `static_light_map` lights are shadowed in their lightmap
   (occlusion baked at compile time). `sdf` lights are shadowed by their runtime per-light
   SDF visibility (diffuse and specular). Dynamic lights are shadowed by the shadow map.
   Never two sources on one light.
7. **SDF never does indirect visibility.** Indirect stays on SH/DDGI. SDF resolves direct
   static-occluder shadows only. This bound is why the per-fragment march is affordable.
8. **K is the SDF shadow budget.** K per-light SDF rays per fragment is both the perf knob
   and the author guideline (≤ ~2 SDF shadows per surface, headroom to K). Beyond K
   overlapping sdf lights, extras drop (treated lit); the compiler warns per `chunk_grid`
   cell. Seed **K = 4** (the 4-channel half-res target is fully per-light; the animated
   channel is reclaimed).
9. **Specular is shadowed by the light's own technique.** SDF lights' specular multiplies
   by their visibility slice. **Known limitation:** baked (`static_light_map`) specular is
   unshadowed (no runtime visibility), and dynamic lights compute no specular term today.

## Removal candidates (completeness audit)

Code that touches lighting and is absent from this map is a removal candidate. The
current sweep flagged:

- **Static dominant-direction view** (`make_direction_view` on the lightmap resources) —
  no caller since the static SDF dominant-direction trace was removed. The `lm_irr`
  direction *binding* stays live (bumped-Lambert); only the extra view handle is dead.
- **Animated dominant-direction trace** — removed by this slice. Drops the
  `animated_lm_direction` atlas, the animated SDF shadow flag, and the lightmap-UV gbuffer
  MRT (the per-light SDF trace keys on light position, not lightmap UV). The bake no longer
  **produces** an unshadowed lightmap — `BakeMode::Unshadowed` is deleted and the bake
  always emits `Shadowed`. The wire enum `LightmapMode::Unshadowed` is retained for
  legacy-`.prl` decode only.
- **Animated weight-map `STAGE_VERSION`** const advertising a cache key that does not exist.
- **Stale `!is_dynamic`/`shadow_tech` filter comments** on the lightmap and weight-map bakes.
- **`_animated` flag + the script→animated-baked compose bridge** (the `f6bf69e` "bake the
  script animation" attempt) — dead once script-driven lights are authored as dynamic-tier
  entities. Gameplay-script lighting is inherently unbakeable; it belongs on the dynamic
  forward-curve path, not a baked compose route.

## What this slice delivers vs defers

- **Delivers:** the `sdf` shadow-type runtime path (diffuse × per-light visibility, plus
  specular × visibility), the two-tier contract (`_shadow_type` rename + the
  `light_dynamic`/`light_dynamic_spot` entity split), disjoint *direct* sets,
  tag-independent indirect, the K=4 budget, the perf gate.
- **Amends `perf-animated-sh-light-culling` (a `done/` plan), consciously:** the delta SH
  volume becomes **indirect-only** (was full direct+indirect). Removes the
  `lm_anim`↔delta-SH direct double-count; retires the `delta_scale` knob.
- **Cleans the animated path:** removes the runtime animated SDF factor (shadow already
  baked into `lm_anim`); removes the animated dominant-direction trace.
- **Defers — animated authoring cleanup:** "animated" should be a fully derived property
  (from `style`/curve KVPs), not its own KVP. Folding that — and any further light-mode
  authoring tidy — is a separate pass after SDF lands.
- **Delivers — script-driven lights restored to dynamic:** the campaign-test arena lights
  (gameplay-script intensity) migrate to `light_dynamic` / `light_dynamic_spot`, dropping the
  `_animated` flag — restoring the robust live forward-curve implementation (`ce2b555`) that
  the `_animated` bake detour displaced. Script-driven animation is a **current** dynamic use
  case, not deferred.
- **Defers — dynamic capabilities:** the `light_dynamic` entity is minimal now (geometric/
  color KVPs, no bake KVPs, sets `is_dynamic`). Light **movement** and **dynamic-entity
  (enemy) shadows** are planned but not built; their KVPs arrive when those features do.
- **Defers — animated SDF shadows:** an animated baked-tier light with `_shadow_type sdf`.
  Visibility is geometry-static and intensity rides the curve, so this folds into the
  static per-light machinery later, behind the perf gate. Until then, animated lights are
  `static_light_map` only.

## Fail-floor

The SDF-perf bet is gated on a perf/quality measurement against baked shadows on the 2020
MBP. If the per-fragment march can't make budget for the intended sparse-SDF case, baked
wins on cost and quality and SDF reverts. The animated cleanup, the delta-SH-indirect-only
amendment, and the entity split are **gate-independent** — they delete broken code, fix a
double-count/double-shadow, and de-conflate authoring, correct on their own even if SDF
reverts.
