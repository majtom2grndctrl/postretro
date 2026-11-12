# Fable reply, discussion round 1

## 1. White square: both, at different t′

- **Mid-travel (t′ 0.2–0.4, the owner's shot 3): the back-room spot, world-fixed.** Dividing direct-only head by direct-only strip removes the checker albedo. The lit part of the door's upper row stays at world x 328–514, z 117–143 (centroid 418–430) while the trailing edge moves from x 205 to 314; its door-relative centroid slides 225 → 104 u. `nospot` removes it. Emptying id 57's depth block leaves it unchanged (panel mean 5.96 vs 6.14), which rules out the floor-weighted in-wall probes there: without the dark tile their share would rise about 30×.
- **At rest and t′ 0.1: a second component Opus is seeing.** The lit area also reaches up to z 210 and out to within 16 u of the trailing edge, and moves with it (x 112 at edge 96, 161 at edge 150). A wedge at the trailing panel survives `nospot` at t′ 0. I did not attribute that one; Opus's enclosing-terminus reading is plausible for it.

## 2. Late-trip ceiling excess: not a HEAD defect. The 177-pose truth is wrong there.

- At 177 poses, pose 139 puts the room's roof/wall seam at z 123.00. The lattice layer is z 122.96, 1 mm away, inside the bake's seam band. The in-wall column at that layer is then not enclosed and stores in-solid dark bounce at that one pose (the plan's Correction "Enclosure seams" case, which only knot placement avoids). The truth lerps into and out of it.
- Experiment: a truth at 178 poses (no pose on the seam). West band beside the door, and depth-stripped pairs:

| t′ | head | truth 177 | truth 178 | head, no depth | 177, no depth | 178, no depth |
|---|---|---|---|---|---|---|
| 0.784 | 10.13 | 8.23 | 9.15 | 25.5 | 19.1 | 22.8 |
| 0.818 | 8.20 | 3.34 | 8.18 | 26.4 | 3.8 | 26.5 |
| 0.852 | 11.58 | 6.05 | 11.04 | 26.0 | 9.2 | 26.1 |
| 0.886 | 12.33 | 7.66 | 12.25 | 24.4 | 11.4 | 24.4 |
| 0.920 | 11.39 | 8.44 | 10.87 | 22.4 | 13.4 | 22.5 |

  Above the door at 0.818 / 0.903: head 11.66 / 16.54, truth 178 11.69 / 16.55, truth 177 7.29 / 14.29.
- So I do not accept the skip mechanism for this excess, and I withdraw my own "known long-interval miss" line. Probe 7341's sparse marks are real but are not shown to cost anything. None of (a)–(d) is warranted by this evidence.
- A real finding falls out: an authored `light_poses` count can land a pose on a brush seam and store a dark state. AC 36's truths should be checked for it.
- Residual: truth 178 sits 1–1.5 below head at t′ 0.68–0.72, unexplained.

## 3. Renderer fix

- **A larger offset moves the flip; it does not remove it.** The backface cosine is taken from the lookup point (`sh_sample.wgsl:278`, `gfrac`). The flip happens when the lookup sits on the layer, where that layer's trilinear weight is 1, whatever the offset. Opus's 0.9 → 0.6 is the weight when the face reaches the layer, by which time the layer is already floored.
- **Correction to my own "one cell out" variant:** it has the same flaw unless the backface term is measured from the surface point instead of the lookup point. That change is the essential one; the distance ramp then makes it continuous for any offset.
- **In-wall column:** my weight does nothing for part (a). Opus's offset does cut the above-roof share (0.49 → 0.19, arithmetic agrees) and the door's y share, but not the ceiling's x share of the in-wall column. The two combine. Neither is measured.
- **Rest and AC 1:** both change every mover's rest frame. AC 1 compares per-probe channel contributions, so it should hold; frame goldens and C10's frame case need re-baselining.
- **IP:** a signed distance along the receiver's normal is the half-space counsel question 6 already asks about; keep it smooth and floored. Lever 5 makes either form the owner's call.

## 4. Scope

Follow-up brief. The hand-over predates this branch, the fix needs an owner ruling on the backface form plus new rest baselines, and the branch's truth-gated ACs hold. Tell the owner M9's "no stepping" row is not met in motion, for a reason this brief does not own.
