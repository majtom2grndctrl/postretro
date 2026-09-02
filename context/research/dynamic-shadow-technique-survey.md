# Dynamic Shadow Techniques — Survey and Fit Assessment

**Date investigated:** 2026-09-01
**Status:** Pre-spec exploration. Not a draft plan. Surveys the technique space
for the shadows promotion casts today — dynamic occluders (skinned meshes,
kinematic movers) lit by fixed static lights — and assesses each against this
engine's constraints and its primary dev hardware.

> **Read this when:** considering an alternative to the promoted-light shadow
> pool, scoping a shadow-performance spec, or evaluating whether a technique
> from a paper or another engine fits PostRetro's lighting architecture.
> **Key invariant:** promotion exists to shadow **dynamic occluders** from
> **fixed** lights. Any candidate that cannot see dynamic geometry does not
> replace it, whatever else it offers.
> **Related:** [Rendering Pipeline §4](../lib/rendering_pipeline.md) ·
> [SDF per-light shadows](../plans/done/sdf-per-light-shadows/research.md)

---

## What we do today

Per-object shadow-map caching in a ranked atlas. Static world depth renders once
per slot assignment into `promoted_depth_cache`; warm frames copy that layer and
draw only entity occluders with `LoadOp::Load`. Warm slots skip the shadow-cone
cull dispatch entirely. Budget is 8 spot + 2 cube slots, ranked by `slot_score`,
overflow dropped to `NO_SHADOW_SLOT`.

Three receiver classes consume the result. Movers and skinned meshes crossfade
between baked direct SH and the pool slot by weight `w`. World surfaces consume
it through the shadowmask union subtraction. See §4 "Promoted static lights."

This is already the efficient end of the shadow-map family. The survey below is
"what else exists," not "what we should have built."

---

## Cost model — where techniques get taxed

Different hardware taxes different things. PostRetro's primary dev machine is an
Intel-era MacBook Pro: **per-pass overhead and bandwidth are expensive; ALU is
comparatively cheap.** Every render pass on a tile-based or bandwidth-limited GPU
costs a load/store cycle regardless of how little it draws.

That penalizes the shadow-map family (many passes, depth reads) and rewards the
analytic and screen-space families (no passes, pure fragment math). Techniques
that *cut passes* beat techniques that *make passes faster*.

Metal-specific constraints that eliminate candidates outright:

- No geometry shaders. Silhouette extrusion must be compute.
- No multiview in wgpu on Metal. Cube shadows cost six real passes.
- No hardware ray tracing on Intel Macs. Metal 3 RT is Apple-Silicon only.

---

## Shadow-map family

| Technique | Era | Look | Where it taxes |
|---|---|---|---|
| Shadow map + PCF | 1978; universal from ~1999 | Hard edge, fixed-radius softening; bias governs acne and peter-panning | One geometry pass per light, plus forward sampling. Pass overhead dominates on TBDR |
| Cube / dual-paraboloid point shadows | Cube ~2003+; DP a 2002–2008 workaround | Cube correct; DP warps geometry non-linearly and needs tessellated walls | Cube: 6× the pass cost of a spot. DP: vertex-stage warping, wrong on large flat quads |
| VSM / ESM / EVSM / MSM | 2006–2014, peak ~2010 | Genuinely soft, wide penumbrae, filterable like a normal texture | Moves cost to the shadow pass: blur passes plus fat formats. Light leaks between nearby occluders. Bandwidth-heavy |
| PCSS | 2005; common 2010–2018 | Contact-hardening — sharp at contact, soft with distance | Two-stage forward sampling, 16–64 taps. Pure fragment cost, scales with screen coverage |
| Cached / per-object atlas | ~2013 onward; current standard | Identical to basic shadow maps | VRAM for atlas plus cache, and invalidation logic. **This is what we built** |
| Virtual / sparse (VSM, Nanite-era) | 2021+ | Near-perfect resolution match, no cascade seams | Page tables, GPU feedback loop, heavy compute infrastructure. Out of scale here |

---

## Outside the shadow-map family

**Stencil shadow volumes.** 1991 theory; Doom 3 in 2004; dead by ~2007.
Extrude silhouettes, count faces into stencil. Pixel-exact and bias-free — and
incapable of soft shadows, which killed it. Taxes fill rate enormously; screen-
covering volumes blow out ROP bandwidth. Worst possible fit for this hardware,
despite being the definitive Doom-era look.

**Projected texture / blob shadows.** 1998–2005. Render the caster to a small
texture and project it, or just project a dark ellipse. No self-shadowing, no
shadow-on-shadow, floats wrong on stairs. Nearly free; the real cost is the CPU
spatial query selecting receivers. Viable as a low-spec tier.

**Capsule shadows.** ~2015 (UE4); still current. Approximate the skeleton with a
few capsules, compute the cone-intersection soft shadow analytically. Very soft
and blobby but correctly directional; no crisp detail. A few capsules × a few
lights of ALU per fragment — no pass, no atlas, no VRAM. **The strongest
candidate here**; see "Fit assessment" below.

**Screen-space contact shadows.** 2014 onward; still standard. March the depth
buffer a short distance toward the light. Tight dark contacts where objects meet;
nothing at distance; breaks at screen edges. Fragment-side march, 8–16 steps,
half-res-able. Same computational shape as the existing SDF march but against
scene depth, so it sees dynamic occluders the SDF atlas cannot.

**SDF shadows.** 2014 (UE4 DFAO). Static baked field only. Covering dynamic
occluders requires per-frame revoxelization or an analytic primitive blend —
the expensive part, never built here. See the fit assessment.

**Voxel cone tracing.** 2011–2016. Soft, GI-flavored, low-frequency. Requires
revoxelization per frame plus a large 3D texture. Bandwidth-prohibitive here.

**Hardware ray-traced shadows.** 2018+. Ground truth, needs denoising at 1spp.
BVH refit per frame. Unavailable on the target hardware.

---

## Bake-included hybrids

**Shadowmask / distance shadowmask.** Unity 2016; the standard mid-spec answer.
Bake static-vs-static occlusion; run shadow maps only for dynamic occluders,
within a distance band. Near-zero static cost. **Already implemented here** — see
§4 "World specular shadowmask" and the union subtraction. Not untapped headroom.

**Baked light direction + runtime proxy.** 2010–2018; common in AAA. Bake the
dominant direction per probe; shadow dynamics against that direction alone with
capsules or a single map. One shadow evaluation total, independent of light
count — a large win when static lights are many and dynamic objects few, which
is this engine's exact workload. Weakness: one direction, so an object between
two lights gets one shadow.

**PRT with dynamic occluder proxies.** 2002–2010. Bake static transfer, add
dynamics as analytic occluders. The intellectual ancestor of the direct-SH plus
promotion split already in place.

**Imperfect shadow maps.** 2008. Hundreds of tiny point-splat maps for many-light
indirect. Research-only; never shipped widely.

---

## Fit assessment

**SDF does not replace promotion.** The SDF atlas is compile-time static world
(PRL section 33); movers and trigger volumes are excluded from it by
construction. A skinned enemy is not in the field, so a trace returns "lit"
through anything dynamic. SDF-typed lights are already excluded from promotion
selection, so the two systems are disjoint by design, not by accident. SDF's
niche remains what the per-light research named: a sparse set of static lights
wanting runtime-tweakable cast shadows on **static world** without a re-bake,
≤2 per surface.

**Capsules are the strongest candidate.** They are the only technique here that
scales the light count without scaling the pass count — cost is per-capsule ALU,
roughly independent of how many lights promoted. They compose with the existing
crossfade as a middle or near tier, need no PRL change and no new bake stage, and
are reversible via the tier boundary. Known risks:

1. **The union term.** World surfaces consume promotion through
   `shadowmask_visibility_difference`, whose dead zone is **calibrated to the PCF
   tap count** (`1/25` spot = "one tap of 25"; `1/9` point). A capsule has no
   taps. What "one tap of residual noise" means for an analytic visibility term
   is undefined and must be settled before capsules can feed that subtraction.
2. **Loss of self-shadowing** wherever capsules replace a pool slot — an arm will
   not shadow its own chest. Mitigate by keeping genuine pool slots for the
   nearest lights, which is the standard shipping configuration.
3. **Capsule derivation** from arbitrary rigs is more art than math and will want
   a dev-tools visualization to tune.

**Contact shadows are additive, not a replacement.** They add a pass. Real
quality gain, not a performance one.

---

## Layering, not coherence

The forward fragment shader walks the light set four times: the SDF K-selection
loop, the shadowmask union loop, the `spec_lights` specular loop, and the dynamic
direct loop. Three recompute the same per-light quantities (`to_light`, `dist`,
`atten`, `cone`, `n_dot_l`) from the same inputs with different shadow terms
attached. `shadowmask_direct` is a near-duplicate of the SDF diffuse loop body,
which is a near-duplicate of the specular loop's setup.

Each was locally the right increment — every spec added its technique without
disturbing the others, which is what kept them shippable. The accumulated result
is that a world fragment under several static lights evaluates the same light
equations three times.

The coherent form is one pass over the fragment's light set with per-light
technique dispatch selecting the visibility term (baked / SDF slice / pool map /
capsule) over a single shared analytic direct. That refactor should follow a
capsule decision, not precede it: capsules change what the dispatch needs to look
like, and doing it first means doing it twice.
