# Owner's three in-engine observations after the amendment: a diagnosis

Branch `feat/mover-light-flow--mid-travel-poses` at 17532c316, release `postretro --features capture` and `prl-build` built at HEAD (copies in `opus/bin/`). No tracked file was edited. Every path below is under `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/opus/`.

## Bottom line

- **On all three surfaces, HEAD renders what a dense even-pose bake renders.** I baked dense truth PRLs with `light_poses` 177 on the room (1 u per pose), 273 on the wall (2 u) and 113 on the sweeper (2.3 u). Against them, HEAD differs by:
  - **Wall view:** mean |Δ| 0.02 luma over 201 frames.
  - **Sweeper crop:** mean 0.51 luma. The pre-amendment build is 4.51 (max 13.45).
  - **Room ceiling view:** mean ≈ 0.05 luma up to t′ 0.70. One known exception follows.
- **The pulses and steps the owner sees are present in the truth.** They come from the probe lattice, not from interpolation between poses.
  - A probe plane sits inside, or hugs, each of these thin moving slabs.
  - When the slab's face or edge passes that plane, a probe that carries most of the trilinear weight of the slab's nearby fragments flips between enclosed (dark tile, 0.03 floor) and open. When open, it is often lit from the far side of the slab.
  - The result is one event per lattice step of travel:
    - room ceiling: 1 m/s, so about 1 Hz;
    - wall-door edge: 2 m/s, about 2 Hz;
    - sweeper face: 4.5 m/s, about 4.6 Hz.
- **The amendment made these events sharp and exact.** Geometry knots resolve each one within about 1 u of travel. Before the amendment they were smeared over 0.07–0.2 t′ intervals and buried under knot blinks and interval drift. Most "flat edges" lost their pulse because no lattice plane is trapped inside those slabs.
- **One real channel error remains: the rising-room ceiling at t′ 0.70–0.96.**
  - HEAD reads up to +21.5 luma per 16 px block, and +6.6 band mean, brighter than the truth.
  - It is all in the indirect term.
  - Cause: a bounce interval left long by skipping. The responsible probe stores no mid-pose bounce state from t′ 0 to pose 37 (0.806), then nothing until pose 40 (0.917).
  - This is the known long-interval residual ("room floor drift 0.806–0.892"), now visible on the ceiling as well.

## 1. Rising-room ceiling above the door: "a pulsing light leak"

### Mechanism

1. **The probe lattice cuts through the room's west wall.**
   - Id 34's grid origin is at map (1280, 1600, −192), with a 39.37 u cell (`opus/tools/prl.py`, from `sh_volume.rs:126`). The x planes sit at 1280 − 39.37k.
   - Plane x = 846.93 lies inside the room's west wall (x 832–848), 1.07 u from its inner face.
   - On the other three walls the nearest plane is either inside the room (5.7–13.5 u in) or inside static solid (invalid, weight 0). That is why only the door wall pulses.
2. **That plane dominates the ceiling edge.**
   - A ceiling fragment at x ≈ 850 puts 0.92 of its x-trilinear weight on x = 846.93.
   - In z, its upper corners lie on the next layer above the roof underside.
3. **Over each lattice cycle, the upper probe moves from open air to inside the roof.**
   - While the roof top is still below layer L, the probe at (846.93, y, L) is open arena air above the room. Its indirect is bright arena bounce.
   - The ceiling's lookup point sits 0.1 cell (3.9 u) below the ceiling (`kinematic_brush.wgsl:246`). So the probe's trilinear z-weight on the ceiling grows linearly as the roof rises toward it, from 0 when the roof top reaches L − 19.4 u to 0.49 when the roof top reaches L.
   - The mover backface weight floors it at 0.03, and its depth tile also holds it near the 0.03 floor (Chebyshev). Neither weight reaches 0 (lever 3: `sh_sample.wgsl:40` `sh_chebyshev_visibility`, `:266` `SH_BACKFACE_SMOOTH_FLOOR`, `:294` `sh_probe_weight`).
   - The ceiling fragment's other corners are also weak:
     - the lower wall-plane probe is enclosed by the lintel or wall for most of each cycle, so it carries the dark tile at 0.03;
     - the in-room probes at x = 886.3 carry only (x − 846.93)/39.37 ≈ 0.08 trilinear weight.
   - After renormalisation, the floored but much brighter probe above the roof takes a visible share.
   - The roof encloses the probe when its top reaches L. Inside knots then store the dark tile, values blend across, and the band drops.
4. **Timing confirmation.**
   - Ramp starts are predicted at (L − 19.4)/176 = 0.141, 0.365, 0.588, 0.812. Drops are predicted at L/176 = 0.251, 0.475, 0.699, 0.922, for L = 44.2, 83.6, 123.0 and 162.3.
   - Measured, on the band over the solid wall: ramps start at 0.13, 0.355, 0.59 and 0.81; drops fall at 0.245–0.26, 0.465–0.48, 0.69–0.70 and 0.915–0.93 (`opus/band.json`, chart `opus/charts.png` top).
   - The band spans the whole west wall, not only the door. From the owner's camera the door segment is what shows.
5. **Light term.** The band is 100% indirect:
   - `light_term_mask` 0x08 (baked direct) gives 0.00–0.22 luma;
   - 0x02 (static indirect) reproduces the band: 8.04 / 15.60 at t′ 0.23 for HEAD, against 8.00 / 15.57 for the truth.

### Experiment

View `door_ceiling`: the camera rides the room inside, looking up and west at the ceiling over the door. Pilot frames match the owner's shot 1 (`opus/door_grid.png`). Columns at t′ 0:1:0.005:
- head: HEAD;
- truth: 177 poses;
- pre: the pre-amendment PRL on the pre-amendment engine;
- strip: id 57 removed;
- nodepth: id 57's depth block emptied.

| Band, seg B (luma) | t′ 0.13 | 0.24 (peak) | 0.26 (after drop) | 0.46 (peak) | 0.48 |
|---|---|---|---|---|---|
| head | 3.87 | 15.80 | 11.97 | 15.79 | 12.21 |
| truth | 3.87 | 15.88 | 11.96 | 15.87 | 12.20 |
| pre | 24.7 | 34.3 | 33.7 | 32.2 | 31.6 |
| strip | 2.9 | 4.5 | 4.5 | 12.1 | 12.1 |
| nodepth | 22.8 | 35.5 | 35.0 | 35.1 | 34.5 |

- Pre-amendment was 2–3× brighter with knot blinks (6.5 at t′ 0.2, 12.8 at 0.4).
- Without mover depth, the leak is about 35 luma throughout.
- Depth reduces it to the truth's sawtooth, which is what remains.
- Whole-frame comparison (`opus/vs_ceiling.txt`): HEAD vs truth mean |Δ| is 0.01–0.16 per frame up to t′ 0.70. Pre vs truth is 2.7–10.1, with worst blocks of 17–31.

**Late-trip residual (t′ 0.70–0.96).**
- HEAD vs truth reads 0.21–1.09 mean |Δ|, with worst block +21.5 at t′ 0.84 on the ceiling edge.
- Band at t′ 0.84, seg A / B: HEAD 12.40 / 11.22, truth 8.82 / 4.88.
- With the indirect mask: HEAD 12.15 / 11.17, truth 8.61 / 4.82. So the excess is bounce.
- Id-57 bounce marks for the probe (846.93, 300.8, 162.3), index 7341, room mover (`opus/tools/strip57lib.py` `bounce_marks`): stored only at poses 0, 37 (t′ 0.806) and 40 (0.917). Every other pose skipped.
- So that probe's bounce is a single lerp across 0 → 0.806, then 0.806 → 0.917.
  - The skip passed at each of HEAD's knots (`skip.rs:245` `skip_bounce`, judged per knot only).
  - The truth dips between those knots (seg B 2.65 at 0.81, where HEAD reads 7.15).
- The even knot at 0.8 was merged into the geometry knots 0.7967 / 0.8063, which is legal under `KNOT_MERGE_TOLERANCE_M` (`geometry_knots.rs:44`). Nothing then lies between 0.8063 and 0.8923.

## 2. Wall-door edge pulse, and the "upper right white square"

### Mechanism

- **A probe plane runs inside the door slab.** Plane y = 1048.8 runs inside the slab (y 1040–1072), 8.8 u from its arena-side face.
  - As the trailing edge (x = 96 + 544t) passes each plane x = 1280 − 39.37k, that plane's probe exits the slab into the opened doorway, where it sees the arena and its bright spot.
  - The probe carries 0.31 of the y-trilinear weight of north-face fragments, after the 0.1-cell normal offset in `kinematic_brush.wgsl:246`. So the strip of the north face beside the edge brightens by about 8–10 luma within 2–4 frames, then decays over about 0.05 t′ as the edge moves away from the plane.
  - This is the same class of event as the ceiling: a lattice plane trapped in a thin moving slab flips from enclosed to open.
- **The upper-right white square is the rest look, not new.**
  - At t′ 0 the square's luma is identical in HEAD, truth and pre: 20.86, with column thirds [17.96, 23.78, 20.85]. AC 1 already proves rest bit for bit against the pre-change build.
  - The blotch is mostly baked direct: direct-only reads 16.61, against 26.26 in strip.
  - That fits the documented, accepted cost "at rest an enclosing wall's inner faces read the base's direct light" (`build_pipeline.md` step 12, An enclosing terminus reads the base). The in-wall probes at y = 1048.8 read the mover-absent direct light, which includes the arena spot. Through the 0.03 floor and their 0.31 trilinear share, they paint a probe-scale bright patch on the north face.
  - The patch moves with the slab mid-travel. HEAD equals the truth there too: 21.17 vs 21.16 at t′ 0.03.

### Experiment

Views:
- `wall_back`: a fixed camera in the back room, yaw −60;
- `wall_owner`: the owner's position (354, 1339, 79), yaw −115, which matches shots 2/3 (`opus/wall_owner_full_s.png`).

Columns at t′ 0:1:0.005 as above, truth at 273 poses.

- Whole frame (`opus/vs_wall.txt`): HEAD vs truth mean |Δ| 0.02, worst block 12.7 at t′ 0.58. Strip vs truth: 3.49 mean.
- North face beside the trailing edge, upper row (`opus/tools/wallpts.py`, region `edge_hi`; chart `opus/charts.png` middle):
  - HEAD and truth rise together at every lattice crossing: t′ 0.44–0.45, 0.515–0.52, 0.585–0.59, 0.655–0.66, 0.725–0.73, 0.795–0.80, 0.865–0.87. Predicted crossings: 0.440, 0.5125, 0.585, 0.657, 0.730, 0.802, 0.874.
  - Example: 0.51 → 0.52 reads 29.2 → 38.1 in HEAD and 29.0 → 38.1 in truth.
  - Pre-amendment shows almost no sawtooth (±1–2, smeared). Strip is flat at about 40–46 (mover absent).
  - So the pulse is new relative to the pre-amendment build, and correct relative to the truth.
- One small HEAD-only deviation: at several crossings HEAD starts the rise one sample (0.005 t′, about 2 frames at 2 m/s) before the truth.
  - 0.58: 37.1 vs 30.0;
  - 0.65: 35.7 vs 30.4;
  - 0.72: 33.0 vs 29.0;
  - 0.79: 30.2 vs 26.9.
  - This is the inside → outside knot window near the crossing, against the truth's 2 u pose spacing. It is minor next to the pulse itself.

## 3. em_sweeper face toward the crushers: rainbow shimmer that now "jumps"

### Mechanism

- **The colour is static content.** The red/orange/yellow speckle on the normal-mapped east face is present with id 57 stripped: the strip column's mean chroma 4.27 sits next to HEAD's 4.37 (`opus/sheet_sw_cross.png`).
- **The jumps are the face crossing probe layers, and the truth has them.**
  - The 32 u slab is thinner than the 39.37 u lattice, so the moving face crosses a probe plane every 0.154 t′.
  - At each crossing, the probe that enters the slab loses its weight: the backface weight drops to its 0.03 floor, and the inside knot's dark tile floors it too. Normalisation then hands the face to the next plane out, all within about 1 frame (3 u of travel per 60 fps frame).
  - Strip shows the same steps, so this exists without the channel: 2.20, 1.32 and 1.55 mean |Δ| per frame at t′ 0.095, 0.245 and 0.405.
- **Why it reads as "jumps" now and "continuous" before.**
  - The pre-amendment build drifted between crossings at about 0.8 luma per frame: long-interval lerp error, up to 13.45 luma off the truth.
  - It also had larger smeared crossing transients: 3.4–5.5 per frame over 2–3 frames.
  - HEAD between crossings changes at the strip-like baseline, 0.3–0.5 luma per frame, which is mostly texture parallax. Its crossing jumps match the truth: 2.95 vs 3.34 and 1.67 vs 1.78. Some are smaller than the truth's (the truth spikes at t′ 0.02, 0.15, 0.29 where HEAD does not).
  - So the shimmer now holds, then jumps at each lattice crossing: a 4–5 Hz flipbook. That is the model's correct output with the drift removed.

### Experiment

View `sweeper_east`: the owner's position (1654, 197, 79), looking west. Captured at t′ 0:1:0.005 and 0.225:0.285:0.001; per-frame proxy at a 0.01 t′ lag (`opus/tools/steps.py`; chart `opus/charts.png` bottom; `opus/sw_truth_steps.txt`).

| Crop (x 400–680, y 220–350) | pre | head | truth | strip |
|---|---|---|---|---|
| Σ\|ΔL\| over trip (0.01 lag) | 85.9 | 61.9 | 68.0 | 39.6 |
| Max per-frame ΔL | 5.47 | 2.95 | 3.34 | 2.20 |
| Mean \|L − truth\| | 4.51 | 0.51 | — | 8.17 |

- Fine window at t′ 0.252–0.272:
  - HEAD has a one-frame step of +0.54 at t′ 0.262–0.263, at the outside knot 0.26227. Nodepth lacks it, so it comes from depth.
  - The truth has an equal step (+0.49) at 0.267–0.268.
  - Both stay within 2 luma of each other: |HEAD − truth| ≤ 0.97 mean, ≤ 2.0 max.

## Shared cause?

**Yes for the visible pulses and steps.**
- One mechanism covers the ceiling sawtooth, the wall-edge pulse and the sweeper steps: a probe plane inside or against a thin moving slab flips between enclosed and open at each lattice crossing, while it holds most of the trilinear weight of nearby mover fragments.
- Its visible size comes from the non-zero weight floors (Chebyshev and backface at 0.03) and from renormalisation against weak in-room corners.
- These are renderer sampling properties of the static 1 m lattice. Every frame, the truth reproduces them to within ~0.05–0.5 luma.
- The amendment did not create them. It removed the knot blinks and interval drift that masked them, and its geometry knots now time the flips exactly.

**Separate causes:**
- the late-trip ceiling excess (t′ 0.70–0.96): bounce skipping leaves long intervals, a real channel error;
- the rest-state white square: the documented enclosing-terminus cost, unchanged since before the brief;
- the sweeper's colour speckle: static content.

## Confidence and what I could not confirm

- **High:** HEAD ≈ dense truth on all three surfaces (numbers above). The pre-amendment build was far off. The colour speckle is static. The white square is the rest look (identical across builds at t′ 0).
- **High:** lattice timing. Ceiling ramp and drop times, wall crossings and sweeper crossings all match the predicted plane crossings to within 0.01 t′.
- **Medium:** the per-probe attribution — the above-roof probe at x = 846.93 for the ceiling, the y = 1048.8 doorway probe for the wall.
  - It rests on geometry, timing and the light-term split. I did not isolate one probe, for example by invalidating it in a PRL copy. The loader would likely refuse a valid-mask mismatch with id 57.
- **Medium:** the late-trip excess comes from bounce skipping between HEAD's knots. Evidence: indirect-only excess, plus that probe's bounce marks. I could not rebuild with skipping off, since that would mean editing code.
- **Not confirmed:**
  - Windows real-time behaviour (async streaming, frame pacing). All captures are offscreen, forced t′, `sync-proof`.
  - Whether any stepping on the owner's machine comes from streaming rather than from the lattice.
  - My "truth" is itself a skipped, mean-and-variance bake at 1–2.3 u spacing. It is a much denser HEAD, not an exact continuous reference.
  - The owner's exact t′ in each shot.

## Candidate fix directions (no code), ranked

1. **Larger probe-lookup normal offset for mover receivers.** `kinematic_brush.wgsl:246/268` `SH_NORMAL_OFFSET_M` = 0.1 cell, against e.g. 0.3–0.5 cell. DDGI-style surface bias, optionally with a view-direction term.
   - *Effect:*
     - It moves the lookup point off the face, so a plane hugging the face loses its share.
     - Wall north face: the in-wall plane's y-weight falls from 0.31 to about 0.01 at 0.4 cell.
     - Ceiling: at 0.4 cell the above-roof probe's peak z-weight falls from 0.49 to about 0.19.
     - Sweeper: the entering probe's weight at a crossing falls from about 0.9 to about 0.6, so jumps shrink.
     - It addresses all three pulses at once.
   - *Cost:*
     - a shader constant, plus a capture A/B;
     - it changes rest frames on every mover, so AC 1 needs an owner ruling;
     - a longer offset can carry the lookup point through thin geometry in front of the face (a nearby wall or ledge), which brings its own leaks.
   - *IP:* a positional bias, with no flag, mask or zeroed weight, and it already exists at 0.1. `ip-check.md` lists Majercik et al. 2019 as prior art; that is the DDGI paper whose sampling bias this resembles. No rule against it was found in `ip-372` levers 1–8. Have the owner confirm.
   - Rank 1 because it is cheap, renderer-only, and targets the shared mechanism.
2. **Close the late-trip ceiling error.** Do not let geometry knots swallow even knots (0.8 merged away), or cap the longest stored interval per probe at about one lattice step. Alternatively, judge skips between knots as well as at them, using the depth score's distances, and give the bounce skip bar more reach on dark receivers (the known follow-up).
   - *Effect:* HEAD's +6–21 luma excess at t′ 0.70–0.96 falls toward the truth.
   - *Cost:* more stored mid-pose bounce states (bytes) and a small bake-time increase.
   - *IP:* storage sparsity and more absolute states blended by position, the accepted shape.
3. **Steer the probe lattice off thin moving slabs.** Use the id-34 grid origin or per-map alignment so no plane lies within a slab or within the normal offset of its faces. For example, the room's west wall is 1.07 u from a plane, and the wall door 8.8 u.
   - *Effect:* removes these specific cases.
   - *Cost:* map-specific and fragile, since other movers collide; it changes every probe.
   - *IP:* neutral.
   - Better as an authoring warning ("a probe plane lies inside mover X's slab") than as a fix.
4. **Temporal smoothing of mover-receiver indirect.** A per-fragment or per-probe exponential filter over frames, so a crossing spreads over about 0.1 s.
   - *Effect:* softens jumps without changing their cause.
   - *Cost:* ghosting and history storage, plus new state.
   - *IP:* not a visibility determination. Smoothing per-probe weights would need a check against lever 2 (no stored verdicts); smoothing the composed colour does not.
5. **Not recommended.**
   - Lowering the 0.03 floor: `ip-372` lever 3, "keep 0.03 until counsel answers".
   - Relocating probes per pose: runtime-varying probe positions sit outside `ip-check.md` Piece 1's offline-only clearance.
   - Any per-probe "enclosed/visible" bit at runtime: levers 2 and 7.

## Files

- **Tools:** `opus/tools/` — `cap.py` (views, `MASK` env for `light_term_mask`), `series.py`, `steps.py`, `band.py`, `wallpts.py`, `vs.py`, `sheet.py`, `prl.py`, `strip57lib.py`.
- **PRLs:** `opus/ws/content/dev/maps/` — head, nodepth and strip, truth (room 177, wall 273, sweeper 113), and pre (a symlink to the pre-amendment motion PRLs).
- **Outputs:**
  - `opus/charts.png`, `opus/band.json`, `opus/vs_ceiling.txt`, `opus/vs_wall.txt`, `opus/wallpts.txt`, `opus/sweeper_steps.txt`, `opus/sw_truth_steps.txt`;
  - sheets: `opus/door_grid.png`, `opus/wall_owner_full_s.png`, `opus/sheet_sw_cross.png`;
  - captures in `opus/cap/`.
