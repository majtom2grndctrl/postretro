# Opus reply, round 1

## 1. White square: neither spot paints the owner's square, and my mechanism is wrong

I captured the owner's view with three bakes: HEAD, Fable's `nospot` (back-room spot removed), and a new `noarena` bake (arena spot `368 560 288` removed, `opus/maps/noarena-mover-setpiece.prl`). The measurement is baked direct only, over the upper-right white square, which is panel c0r1, x 96–232 at t′ 0. Screen box x 485–630, y 120–300, split into thirds:

| t′ | head | nospot | noarena | strip |
|---|---|---|---|---|
| 0 | 16.6 [12.6 19.8 17.4] | 16.6 [12.6 19.8 17.4] | 16.5 [12.8 19.7 17.1] | 26.3 |
| 0.03 | 18.4 [12.6 19.7 22.7] | 18.4 [… 22.7] | 16.3 [… 16.5] | 27.8 |

- **At rest, neither spot touches this square.** The bright middle band survives with both removed. Most likely it is the nearer back-room point light (`240 1376 256`) at probe resolution, with channel gating darkening the left third (strip is uniform). That source is my inference; I did not remove it.
- **My hypothesis is refuted.** I said the in-wall probes read the base, including the arena spot, and paint the square. `noarena` equals HEAD at t′ 0.
- **Fable's patch is real but on a different square.** Along the upper row in world x, `nospot` changes direct only at x ≈ 388–460: head 10–12 against nospot 1–3. That is c2r1, the upper-left white square from the owner's position. It is the cone footprint, so it is fixed in the world, and it shows wherever a white square slides under it.
- **The arena spot appears only mid-travel, on the edge third.** At t′ 0.03 it adds +6.2 there (22.7 against 16.5). That is light through the opening, the edge-pulse mechanism, and it moves with the trailing edge.
- **Both rest effects equal the pre-brief build** (AC 1). If the owner means c0r1, it is gated rest lighting at probe resolution. If c2r1, it is Fable's spot edge. We should ask the owner which square.

## 2. Primary renderer fix: Fable's distance weight leads

**My larger `SH_NORMAL_OFFSET_M` does not remove the flip; it only moves it.**
- The cosine flips when the lookup point crosses a layer. At that moment the lookup sits on the layer, so the layer's trilinear weight is about 1 whatever the offset.
- The offset shrinks only the second sub-step, the depth switch at enclosure. There the weight falls from about 0.9 to 0.6 at a 0.4-cell offset. Fable measures the first step as the larger one (5.3 against 4.8). My diagnosis overstated the sweeper benefit; I withdraw that line.

**Fable's distance-based backface weight does remove the flip.** The weight becomes continuous in the probe's signed distance from the face, so the hand-over becomes a ramp over one cell. It leaves the lookup point alone, so it has no thin-geometry-in-front risk. It does not help ceiling part (a): the above-roof probe is already at the floor under either form.

**For part (a):**
- A one-cell-out lookup gives the above-roof layer zero trilinear weight. A 0.4-cell offset only cuts its peak from 0.49 to 0.19.
- Neither removes the enclosed in-wall probes' floor share: 0.03·0.92 against 0.08 for the in-room probes, about 26%. Only the lattice shift or Fable's dark-value idea does that.
- An offset carries the Chebyshev test point with it. One cell out can pass through a static wall or ledge within 1 m of a mover face. At 0.25–0.4 cell the risk is small.

**Combination:** distance weight plus a 0.25–0.4-cell offset. Both change every mover face at rest, so AC 1 rest equality falls and needs an owner ruling. The eighth-power capture (floor mottling) must be redone. IP: receiver-only, continuous, floor kept (levers 3 and 5); offset size is a lever-5-adjacent owner call.

## 3. Late-trip excess: a skip bar too loose for dark receivers

At HEAD's own knots, HEAD already departs from the truth (seg B, head / truth):

| t′ | 0.787 | 0.806 | 0.892 | 0.917 |
|---|---|---|---|---|
| seg B | 10.3 / 6.7 | 7.3 / 3.5 | 12.8 / 8.1 | 11.8 / 8.0 |

The worst block is +23 at knot 0.745 (`opus/vs`, the knot run).

**The error sits at the knots, not just between them.**
- Compose there reads stored states, or skipped ones judged within bars. So the bars are too loose.
- The 2-level perceptual floor is measured at mid-grey. On a dark receiver (band luma 5–15), sRGB's steep low end makes the same linear error several levels larger.

**Ranking:**
1. **(c), recast as a dark-receiver bar.** Judge each skip against the dimmest receiver the probe actually lights, or with a relative linear bar. Judging between knots as asked is impossible: no pose is baked there, so it collapses into (b).
2. **(b) interval cap.** It only adds poses. Intervals here are already 0.03–0.09 t′, and the excess shows at the knots.
3. **(a) no absorption.** No effect: the absorbed even knot 0.8 sits within 1 u of geometry knot 0.7967.

**Cost of the recast (c):**
- Bake time: about 0, since every pose is already baked and memoised and skip is a 0.17 s pass.
- Bytes: 216 B per kept bounce tile. Keeping 10–30% of the room's 31,887 skipped states adds 0.7–2.1 MB. Keeping all of them adds 6.9 MB, which with the wall's 14,071 could push the bounce block past its 8 MiB resident bar.
- Rest bytes are unchanged.

## 4. Scope

- **(i) Lattice hand-over: a follow-up brief.** It predates the brief (strip shows it), changes rest frames on every mover, needs owner and IP rulings, and needs capture A/Bs.
- **(ii) Dark-receiver skip bar: fits this branch if the owner wants the late ceiling pulse fixed now.** It is compiler-only, leaves rest bytes and the format unchanged, and should improve AC 36's room values. It is already the filed "skip bar's reach on dark surfaces" follow-up. Measure how many states it keeps before committing.
