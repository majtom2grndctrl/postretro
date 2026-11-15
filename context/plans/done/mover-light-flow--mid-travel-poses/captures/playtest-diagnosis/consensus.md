# Consensus: post-amendment playtest (lead, Opus and Fable all signed, 2026-11-12)

1. **No new channel defect.**
   - HEAD tracks dense truth: wall 0.02 luma, sweeper 0.51 (pre-amendment 4.51), room ceiling ~0.05.
   - The late-trip ceiling "excess" was a truth artifact. In the 177 truth, poses 123, 139 and 171 sit within 1 mm of layer z 122.96. A 178 truth matches HEAD (8.18 vs 8.20 at t′ 0.818).
   - Small residuals remain for the long-interval follow-up:
     - west band +1.2–2.0 at t′ 0.70–0.79;
     - a door-jamb block −10 to −20 at 0.82–0.85;
     - sweeper direct gaps of 9–14 per block inside long intervals.
2. **Lattice hand-over (sweeper jumps, wall-edge pulse, ceiling part b).**
   - Cause: the mover backface cosine is taken from the lookup point (`sh_sample.wgsl:278`). It flips where a layer's trilinear weight peaks, and renormalisation hands the face to the next layer in 1–3 frames.
   - The sweeper has a second sub-step at enclosure, from the depth switch.
   - It predates the brief: the strip column shows it. The amendment unmasked it.
   - A larger offset only moves the flip.
   - **Fix, in a follow-up brief:** take the backface from the surface point with a smooth, floored signed-distance ramp, optionally adding a 0.25–0.4 cell offset.
   - Owner rulings needed: lever 5 and counsel question 6, rest baselines, and a redo of the eighth-power mottling capture.
3. **Ceiling part (a): plane x = 846.93 sits 1.07 u inside the room's west wall.**
   - The lattice-shift test drops the band from 6–13.5 to under 2 until t′ 0.56.
   - **Follow-up options:**
     - an authoring warning or a lattice-phase key;
     - a dark value state for enclosed mid poses. That reverses the 2026-11-10 ruling and carries a dark-sphere risk.
   - The split between in-wall and above-roof probes is unmeasured.
4. **White square: static, not the amendment.**
   - The back-room spot `150 1420 290` lands a world-fixed footprint at x ≈ 330–510, z ≈ 117–143.
   - The upper-right square c0r1 slides under it at t′ ≈ 0.2–0.5, which is the owner's mid-travel shot.
   - At rest, c0r1 has an unattributed gated look equal to the pre-brief build.
   - The patch beside the moving edge is partly the arena spot through the opening.
5. **Bookkeeping.**
   - M9's "no stepping" is not met, for a reason this brief doesn't own.
   - An authored `light_poses` can land a pose on a seam or face band and store a dark state:
     - room 41: pose 19 is 0.24 mm from a layer;
     - room 78: pose 71 is 1.14 mm from a layer;
     - room 177: three poses.
   - The 78-pose per-knot failures (t′ 0.117, 0.805) are HEAD's 1.4 u windows, not band poses. The value at 0.831 is a genuine long-interval miss.
   - The only clean room truth count in 76–81 is 76.
