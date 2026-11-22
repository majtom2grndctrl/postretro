# dynamic-reflection-layer

> **Status:** seed, not a brief. It records the problem and the work done so far, so a `/draft-session` can scope it. Run that after glossy-world-reflections builds.
>
> **Origin:** split out of `drafts/glossy-world-reflections/` on 2026-11-21, when the owner made glossy v1 static-only.

## The problem

Glossy world faces and water reflect compile-time probes. Anything that changes after compile is wrong in a reflection, or missing from it:
- **Animated glow.** A flickering neon sign, an `emissive_volume` a script dims, or `light_area` glass switching on and off. Captures hold each at its reference, so the reflection stays steady. A thin emitter is the worst case (`research.md` §Thin emitters).
- **Moving surfaces.** Doors and other movers are absent from captures, and so are characters and projectiles. `mover-aware-reflection-captures.md` records the door case.
- **Dynamic lights.** Runtime lights (muzzle flashes, rockets, scripted lamps) are diffuse-only: no surface receives their specular (`rendering_pipeline.md` §4). They show neither as a highlight nor in a reflection. Static and animated baked lights do draw highlights, sized by the same exponent as the probe blur.

What the owner wants: dynamic content shows in reflections, softened by the same blur as the static reflection.

## Candidates so far

- **Selective SSR layer.** A pass draws a chosen group into its own screen target. The glossy overlay marches the frame's depth to it and blurs the result by mip from the texel's exponent. A miss falls back to the probe. Glow is the cheapest first consumer: it writes its change from reference, so a miss shows reference glow (`research.md` §Glow SSR). The same pass could draw movers, characters and projectiles. Needs owner sign-off on a screen-space march on world faces (`context/lib/index.md` §2, Pre-RTX). IP: `ip-glow-ssr.md`.
- **Glow delta tiles.** A sparse baked delta per animated glow source, summed at sample time by its live value. Exact off screen, but it carries the nearest risk on Tencent US12293451B2 claim 1(c) (`research.md` §Glow delta tiles; IP: `ip-delta-tiles.md`). It can stack under the SSR layer for off-screen glow.
- **Analytic stand-ins for moving surfaces.** Movers as transformed box or hull SDFs, characters as a few capsules. A reflecting fragment, glossy or water, intersects its reflection cone with those primitives. The cone is as wide as the fragment's blur: the texel's exponent on glossy faces, narrow on water, which samples mip 0. The covered fraction of the probe sample takes a flat colour: a mover's mean albedo × SH at the hit, or a character's mean colour. Soft by construction, works off screen, needs no screen-space march and bakes no per-probe data. It covers a closed door's lit doorway without extra captures. The reflection counterpart of capsule shadows (`research.md` §Analytic and SDF reflections).
- **Analytic emitter specular.** Reflect a tube's or panel's glow as its known shape (representative point or LTC; the runtime-specular door in `plans/done/area-lights/research.md` §6). Exact, soft, off screen, and it follows the light's animation for free, so it answers thin emitters that are area lights. Plain `emissive_volume` glow has no light shape and stays with SSR or delta tiles. Occlusion toward the emitter needs a visibility source; the static SDF atlas is the candidate.
- **Mover-aware captures.** Captures at a mover's travel ends, blended by its pose. A baked option for doors only (`mover-aware-reflection-captures.md`).
- **Rejected:** the aggregate lobe share, which dims a whole quadrant (`research.md` §Volume glow share), and a per-texel glow id, which cannot mix sources under blur (§Thin emitters).

## Open before drafting

- Owner sign-off on a screen-space march on world faces, which reverses glossy's probe-only ruling.
- Counsel on '451 for delta tiles and for the shipped share ratio (`plans/done/reflection-probes/ip-check.md`, Watch), and on the glow-SSR fences.
- Cost of a permanently open SSR gate on the 5300M.
- Dynamic lights: no candidate shows them. Their specular on glossy faces is the direct fix, and it belongs with forward's light loop, not this layer.
- IP checks for both analytic candidates. For stand-ins, the reading to get is WO claim 15: they only cover probe samples where the cone hits a primitive, and never fill capture shadow.
- Emitter occlusion: prl-build bakes the SDF atlas (id 33) only for maps with SDF-shadowed lights, which are soft-deprecated. Using it here means baking it for maps with glossy faces and area lights, and a half-resolution march on the Mac target.
