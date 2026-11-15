# Mid-travel poses: diagnosis of the owner's post-amendment observations (Fable)

Branch `feat/mover-light-flow--mid-travel-poses` at `17532c316`. Release `prl-build` and `postretro --features capture` built at HEAD. Diagnosis only; no tracked file was edited.

Everything below is under `F` = `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/fable`.

## Headline

Compose is doing what the amendment asked. On every surface the owner named, HEAD tracks a fine-pose truth bake through the artifact, so none of the three is a knot-placement, bracket, skip or depth-blend error. What the owner now sees is what the truth bake itself looks like in motion:

1. **Lattice hand-over (shared by all three motion artifacts).** A mover face is lit from a world-fixed 1 m probe lattice. Each time the face's lookup point passes a probe layer, the layer carrying the face's light is swapped for the next one within 1–3 frames. The two layers hold different light, so the face steps. It predates the brief (the id-57-stripped build shows it), and it is now the largest thing left.
2. **A probe layer inside a thin mover wall (ceiling band only).** On `mover-setpiece` the lattice layer x = 846.9 sits inside the rising room's west wall (x 832–848), 1.1 u from its inner face. Every probe the ceiling's west strip can read on that side is either enclosed or above the roof, and their floor-level weights carry bright arena bounce into a dark room. Shifting the lattice 32 u removes the band.
3. **The white-square patch is a separate, static thing.** It is the outer cone of the back room's spot (`150 1420 290`) on the door's north face, drawn at probe resolution. Removing that light removes it.

## Method

- **Columns.** All baked cold `--release`, each with its own cache dir, deleted after.
  - `head`: the maps as committed (room 45 poses, wall 71, sweeper 31, as the brief states).
  - `truth`: map copies with `light_poses` 257 on em_sweeper (1 u), 177 on the room (1 u), 137 on the wall (4 u).
  - `nodepth`: head with id 57's depth block emptied. `strip`: head with id 57 removed.
  - `shift`: `mover-setpiece` with the world's east wall thickened 32 u inward, which moves the x lattice 32 u (layers 854.3 and 814.9 instead of 846.9; no layer inside either room wall).
  - `nospot`: `mover-setpiece` without the spot at `150 1420 290`.
- **Captures.** Offscreen, 960×540, `POSTRETRO_SH_STREAMING=sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover. Camera rides the mover so pixels stay on the surface. Samples every 1 u of travel on the sweeper and room, 2 u on the wall. Luma is Rec. 709, 0–255; blocks are 8×8 px.
- **Probe lattice** read from each PRL (id 47 header). Setpiece: x layers …807.6, 846.9, 886.3…1122.5, 1161.9; y …1009.4, 1048.8, 1088.2…; z …44.2, 83.6, 123.0…. Enemy-movers: x …1300.0, 1339.3, 1378.7….
- **Speeds.** One 60 fps frame is 2.95 u on the sweeper (4.5 m/s), 0.66 u on the room (1 m/s), 1.31 u on the wall (2 m/s). One lattice step takes 0.22 s, 1.0 s and 0.5 s.
- Tools: `F/tools/ocap.py` (views, capture), `F/tools/oan.py` (series), `F/tools/bake1.sh`. Captures in `F/cap/`. Series tables `F/results-*.txt`, data `F/results-*.json`. Charts `F/chart-series.png`, `F/chart-shift.png`.

## 1. em_sweeper, face toward the crushers: shimmer that now steps

### Mechanism

- A mover fragment looks up probes at its position plus 0.1 cell (3.94 u) along its bumped normal (`crates/renderer/src/shaders/kinematic_brush.wgsl:246–247`, `:268–269`), with the smooth backface weight ((cos + 1)/2)⁸ + 0.03 (`crates/renderer/src/shaders/sh_sample.wgsl:268–292`).
- The east face is parallel to a lattice plane and moves along its normal. The whole face therefore passes each x layer at once.
- The backface term takes the cosine of the direction to the probe. When the lookup point passes a layer, that cosine flips from +1 to −1 for the probe straight ahead, so the layer's weight falls to the floor within a few units of travel (by the formula; the measured step spans 2–3 u). The next layer, 39 u ahead, has trilinear weight near 0 but is the only probe left above the floor, so after renormalising it takes over.
- About 4 u later the face itself reaches the layer. The pose just inside (the geometry knot) stores the dark depth tile and the layer's values blend across. That is a second small step.
- The two layers hold different direct light: the near one is shadowed by the slab from the west lights, the far one is not. The lights are coloured (`255 140 100`, `255 180 90`, `210 225 255`), and the lookup offset follows the normal map, so the swap arrives per bump. That is the "rainbow".

### Evidence (`F/results-sweepfix.txt`, `F/chart-series.png` top, `F/sheet-sweep-crossing.png`)

- **HEAD tracks truth through every crossing.** At the 17 samples where head's block step passes 3, truth − head is within ±0.3 in the mean at 14 and within 0.9 at all; the worst block is under 1.1 at 11, 2.2–3.2 at four, and 7.2 at one (t′ 0.859, where the truth also has a direct-shadow step head interpolates).
- **The truth steps.** Largest block step per 1 u sample:

  | t′ | 0.094 | 0.109 | 0.250 | 0.402 | 0.559 | 0.711 | 0.855–0.879 |
  |---|---|---|---|---|---|---|---|
  | head | 5.3 | 4.8 | 4.5 | 3.5 | 3.3 | 4.4 | 3.6–7.4 |
  | truth | 5.7 | 4.6 | 4.6 | 3.7 | 3.5 | 4.3 | 3.2–11.8 |
  | strip | 3.6 | 0.9 | 2.8 | 3.7 | 2.9 | 3.4 | 0.4–2.6 |

- **Timing matches the lookup offset.** The first step falls at t′ 0.0937, face at x 1336.0, lookup at 1339.9, layer at 1339.3. The second falls at 0.109, the inside knot 0.10848. `nodepth` has the first and lacks the second (0.97 against 4.80), so the second is the depth switch at enclosure.
- **Per 60 fps frame** (every third sample): 15 of 85 frames have a block step over 3 (up to 9.2), in six clusters of 2–3 frames, 0.154 t′ apart. The other frames' median largest step is 1.3. The crop mean moves 1 to 4.6 luma per cluster (47.8 → 52.4 over t′ 0.086–0.109 at the first).
- **It is baked direct.** Light-term masks at the first crossing (`cap/sweepfix-*-headM*.png`): baked direct alone steps 6.9–9.2 per block with |Δ(R−B)| up to 15.4; indirect alone steps 1.0–1.7. Specular is zero in capture (no promotion in a one-frame capture).

### Why it changed from continuous to stepping

- Before the amendment (`captures/motion-results.md`), the face ramped 5.7–10 luma through every knot interval, seven per trip, so it changed on every frame: a continuous shimmer.
- Geometry knots removed those ramps. The face now holds nearly still between crossings and changes only in the 2–3 frames of each hand-over, 4.5 times a second. The hand-over was always there (strip has it); it was hidden inside the ramps.
- The geometry knots also make the enclosure switch exact, which adds the second sub-step that `nodepth` lacks.

## 2. Rising room, ceiling above the door: pulsing light leak

### Mechanism

Two parts, both periodic at one z-lattice step (0.224 t′, 1.0 s).

- **(a) The in-wall probe column (dominant).** Layer x = 846.9 lies inside the west wall. Ceiling fragments from x 848 to 886 blend it with the interior layer 886.3, and near the wall its trilinear weight approaches 1. Around the ceiling, its probes are either enclosed by the wall and roof (dark depth tile, so Chebyshev floors at 0.03; values blend across toward the terminus, which reads the base — `build_pipeline.md` step 12, *An enclosing terminus reads the base*, *Skipping*) or above the roof in open arena air (backface floor 0.03). No probe on that side has a real weight, so the floors decide the mix, and the values they carry are arena-bright while the room is dark. As the room rises, the z weights, the roof-top enclosure and the ceiling-plane crossing move that mix through one cycle per layer.
- **(b) Hand-over above the door.** The ceiling's own lookup passes a z layer once per cycle, as in §1.

No other wall of the room contains a layer (south 182.7, north 458.3 and east 1122.5 are all inside the room), which is why only the door wall shows it. The band runs the whole length of the west wall in my captures, not only above the door.

### Evidence (`F/results-lintel2-wall.txt`, `-door.txt`, `F/chart-series.png` middle and bottom, `F/chart-shift.png`)

- **A regular pulse.** Band beside the door, head: 3.7 → 13.4 (t′ 0.13–0.23), 6.2 → 13.6 (0.36–0.46), 6.9 → 13.5 (0.585–0.69). Drops fall at the roof top passing a layer (0.251, 0.475, 0.698) and at the ceiling plane and lookup passing it (0.342–0.364, 0.566–0.588, 0.79–0.81).
- **HEAD tracks truth up to t′ 0.69.** The band means agree within 0.3 luma at 112 of 122 samples beside the door and 103 of 122 above it; the rest are single samples beside an event, off by up to 2.3 (the 1 u truth has no pose inside head's 1.4 u windows). Past 0.69 head is brighter than this truth by up to 5 (0.70–0.92). **Corrected in round 1:** that gap is an artifact of the 177-pose truth, whose pose 139 lands on the roof/wall seam 1 mm from layer z 122.96; a 178-pose truth agrees with head within 0.5 from t′ 0.82 on (`reply1.md`).
- **It is all static indirect.** At t′ 0.364, 0.449 and 0.523 the indirect-only capture equals the full frame (5.92, 13.17, 10.06); baked direct and dynamic are 0.
- **Depth already removes most of it.** `nodepth` reads 20–33 where head reads 6–13.
- **Shifting the lattice removes it.** Band beside the door: head 6–13.5, `shift` 0.0–1.8 up to t′ 0.56, under 3.3 to 0.77 and under 5.3 to the end. Its largest block steps fall from 3–5 to under 2.
- **Part (b) remains in `shift` above the door:** 3.2 → 7.1 (t′ 0.47–0.56) then 3.9; 11.3 → 5.9 at 0.79–0.81. A sawtooth of 3–5 luma at the lookup crossings.

## 3. Wall door

### Edge pulse

- **Mechanism:** §1's hand-over on the trailing end face, which is parallel to an x lattice plane and moves along its normal into brighter air.
- **Evidence (`F/results-walledge.txt`):** a staircase every 0.0724 t′ (39.37 u, 0.5 s). End-face mean: 41 → 67 at t′ 0.147–0.162, 72 → 88 at 0.221–0.232, 91 → 95.5 at 0.298–0.305, 97 → 101 at 0.371–0.379, and smaller ones on to 0.74 (block steps 3–6). Largest single 2 u step: +11 mean, 26 per block.
- **`strip` has the same staircase** (52 → 69, 80 → 91), and so do truth and `nodepth`. This one does not involve id 57 at all.

### Upper-right white square

- **Mechanism:** baked direct light for movers comes from the 1 m direct-SH probes. The back room's spot at `150 1420 290` (pitch −44°, outer cone 28°) puts its cone edge across the door's north face at about z 110–140 (my estimate from its angles). The z 123 and z 162 probe layers straddle that edge, so it draws as lattice-sized blotches. With id 57 absent, the arena spot (`368 560 288`, aimed north through the opening) lights those probes through the missing wall and hides the edge. The channel gates that light off (the gate report lists it for the wall at both termini), which is right for a face turned away from it, and the blotchy edge is what remains.
- **Evidence (`F/sheet-wall-zoom.png`, `F/sheet-wall-mask.png`, `F/sheet-wall-m8.png`, `F/sheet-wall-nospot.png`):**
  - head and truth show the same patch at t′ 0.2, 0.26 and 0.3; strip is uniform.
  - It is in baked direct alone (panel mean: head 5.96, strip 22.02); indirect is uniform. `nodepth` shows the same patch, so depth plays no part.
  - It appears fixed in the world while the door slides (by eye, across t′ 0–0.6).
  - `nospot` at t′ 0.26: the patch is gone (door region mean 14.5 → 9.5).
- It is static, present at rest, and not a product of the amendment.

## Shared cause?

- **Sweeper stepping, wall-edge pulse, and part (b) of the ceiling pulse: one cause.** A mover surface read from a world-fixed lattice through a direction-cosine backface weight hands over between layers in 1–3 frames. It is worst where a flat face parallel to a lattice plane moves along its normal, because the whole face hands over at once.
- **Ceiling band, part (a): a second cause** that needs the first's ingredients (floors, renormalising) plus a layer inside a thin mover wall.
- **White square: unrelated** to motion and to the amendment.
- **Why most edges were fixed and these survived:** the amendment removed blend errors (knot blinks, ramps across crossings). These three are not blend errors; the truth bake has them.

## Confidence and gaps

- **High:** head tracks truth on all three; the hand-over timing and its presence in strip; the lattice shift removing the band; the spot removal removing the patch; the term splits.
- **Medium:** the split inside ceiling part (a) between enclosed probes' blended-across values and the above-roof probes. The shift test shows the in-wall column is necessary; I did not dump per-probe weights, so I cannot say which carries more.
- **Not confirmed:**
  - Nothing was run in real time or on Windows. Frame-rate claims come from the travel grid and the authored speeds.
  - Promoted-light specular and the promotion crossfade do not run in a one-frame capture. In play they may add to the sweeper's shimmer.
  - No pre-amendment build was captured here; "continuous before" rests on `motion-results.md`.
  - The door's large faces and the room's floor were not re-measured.
  - The wall truth has 4 u knots, coarser than its 1.4 u windows.
  - Whether the owner's "above the door" means the whole west band or only the door segment.

## Fix directions, ranked

1. **Continuous hand-over for mover receivers (shader only).** Replace the direction cosine in the mover backface weight with the probe's signed distance along the receiver's geometric normal, scaled by the cell, so a layer's weight reaches the floor as the face reaches it instead of flipping when the lookup passes. A simpler variant places the normal-axis lookup one cell out, so a layer's trilinear weight is already 0 when the face swallows it.
   - *Effect:* the 1–3 frame steps on the sweeper, the wall's end face and the door ceiling become ramps over one lattice step. The lattice-period variation stays, as a smooth ramp (the wall end still gains ~19 luma over 0.5 s). Ceiling part (a) is not fixed by this alone.
   - *Cost:* one WGSL function and its capture A/B; the eighth-power form was an owner choice by capture, so the floor mottling it fixed needs re-checking. No bake or format change.
   - *IP:* uses only the receiver's own normal and the lattice arithmetic; stores nothing; the weight stays continuous with the 0.03 floor (levers 3, 5, 6). No per-probe flag.
2. **Keep probe layers out of thin mover walls (compiler placement).** Choose or let the author set the lattice phase so no layer lies inside a mover's wall close to its face along its path, or warn when one does.
   - *Effect:* measured on this map: the west band falls from 6–13.5 to under 2 until t′ 0.56 and under 3.3 until 0.77.
   - *Cost:* small in code, but an automatic phase moves every probe in a map, so every bake byte changes; an authored offset key with a warning is the cheap form. It cannot satisfy every mover at once.
   - *IP:* placement of static probes only; no visibility data.
3. **Give enclosed mid poses a dark value state**, now that geometry knots bracket each enclosure within 1.4 u, so floor-weight shares carry black instead of arena light.
   - *Effect:* expected to remove the enclosed probes' share of ceiling part (a); unmeasured, and the above-roof share would remain unless direction 1's one-cell variant also lands.
   - *Cost:* compiler rule, rebake, AC 12 and the brief's enclosing rule restated (an owner ruling). Risk: the dark-sphere artifact that rule was written to stop; the terminus rule and AC 1 would still leave the band at rest.
   - *IP:* stores absolute value states blended by mover position; no flag, no zeroed weight, floor unchanged.
4. **White square: light the door from the pool, or accept it.** Check whether the back-room spot is promoted for the door in play; a promoted light shades per fragment with a crisp cone. Otherwise it is the direct-SH resolution limit on any mover face, or an authoring matter (move the cone edge off the door).
   - *Cost:* investigation first. *IP:* none new.
5. **Probes that ride the mover (root fix, own brief).** Bake each pose's light at points fixed to the mover and blend by t′, so a face never crosses a lattice layer.
   - *Effect:* removes the hand-over entirely.
   - *Cost:* new bake output, format, sampler path and memory. *IP:* the same N-absolute-states-by-mover-position shape, but a new stored structure, so it needs its own `ip-check` read before any build.
6. **Long-interval direct interpolation** (already a filed follow-up): head is 9–14 per block off truth inside the sweeper's long intervals. (The west-band gap at t′ 0.70–0.92 first listed here was a truth artifact; see §2.) It makes head smoother than truth, so it is not what the owner described.
