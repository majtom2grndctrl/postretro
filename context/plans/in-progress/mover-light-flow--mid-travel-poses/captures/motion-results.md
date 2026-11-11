# Mid-travel poses in motion: what makes the playtest artifacts

Branch `feat/mover-light-flow--mid-travel-poses` at `ebb65f313`. Release `prl-build` and `postretro --features capture` built at HEAD (`motion/bin/`). Offscreen captures with `POSTRETRO_SH_STREAMING=sync-proof`, `force_full_resident_sh_compose`, `reflection_quality: off`, and one forced mover. Diagnosis only: no engine or compiler code was changed.

## Columns

All PRLs were baked cold with `--release`, each with its own fresh `--cache-dir`. Bake logs are in `motion/logs/bake-*.log`.

| Column | What it is |
|---|---|
| head | The maps as committed. |
| two | Map copies with `"light_poses" "2"` on `setpiece_room` and `em_sweeper`. The other movers are unchanged. Only id 57 differs from head; the section diff confirms it. |
| nodepth | head with id 57's depth block emptied (u32 count 0, no tiles). I used my own walker of the epoch-5 layout (`motion/tools/strip57.py depth`). The committed `strip57.py` only removes the whole section. |
| strip | head with id 57 removed: the mover-absent frame. |
| dense, densenodepth (added) | Map copies with `light_poses` 11 on the room (knots every 0.1) and 15 on the sweeper (knots every 1/14). The dense knots include every head knot, and at those knots head and dense agree within 0.2 luma in every view. A dense-only knot therefore shows what a stored state reads at head's interval midpoints. |

## Knots

| Mover | Poses | Knots (id-57 records, head) |
|---|---|---|
| setpiece_room | 6 | 0, 0.2, 0.4, 0.6, 0.8, 1. Interval midpoints at 0.1, 0.3, 0.5, 0.7, 0.9 |
| em_sweeper | 8 | k/7: 0, .143, .286, .429, .571, .714, .857, 1. Midpoints at (2k+1)/14 |
| setpiece_wall (unforced) | 15 | k/14 |
| em_lift, em_crusher_open, em_crusher_wall (unforced) | 4, 6, 9 | k/3, k/5, k/8 |

- Neither map sets `light_ramp`, so t′ = t.
- Mid-pose states skipped (Build Summary):
  - setpiece_room: bounce 2111 of 3093, depth 1468 of 3336.
  - em_sweeper: bounce 807 of 1748, depth 1289 of 1698.
  - A probe whose mid states are skipped blends over a longer interval than the knot spacing.

## Grid, method and deviations from the brief

- **Grid.** Room views use t′ 0 to 1 in steps of 0.02 (51 samples); the room's knots and midpoints fall on that grid. Face views use steps of **1/70** (71 samples) so the sweeper's knots k/7 and midpoints fall on the grid. A 0.02 grid does not hit them. This is a deviation from the brief's 0.02.
- **Pilot.** One t′ per view per column. Every scene loaded, and frames differ across t′ and between columns. head ≠ strip, with per-pixel differences of 6 to 57 levels.
- **Cost.** About 0.75 s per capture. All 1,180 captures in the four-column grid took 6 minutes at 2 jobs, plus 568 for the dense columns. No coarsening was needed.
- **Ceiling view.** The room's inner box is x 848–1136 and y 176–464. The ceiling underside sits at z −16 + 176t. The camera is at (870, 320, floor + 24), pitched up 60°, FOV 100. The crop is z = ceiling, x 856–1128, y 184–456.
- **Step metric, room views.** 8×8 pixel blocks. Floor, ceiling and interior cameras ride the room, so blocks stay fixed on the surface.
- **Step metric, faces.** cap.py's east and west cameras are fixed while the slab moves, so pixel blocks would track a moving texture. Steps on the faces therefore use 8×8-unit **face-space cells** instead (mean of pixel centres that hit each cell; at least 12 pixels). 12c and 12d use cap.py's pixel blocks.
- **Kinks.** Besides |Δ| between consecutive samples, I report the per-cell |second difference|. Piecewise-linear compose shows up as kinks at knots, not as steps.
- **Knot window.** Ceiling extra samples at 0.4 and 0.6, ±0.001 … ±0.015, head and nodepth.

## Per artifact

Luma is Rec. 709, 0–255. "Mean" is the crop mean, or the frame mean for the interior.

### Room ceiling: knot blink, with lattice dots between knots

- **head ≡ nodepth everywhere except at knots.**
  - At every non-knot grid sample, head − nodepth is exactly 0.00.
  - At knots 0.2 / 0.4 / 0.6 / 0.8, head drops 6.4 / 6.0 / 4.3 / 1.7 below nodepth (per block up to −29 / −27 / −23 / −18).
- **The dark window is extremely narrow.** Around knot 0.4 the mean reads:

  | t′ | 0.395 | 0.398 | 0.399 | 0.400 | 0.401 | 0.402 | 0.405 |
  |---|---|---|---|---|---|---|---|
  | head | 8.20 | 7.20 | 6.41 | **1.38** | 6.67 | 7.11 | 7.35 |
  | nodepth | 8.32 | 7.75 | 7.55 | 7.36 | 7.36 | 7.36 | 7.36 |

  - Knot 0.6 behaves the same: 3.30 at the knot against 7.17 at 0.601.
  - Depth has its full effect only within about ±0.001 t′ (±0.18 units of travel, ±4.5 ms at the room's 1 m/s), and fades by about ±0.005.
  - At 60 fps, a frame lands in that window only sometimes. In play this reads as a sporadic one-frame dark flash at each of the 4 mid knots, as well as on leaving each terminus.
- **Between knots the ceiling shows a probe-lattice dot pattern.**
  - 12c peaks: head 20–30 at the midpoints 0.3–0.9, and 8 / 0 / 0 at knots 0.4 / 0.6 / 0.8. Knot 0.2 reads 26, on a near-black frame (mean 0.61).
  - nodepth has 17–31 at every sample from 0.2 to 0.9. two has 0–3 from 0.2 on.
  - All three columns read 0 at 0.1.
  - See `ceiling-sheet-t0.400-x6.png`: uniform dark at 0.400, dots at 0.38 and 0.42, uniform light grey in two.
- **Steps.** The largest per-block |Δ| are 27–29 on both sides of knots 0.2 and 0.4, 23–24 at 0.6, and 18–19 at 0.8. The median elsewhere is 1.8. nodepth peaks at 16 at t′ 0.13. Second difference at knots: 58 / 54 / 48 / 37.
- **Against the past.** head's mean is far darker than two: −31 at 0.2, −24 at 0.4, −16 at 0.6, −9 at 0.8. two shows no dots, though; the dots are new. The leak the owner sees is spatial (dots) plus the blink, not a brighter mean.
- **Interpolation of bounce and direct is not the issue here.** At head's midpoints, nodepth − densenodepth is −0.03 to +0.00 in the mean (per block within ±0.9, −2.9 at 0.9). At those t′ the dense build blinks dark exactly as head does at its own knots: dense − densenodepth is −5.9 / −5.7 / −4.5 / −3.5 / −3.7.
- **At the termini.** The blink exists there in two as well, i.e. pre-brief. head at 0 / 1 reads 0.50 / 8.41 against nodepth 4.78 / 11.35. The step leaving t′ 0 is 11–12 in head and in two.

### Room floor: steps at probe-layer crossings; brighter than two from stored states

- **Big steps sit at probe-layer crossings, not at knots.**
  - The floor top z = −176 + 176t passes the lattice layers z = −39.37k at t′ 0.105, 0.329, 0.553 and 0.776.
  - The largest head steps: 0.32–0.34 (22.7), 0.54–0.58 (20–22), 0.78–0.80 (14.6). nodepth is identical there.
  - Mean ramps: head +6.8 over 0.32→0.38 and +8.0 over 0.54→0.58, against +1.3 and +2.0 in two.
  - **strip has the same steps:** 0.10–0.12 (24.8), 0.32–0.34 (25.7), 0.56–0.58 (16.1).
  - two has a 41-level step at 0.78–0.80 (its mean jumps 7.6 → 30.5).
  - These are the static probe field's hand-over as a moving surface passes a layer. They predate this brief.
- **Depth on the floor is small, and brightens.** head − nodepth is +0.09 / +0.79 / +1.86 / 0.00 at knots 0.2 / 0.4 / 0.6 / 0.8 (per block up to +3.5 and +6.8). Depth exactness at a knot brightens the floor rather than darkening it. That is surprising and I have not explained it.
- **Leak against the past: head is brighter than two from t′ 0.34 to 0.78, then darker.**
  - head − two: +7.0 at 0.4, +4.9 at 0.5, +13.7 at 0.6, +14.7 at 0.7, +17 at 0.78.
  - From 0.8: −7.2 at 0.8 and −7.7 at 0.9.
  - The bright region is a white (world-light) patch near the west wall and door, on the camera's left (`floor-sheet-t0.600-x3.png`). In two the same area is dim and warm (the carried lamp).
  - head equals dense at every head knot (max |diff| 0.19), so the extra light is in the **stored posed states**, not in the blend.
  - A plausible cause I did **not** verify: the door (west wall gap y 272–368, top at z −48 + 176t) starts to clear the arena floor at t′ ≈ 0.27, so arena light may really enter. Only a static-brush reference would settle whether the stored states are right.
- **Interpolation error.**
  - At head's midpoint 0.5: nodepth − densenodepth is −4.3 mean (per block −15). head reads darker than the stored state there.
  - At 0.7 and 0.9: mean −0.1 and −0.2, but per-block ranges of −5.2 to +5.8 and −11.6 to +15.5. These are local patches.
- **12c.** At knots and midpoints from 0.4 to 1.0, head's bright excess is 6.8–10.1 with 5–11 peaks; two's is 5.8–8.9 with 5–11 peaks. Similar counts.

### Room interior (frame mean)

- head is darker than two through most of the trip: about −3 at 0.2–0.55, −6.1 at 0.8, −4.5 at 0.9. It is +0.1 to +1.8 at 0.7–0.78.
- Depth at knots: mean −0.2 to +0.3, per block −11 at 0.2.
- Interpolation at the midpoints: mean within −1.3, per block −12.6 at 0.5 and up to ±15 at 0.9.
- Steps fall at the floor's layer crossings, as on the floor, and at the knots 0.2 / 0.4 (kinks 23 / 19 in head, absent in nodepth: the knot blink).

### em_sweeper east face (toward the crushers): a sawtooth with the knot period

- **A sawtooth.**
  - head rises roughly linearly through each interval over t′ 0–0.57, then drops sharply where the face crosses a lattice layer. The layers are at x = 1338.6, 1377.9, 1417.3 and 1456.7 (face at 1312 + 256t), crossed at t′ 0.104, 0.257, 0.411 and 0.565.
  - The minima fall at the knots: 0.086 → 0.143 goes 59.8 → 52.1; 0.243 → 0.286 goes 58.0 → 47.3; 0.400 → 0.429 goes 54.3 → 45.0; 0.543 → 0.571 goes 53.9 → 46.1.
  - nodepth has the same shape. head ≡ nodepth off-knot.
  - Past t′ 0.6 the face is nearly flat (~46–47).
  - Period 1/7 t′ is about 0.2 s at the sweeper's 4.5 m/s, so the face pulses about 5 times a second while it moves.
- **It is interpolation, measured against the dense build's stored states.** nodepth − densenodepth at head's midpoints:

  | t′ | 0.071 | 0.214 | 0.357 | 0.500 | 0.643 | 0.786 | 0.929 |
  |---|---|---|---|---|---|---|---|
  | Mean | **+10.0** | **+9.7** | +7.1 | +5.7 | +0.3 | −1.1 | −1.7 |

  - Per cell up to +14.6, +15.5, +11.1 and +9.6 over the first four.
  - At head's knots, head − dense is within 0.1.
  - So between knots head reads up to about 10 levels **brighter** than the stored state at that pose. The dense build still sawtooths, with about half the amplitude, around a darker curve (`east-dense.png`).
- **Depth at knots:** −1.1 / −1.9 / −2.3 / −2.6 at 0.143 / 0.286 / 0.429 / 0.571 (per cell down to −6.3). The second difference at knots is 8.6–15.7 in head, against 3.3–4.2 in nodepth (except 11.8 at 0.857, a layer crossing).
- **Against the past.**
  - head is darker than two by 1.9–10.5 over 0.26–0.86, with sawteeth that two lacks (two steps only at 0.857–0.871, 13.7).
  - Against strip, 12d is −6 to −8 at knots and −1.2 to −5 between. The brief restates that the slab really shades these probes, so strip reads too bright here.

### em_sweeper west face

- **head is close to strip all trip.** 12d mean is within ±1.3, dark max ≤ 1.9, with 0 pits.
- **two shows the pre-brief pit:** 12d −15.3 with 15 pits at 0.286 and −13.0 with 17 pits at 0.357. This brief fixed the west face's large error.
- **Steps.** The largest head steps (10.6, 10.3, 6.5, 6.3) are at 0.386–0.400, 0.086–0.100, 0.857–0.871 and 0.543–0.557. strip has them at the same size (10.4, 10.4, 7.0, 6.7): lattice crossings.
- **Depth blink at knots** brightens slightly: mean +0.3 to +1.2, per cell up to +2.6.
- **Knot-locked kinks.** The second difference at knots 0.286 / 0.429 / 0.571 is 3.0 in nodepth (strip 0.9–1.4, two 1.5–1.6), and 6.3–6.9 in head.
- **Interpolation at midpoints:** mean +0.1 to +1.5, per cell ≤ +3.0.

## Verdicts

- **(a) Depth exact only at knots: confirmed. High confidence.**
  - head equals nodepth to the pixel at every off-knot sample, in all five views.
  - The depth effect lives in a window about ±0.001 to ±0.003 t′ wide around each knot (ceiling: 1.38 at 0.400 vs 6.41 at 0.399 and 6.67 at 0.401; nodepth 7.36–7.55).
  - It is the ceiling's main artifact:
    - a dark flash at each knot (mean −4 to −6, per block up to −29);
    - the lattice-dot leak between knots (20–30 peaks against 0 in two).
  - On the floor, interior, east face and west face it is a 0.2–2.6-level mean blink at knots (per cell up to 11 in the interior). It is darker on the east face and slightly brighter on the floor and west face.
  - The "interval midpoint" part of the prediction is moot: depth is already fully permissive 0.005 t′ from a knot.
  - The mechanism predates the brief at the termini; two shows the same step leaving t′ 0 and 1. Mid poses add 4 such blinks per room trip and 6 per sweeper trip.
- **(b) Piecewise blend across a crossing: confirmed on the east face; minor on the floor and interior; absent on the ceiling. Medium-high confidence.**
  - East face: the stored state at head's midpoints is 5.7–10 levels darker in the mean (up to 15.5 per cell) than head's blend over t′ 0–0.5. The error peaks late in each interval and ends at the lattice crossing.
  - Floor: −4.3 mean at 0.5 (−15 per block), ±15 per block at 0.9.
  - Ceiling: ≤ 0.03 mean.
  - Frames cannot split direct from bounce. Task 6b's CPU numbers (direct worst 49.6 vs bounce 2.07 between knots in the room) point at direct.
  - I also cannot split the plain interval lerp from the enclosed-mid-pose "blend across" rule. The shape fits the latter: layer L ramps toward its far-side (lit, west-of-slab) state while still in front of the east face, then drops out as it is crossed. That is an inference, not a measurement.
- **(c) AC 7 patch-grid error on the west face: refuted as a visible, west-only cause. Medium confidence.**
  - Knot-locked kinks without depth are present on **both** faces, about 3.0 per cell on the west and 3.3–4.2 on the east, against about 1 in strip. That is the generic piecewise-linear signature.
  - No west-only step stands out. The west face's per-cell interpolation error is ≤ 3.0. If the 4.7-level probe error exists, it reaches the face at about 2 levels or less per cell.
- **(d) Other causes found.**
  1. **Probe-layer hand-over steps.** Floor, interior and both faces step where a moving surface passes a static probe layer: floor t′ 0.105 / 0.329 / 0.553 / 0.776, faces at the lattice x crossings. strip and two show them as well (floor strip up to 25.7 per block). They predate this brief, but head's higher contrast between layers makes them more visible on the floor: +6.8 and +8.0 mean ramps at 0.32–0.38 and 0.54–0.58, against +1.3 and +2.0 in two. strip ramps +17.9 and +7.5 there.
  2. **Brighter floor from the stored states (+5 to +17 over two at t′ 0.34–0.78).** It is identical in dense at the knots, so it is bake output, not compose. Whether that light is right needs a static-brush reference. A door clearing the arena floor from t′ 0.27 is a plausible real light path.
  3. **Depth at a knot brightens some surfaces** (floor +1.9 mean at 0.6, per block up to +6.8; west face up to +2.6). This is unexplained.

## Not measured

- No static-brush reference frames, so I cannot say which of head or dense is right in absolute terms, only how far head's blend lands from its own stored states.
- No direct-only or bounce-only strip, so (b)'s direct and bounce shares are not separated.
- Low fidelity was not captured; all builds are `--release`.
- Real-time frame pacing was not simulated; the flash-frequency claims come from the measured window width and the FGD speed (m/s).
- I did not check that `two` is byte-identical to the parent build (t11 reported AC 2); here only id 57 differs between head and two.

## Files

All under `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/motion/`.

- **Charts**
  - `charts/{floor,ceiling,interior,west,east}-luma.png`: means vs t′, four columns, knot and midpoint ticks, head − two / nodepth / strip.
  - `charts/{…}-steps.png`: per-cell max and p95 |Δ| and max |second difference|.
  - `charts/{…}-dense.png`: adds the dense columns.
- **Sheets** (head / nodepth / two rows, ±2 samples; room sheets shown at gain)
  - `sheets/ceiling-sheet-t0.200-x6.png`, `ceiling-sheet-t0.400-x6.png`
  - `sheets/floor-sheet-t0.340-x3.png`, `floor-sheet-t0.600-x3.png`
  - `sheets/interior-sheet-t1.000-x4.png`, `interior-sheet-t0.200-x4.png`
  - `sheets/west-sheet-t0.400.png`, `west-sheet-t0.286.png`
  - `sheets/east-sheet-t0.571.png`, `east-sheet-t0.143.png`
- **Data**
  - `results/{view}.json`: per-t′ means, 12c and 12d, steps, kinks.
  - `results/dense.json` and `results/dense.txt`: six-column means.
  - `results/percell.txt`, `results/knots.json`.
- **Tools:** `tools/` (`mcap.py`, `mmetrics.py`, `mcharts.py`, `mreport.py`, `mdense.py`, `mdensechart.py`, `strip57.py`, `knots.py`, `secdiff.py`, `bake.sh`).
- **PRLs:** `ws/content/dev/maps/`.
- **Captures:** `cap/`.
