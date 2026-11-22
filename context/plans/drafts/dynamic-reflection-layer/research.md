# dynamic-reflection-layer — research

**Status:** research behind the seed in `index.md`. Moved from `drafts/glossy-world-reflections/research.md` on 2026-11-21, when glossy v1 went static-only. Section references to §Rivals, §Blur, §Memory and the other unlisted sections point at the glossy draft's `research.md`.

## Thin emitters

Why a thin, bright, animated emitter (a neon tube, a strip light, area-light glass) is the hard case for probe reflections. Discussion 2026-11-21.

- **Probe resolution.** A 128² octahedral tile spans about 1.6° per texel. A 5 cm tube seen from 3 m subtends about 1°, so it reflects as a dimmed streak under a texel wide, which coarser mips spread further. Its height is fine: 2 m at 3 m is about 37°.
- **Share ratio.** Area-light glass counts in its light's share, but a share covers a whole lobe. Switching the tube off removes only its fraction of that lobe. With other lights in the same quadrant, a ghost streak remains, at 40% of the glow when the tube supplies 60% of the lobe, and at default `_glow` the streak can still bloom. The area-lights brief's reflection rows go fully dark only in a fixture lit by that light alone.
- **Plain glow.** An `emissive_volume` tube's reflection holds its no-script state, so a flickering sign over a wet floor reflects steadily (§Volume glow share).
- **Considered and set aside: a per-texel glow id.** Store which source each texel shows and scale it at sample time. It is exact at mip 0, but it cannot mix sources under the blurred mips, and a sub-texel tube needs coverage weights even at mip 0. §Glow delta tiles is the exact form of the same idea.
- **The candidates** are §Glow delta tiles (exact everywhere) and §Glow SSR (exact on screen, reference glow elsewhere). Both are candidates for this seed's first consumer.

## Volume glow share

Estimate, 2026-11-21, against the code at `e9495f27`. [M] read from code; [E] estimated.

- **Today** [M]: id 54 stores 4 lights × 5 lobes per probe (`REFLECTION_PROBE_LIGHTS_PER_PROBE`, `REFLECTION_PROBE_LOBE_COUNT`). The GPU `FluidProbeTable` is 240 B per probe, 61,456 B at 256 probes, under a 64 KiB `const_assert` (`render-cpu` `fluid_surface/reflection.rs`). A fifth light slot does not fit. `fluid_reflection_ratio` evaluates the shares per fragment.
- **Cheapest design** [E]: one aggregated glow term per probe and lobe, Σ_v share_v × (current_v / ref_v − 1). This is exact because the ratio is linear. current_v is the post-clamp table entry; ref_v is max × colour. It joins the per-probe sums of `reflection-probe-fluid-pass-performance` (unlanded; the owner accepted its IP reading on 2026-11-22, so counsel does not gate it) at zero per-fragment cost. Before that lands it fits the table's spare words, but the table then stops being install-only.
- **Cost** [E]: CPU is about 15 MADs per (probe, volume) pair, so a few µs typically and about 1 ms at 256 × 256, and nothing on a static frame. Id 54 gains about 64 B per volume per probe and needs a version bump shared with this brief's v3. Compile cost is negligible: `ShareSums::add` gains one lobe accumulation.
- **Why it is deferred** [E]: a share spreads over a whole lobe. A sign is small and sharp, so pulsing one dims or tints a quadrant of the reflection while the sign's own image stays fixed. Ref-0 channels cannot show a recolour, and tiny refs amplify unorm10 share error.

## Glow delta tiles

A rival to the deferred limit (§Volume glow share) for glow that changes at runtime: area-light glass, `emissive_volume` signs, neon tubes. It follows the delta-SH form: a base plus each animated source's baked contribution × its live value. IP reading: `ip-delta-tiles.md`.

- **Shape.**
  - The compiler bakes, per probe, one sparse delta tile per animated glow source: what that source's glow adds to the probe's view at unit strength. A source is an area light's glass or an `emissive_volume`'s faces. Tiles are stored as 8×8 blocks of the 128² colour tile, with prefiltered mips.
  - At lookup, the overlay samples the base tile and each delta block present at that texel, and adds current_s × delta_s. Here current_s is the presented value: the volume table entry, or the area light's compose descriptor, both of which the overlay already binds.
  - Cast light stays on the share ratio. Its deltas would be dense, at about 21 KiB per light per probe, or about 22 MB at 4 lights × 256 probes [E]. Light spreads softly enough that the ratio hides its error.
- **Why it beats the alternatives.**
  - Prefiltering is linear, so blurring each delta and summing equals blurring the sum: the result is exact at every colour mip. A per-texel glow id cannot mix sources under blur.
  - It is exact off screen, where a screen-space layer has nothing.
  - It is per texel, where the aggregate share dims a whole lobe.
  - A switched-off tube goes fully dark in its reflection, and a recolour into a channel the reference lacks shows.
- **Base variant.** Prefer a base that holds glow at reference, with deltas adding (current − reference) × delta. It keeps today's tiles, water parity and the reference-state contract, and differs from the alternative (no animated glow in the base) only in how claim wording maps onto it (`ip-delta-tiles.md` §2, §7).
- **Cost** [E].
  - Bytes: a 2 m tube touches a handful of mip-0 blocks per probe, a few KB per source per probe with mips, plus a per-probe block index.
  - Per glossy fragment: one block-index read and one sample per glow source present, times the mips the exponent selects. Cap it at about 4 glow sources per probe; glow past the cap holds its reference, as today.
  - Compile: one more accumulation per glow hit in the capture trace, which `ShareSums::add` already visits.
  - Id 54: rides this brief's version 3.
- **IP.**
  - A compose pass that writes base + Σ deltas into a live tile on a source change is out. It updates stored probe lighting on a light change, which is what Tencent WO2026113651A1 claims 10–14 and 19 describe (`plans/done/reflection-probes/ip-check.md` §4).
  - Summing at sample time rewrites nothing and appears to stay outside WO claims 10–14 and 19, with claim 10 the closest element.
  - The nearest risk is Tencent US12293451B2 claim 1(c): a fixed per-probe, per-source delta × a runtime coefficient reads like its "radiance transfer parameter". This is the same open question as the shipped animated SH deltas (id 27). The shipped water share ratio has the same shape at lobe resolution; `plans/drafts/reflection-probe-fluid-pass-performance/ip-check.md` §Lever A charts it.
  - Fences: write tiles once at compile, keep deltas glow-only, and store them at base resolution (`ip-delta-tiles.md` §7).
- **Open.**
  - Per-fragment cost on the 5300M near a large glossy floor with several glow sources.
  - The sparse-block index format.
  - Whether the overlay's binding budget holds the delta array beside the base.

## Glow SSR

A second rival for glow that changes at runtime, beside §Glow delta tiles. A screen-space march fixes only the glow; the probe keeps everything else. IP reading: `ip-glow-ssr.md`.

- **Shape.**
  - *Glow pass.* After forward, an `IndirectDrawPlan` redraws only the selected faces, with depth `Equal`, into a small screen target. Selected faces are animated `light_area` glass and `emissive_volume` faces whose presented value is off reference. Each pixel writes current − reference glow, never full glow. The forward shader is untouched, as the brief's forward-unchanged row requires.
  - *Probe.* Unchanged: the base tile holds glow at reference, and cast light stays on the share ratio.
  - *March.* In the overlay, a glossy fragment marches the frame's depth along its reflected ray under the `reflection_quality` step budget, reusing `fluid_reflection_ray.wgsl`'s search. On a hit it samples the glow-change target at a mip chosen from the texel's exponent and the hit distance, and adds the sample to the probe term.
  - *Fallback.* A miss, a hit off screen or behind an occluder, or a fragment in capture shadow adds nothing, so the reflection shows reference glow. The change fades to 0 at screen edges.
  - *Gate.* The CPU skips both passes on any frame where no selected source is in view and off reference.
- **Against §Glow delta tiles.**
  - Delta tiles are exact off screen; this is exact only on screen, where neon over a wet floor usually is.
  - It bakes nothing: no id-54 bytes and no compile cost.
  - It appears to avoid '451 element (c), the stored transfer × coefficient form, with medium-high confidence: the march finds the glow at runtime from live screen data (`ip-glow-ssr.md` §4).
  - The two can stack: SSR on screen, delta tiles off screen.
- **IP fences** (`ip-glow-ssr.md` §7, §8). The nearest risk is Tencent WO2026113651A1 claims 10 and 19, which this approach appears to avoid on the same footing as the shipped share ratio, and it sits further from claim 15 than fluid SSR does. Keep:
  - The change term inside W, so capture shadow withholds it; never weight it by (1 − s) or march only where s < 1.
  - Colour from the glow target, never the frame buffer.
  - One depth buffer: selected faces write colour only.
  - The edge fades, and the ray never bends toward the screen interior (Nintendo US12694606B2, granted).
  - One direction-blind ray per fragment, with no history, no probe lookup seeded from the hit, and nothing baked per source.
  - State this layer's own capture-shadow rule in the brief, since §7.3a's "SSR never reads s" was written for water's layer beside the probe.
- **Policy.** This lifts the owner's probe-only ruling for world faces (`index.md`, Decisions) and adds a screen-space march on world faces, which needs owner sign-off, a gate and a fixed step budget (`context/lib/index.md` §2, Pre-RTX). The gate here is glossy fragments only, and only on frames whose glow pass drew. The selection is the glow pass's draw list; projectile or billboard glow could join it later.
- **Cost** [E].
  - Target: R11G11B10 or RGBA16F, about 33 MB at 4K full resolution, a quarter of that at half.
  - GPU: the march on every visible glossy fragment on frames the gate opens. A permanently animated sign in view keeps it open. Measure on the 5300M against the overlay's ceiling, with half-resolution and step-budget variants.
  - CPU: the gate test, and building the glow pass's draw list from the emissive table and the animated light set.
- **Open.**
  - Owner sign-off on the screen-space exception.
  - The glow target's resolution and format.
  - Whether the change term can be negative (a source dimmed below reference) in a blended HDR target, or needs a signed format.
  - The march's cost when the gate is permanently open.
  - The IP reading.

## Analytic and SDF reflections

Where SDFs and analytic shapes fit reflections, 2026-11-21. They earn a place where they stand in for what probes cannot hold; full SDF ray-traced reflections are not the goal.

| Use | Verdict | Why |
|---|---|---|
| Analytic stand-ins for movers and characters | Candidate | Cone versus box or capsule is analytic and cheap; the cone width from the exponent gives the softness. A flat colour per primitive suits a blurred reflection. Static geometry stays in the probe. |
| Analytic emitter specular, occluded through the SDF atlas | Candidate | A known emitter shape gives an exact soft reflection that follows its light's animation. Only occlusion needs a march, a short cone toward the emitter. |
| Specular occlusion in crevices | Baked instead | Static-on-static: the baked bent-normal-and-aperture cones planned in `context/research/gi-rtx-lite/index.md` cost about 2 B per texel and no runtime march. |
| SDF march to find the reflected hit, replacing the distance-tile search | Rejected | The distance tile already works, and a new tracer sits close to the WO claim-1 question counsel is reading. |

- **Stand-in colour.** A character has no baked albedo mean; take one from its model texture at import.
- **Capture shadow.** A stand-in in front of capture shadow changes nothing there: W is 0, so the overlay draws no term.
