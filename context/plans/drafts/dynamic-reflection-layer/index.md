# dynamic-reflection-layer

> **Status:** seed, not a brief. It records the problem and the work done so far, so a `/draft-session` can scope it. Run that after glossy-world-reflections builds.
>
> **Origin:** split out of `drafts/glossy-world-reflections/` on 2026-11-21, when the owner made glossy v1 static-only.

## The problem

Glossy world faces and water reflect compile-time probes. Anything that changes after compile is wrong in a reflection, or missing from it:
- **Animated glow.** A flickering neon sign, an `emissive_volume` a script dims, or `light_area` glass switching on and off. Captures hold each at its reference, so the reflection stays steady. A thin emitter is the worst case (`research.md` §Thin emitters).
- **Moving surfaces.** Doors and other movers are absent from captures, and so are characters and projectiles. `mover-aware-reflection-captures.md` records the door case.

Moving lights are mostly covered already: a glossy texel's highlight is the blurred reflection of a point or spot light, sized by the same exponent as the probe blur. Glow is the light that highlights miss.

What the owner wants: dynamic content shows in reflections, softened by the same blur as the static reflection.

## Candidates so far

- **Selective SSR layer.** A pass draws a chosen group into its own screen target. The glossy overlay marches the frame's depth to it and blurs the result by mip from the texel's exponent. A miss falls back to the probe. Glow is the cheapest first consumer: it writes its change from reference, so a miss shows reference glow (`research.md` §Glow SSR). The same pass could draw movers, characters and projectiles. Needs owner sign-off on a screen-space march on world faces (`context/lib/index.md` §2, Pre-RTX). IP: `ip-glow-ssr.md`.
- **Glow delta tiles.** A sparse baked delta per animated glow source, summed at sample time by its live value. Exact off screen, but it carries the nearest risk on Tencent US12293451B2 claim 1(c) (`research.md` §Glow delta tiles; IP: `ip-delta-tiles.md`). It can stack under the SSR layer for off-screen glow.
- **Mover-aware captures.** Captures at a mover's travel ends, blended by its pose. A baked option for doors only (`mover-aware-reflection-captures.md`).
- **Rejected:** the aggregate lobe share, which dims a whole quadrant (`research.md` §Volume glow share), and a per-texel glow id, which cannot mix sources under blur (§Thin emitters).

## Open before drafting

- Owner sign-off on a screen-space march on world faces, which reverses glossy's probe-only ruling.
- Counsel on '451 for delta tiles and for the shipped share ratio (`plans/done/reflection-probes/ip-check.md`, Watch), and on the glow-SSR fences.
- Cost of a permanently open SSR gate on the 5300M.
