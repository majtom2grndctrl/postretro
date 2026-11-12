# Amendment 2: measurements for rulings A and B (Windows, 2026-11-12)

Measurement and prototype only. Nothing here is the final fix. Every number is relative and was taken on this machine: a GTX 1660 SUPER under Vulkan (wgpu's default pick), driver 621.14. Don't compare these numbers with the Mac's.

## Answers first

- **Ruling A (backface form).** Recommend the surface-point ramp with a **0.4-cell lookup offset** (`r40`). It is the only form that cuts the step on every hand-over surface:
  - sweeper east face 11.2 → 4.3 levels per frame;
  - wall end face 19.9 → 8.7;
  - ceiling band 4.7 → 1.2.

  It also tracks the dense truth at least as well as today's form. The ramp at today's 0.1-cell offset (`r10`) is **worse** than today's form on the sweeper and the wall end, so don't adopt the ramp without the larger offset. The offset alone (`o40`) moves the flip without removing it, and changes rest frames the most. `r25` is the fallback if the owner judges a 0.4-cell lookup too risky near thin static geometry.
- **Ruling B (in-wall layer).**
  - **The split is measured.** In the band over the solid west wall, the in-wall column's enclosed probes carry **77 %** of the light on average (62–99 %). The above-roof probes carry 12 % on average, but **up to 38 %** at each pulse peak. Above the door the split is 50 % and 11 % (peak 55 %).
  - **`r40` takes the pulse out of the band.** The band settles to a steady 5–7 luma (indirect only), and its largest step falls 4.7 → 1.2. A steady level remains, almost all of it from the in-wall column.
  - **Recommendation:** take (i), the authoring warning, now. Treat the lattice-phase key as an author's remedy that cannot always succeed. Hold (ii), the dark value state, until a bake-side prototype checks its dark-sphere risk. On its own, (iii) leaves the pulse.

## Setup

- **Branch and commit.** `feat/mover-light-flow--mid-travel-poses` at f712cfc14.
- **prl-build.** Release, built 09:41–09:46.
  - The concurrent worker's edits started landing during that compile. Only one could have entered it: `geometry_knots.rs`, a visibility-only change (`pub(super)` on `SIDE_GUARD_M`, `Sweep`, `Sweep::new`, `Sweep::spans`, `Span`).
  - Their `placement.rs` and `layer_bands.rs` edits (task 23's nudge) did not enter it. The build would not have compiled with them, and `layer_bands.rs` was created after the compile.
  - So my prl-build is semantically HEAD: no authored pose is nudged. The binary is kept in scratch.
- **Engine.** Release `postretro --features capture` with the prototype switch below, built in 28.5 min.
- **Bakes.** Each was cold, `--release --no-tui -v --cache-dir <fresh scratch dir> --baked-root <repo>/baked`, with its cache deleted after (`tools/bake.sh`).

  | Column | Map | Counts | Bake time |
  |---|---|---|---|
  | head | `enemy-movers` as committed | sweeper 31 (7 even + 24 geometry) | 18 s |
  | head | `mover-setpiece` as committed | room 45, wall 71 | 78 s |
  | truth | `enemy-movers` | sweeper `light_poses 113` | 25 s |
  | truth | `mover-setpiece` | room `light_poses 76`, wall `light_poses 267` | 233 s |
  | shift | `mover-setpiece`, `wall_east` inner face 1280 → 1248 (Fable's 32 u lattice shift) | room 35, wall 71 | 59 s |
  | strip | head setpiece with id 57 removed (`strip57.py all`) | — | — |

- **Truth counts and the band check** (`tools/bandscan.py`: every travel-axis brush plane at every pose against every lattice layer, flagged within 2.54 mm).
  - Room 76: one pose is near, pose 10 at 2.45 mm, the floor layer cleanly enclosed as Fable found.
  - Sweeper 113: nearest 2.29 mm.
  - Wall 267: nearest 3.22 mm. 267 is a clean count I picked near Opus's 273, which has eight poses within 2.54 mm.
  - No 177, 178, 78 or 41 truth was baked. Per the executor's correction, the wall's 267 is not one of the cleared counts, so its truth columns rest on my scan alone. My prl-build predates task 23's nudge, so no pose moved.
- **Captures.**
  - Settings: offscreen 960×540, `POSTRETRO_SH_STREAMING=sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover.
  - Views are ported from `fable-tools/ocap.py` and `opus-tools/cap.py`:
    - `sweepfix` rides the slab, square on to the east face;
    - `walledge` rides the wall and looks at the trailing end face and the north face beside it;
    - `door_ceiling` is Opus's owner-shot-1 view, with band segments A (above the door) and B (above the solid west wall) from `band.py`;
    - `floor` is task 12c's floor view and metric;
    - `wall_owner` and `sweeper_east` are owner poses, used for rest frames.
  - About 5,000 captures in all, run 4–6 in parallel at about 1 per second. No engine process was left running.
- **Tools.** Windows copies are in `tools/` here (`acap.py` and the rest). They take a scratch root from `A2_ROOT`, defaulting to the parent of their own folder, and the repo from `A2_REPO`. The frames (1.5 GB) stay in scratch.
- **Step metric.** For each 60 fps frame of travel, take the largest |Δluma| of any 8×8-pixel block in the region between consecutive frames. This is the diagnosis's metric, sampled at the real frame spacing:
  - sweeper: 2.95 u a frame, sampled every 1 u, at lag 3;
  - wall: 1.31 u a frame, sampled every frame;
  - room: 0.66 u a frame, sampled every frame.
  "n > 3" counts the frames whose step is over 3 levels. Rec. 709 luma, 0–255.

## The prototype switch

`POSTRETRO_PROTO_MOVER_SH`, read once when the mover pipeline is built. It rewrites prototype constants in the mover pass's composed WGSL. All wgpu stays in the renderer, and every other pipeline keeps the defaults, which are today's behaviour. Each capture is its own process, so it flips per capture. Captures fail unless stderr confirms the rewrite.

| Form | Spec | What it is |
|---|---|---|
| `cur` | (unset) | Today: ((cos + 1)/2)⁸ + 0.03, cosine taken from the lookup point, lookup 0.1 cell off the face |
| `r10` | `form=1;ramp=0.1;offset=0.1` | Backface = smoothstep(0, R, s) + 0.03, where s is the probe's signed distance from the **surface point** along the geometric normal, in cells. R = 0.1, lookup offset 0.1 |
| `r25` | `form=1;ramp=0.25;offset=0.25` | The same with R = offset = 0.25 cell |
| `r40` | `form=1;ramp=0.4;offset=0.4` | The same with R = offset = 0.4 cell |
| `o40` | `offset=0.4` | Today's cosine, with the lookup offset at 0.4 cell (option B iii) |

- The ramp width R equals the offset by design. A probe beyond the lookup point gets full weight, one behind the surface gets the 0.03 floor, and one in between is ramped.
- The hand-over between two layers then spans k cells of travel, k being the offset:
  - 3.9 u at 0.1, which is still about one sweeper frame;
  - 9.8 u at 0.25;
  - 15.7 u at 0.4.

  That is why `r10` alone cannot work: it squeezes the hand-over into the offset and adds the depth switch at enclosure on top.
- Also measurement-only: `attr=1|2;min=…;max=…`. It keeps or drops the **numerator** of the mover indirect for probes inside an engine-space box; the weights and their sum are untouched. It exists only to attribute the band (see the IP check).

## Ruling A: tables

Head bake. "vs truth" is the mean / worst |head − truth| per block under the same form, averaged over the window. "Look change" is the mean |form − `cur`| per block on the head bake. Full output: `analyze.txt`.

**Sweeper east face** (`sweepfix`, t′ 0–0.5, three layer crossings, 4.5 m/s):

| Form | Step max | n > 3 | vs truth | Look change |
|---|---|---|---|---|
| cur | 11.2 | 23 | 0.71 / 10.0 | — |
| r10 | 14.4 | 14 | 0.67 / 9.1 | 0.49 |
| r25 | 7.4 | 17 | 0.67 / 9.1 | 0.52 |
| **r40** | **4.3** | **12** | 0.66 / 9.2 | 0.78 |
| o40 | 8.9 | 22 | — | 1.02 |

- The full-trip baseline (`cur`, t′ 0–1) reproduces the diagnosis on this machine: crossings at t′ 0.094, 0.25, 0.40, 0.56, 0.71 and 0.86, worst 13.6 at 0.859.
- The truth bake's own steps at t′ 0.016–0.035 (10.8 under every form, absent from head) are a truth artifact. Head is smoother than truth there.

**Wall trailing end face** (`walledge`, t′ 0.10–0.45, five crossings, 2 m/s):

| Form | Step max | n > 3 | vs truth | Look change |
|---|---|---|---|---|
| cur | 19.9 | 18 | 0.44 / 11.0 | — |
| r10 | 22.4 | 21 | 0.56 / 10.0 | 1.00 |
| r25 | 13.3 | 13 | 0.43 / 5.5 | 1.09 |
| **r40** | **8.7** | 12 | **0.35 / 4.1** | 1.81 |
| o40 | 18.9 | 11 | — | 2.89 |

- The face still brightens by about 70 luma over the trip, which is the real opening of the doorway. Under `r40` each lattice step arrives as a ramp over about 6 frames, not 1–2 (`a2-ruling-a-steps.png`).
- Under `o40` the largest step moves from t′ 0.158 to 0.18: the offset only moves the flip.

**Wall north face beside the edge** (same window): `cur` 7.4 with 11 frames over 3. Every other form reads 6.2–6.5 with 4 frames over 3.
- The remaining steps at t′ 0.380 and 0.448 appear under every form and in the truth, so they are not the hand-over. They are not attributed.
- The lattice-crossing steps at 0.223, 0.296, 0.368 and 0.440 are gone under every ramp form.

**Room ceiling** (`door_ceiling`, t′ 0.10–0.60, two z-lattice cycles, 1 m/s):

| Form | A above the door: step max (n > 3) | A vs truth | B west wall: step max (n > 3) | B vs truth | B range |
|---|---|---|---|---|---|
| cur | 5.5 (7) | 0.23 / 4.8 | 4.7 (12) | 0.22 / 8.4 | 12.8 |
| r10 | 2.9 (0) | 0.18 / 4.5 | 8.5 (5) | 0.15 / 10.3 | 6.6 |
| r25 | 3.6 (1) | 0.16 / 5.7 | 2.1 (0) | 0.07 / 1.0 | 5.8 |
| **r40** | 4.9 (1) | 0.17 / 4.9 | **1.2 (0)** | **0.07 / 1.0** | **5.7** |
| o40 | 7.1 (4) | — | 2.8 (0) | — | 10.3 |

- `r40`'s one remaining step over 3 above the door, at t′ 0.525, is in the truth too (5.1).
- It falls as the lintel's bottom face (z −48 + 176t′) passes the z 44.22 layer: an enclosure sub-step in the in-wall column, not a lookup hand-over. It grows with the offset: 2.9 at 0.1, 3.6 at 0.25, 4.9 at 0.4.

**Full room trip** (t′ 0–1 at 0.01, `cur` against `r40`, head against truth): `r40` tracks truth better, at 0.04 against 0.23 mean on segment B and 0.15 against 0.24 on A.

### Rest-frame changes (t′ 0 and 1, head bake, form − `cur`)

| View, t′ | r10 | r25 | r40 | o40 |
|---|---|---|---|---|
| Sweeper face (`sweepfix`), 0 | −0.10 mean, worst block 3.5 | −0.14, 3.4 | −0.16, 3.4 | −0.01, 1.3 |
| Sweeper face, 1 | −0.07, 2.6 | +0.10, 3.3 | **+0.83, 7.3** | **+1.86, 15.7** |
| Sweeper owner view (`sweeper_east`), 1 | 40 % px change, worst 3.0 | 51 %, 3.3 | 66 %, 7.4 | 73 %, 16.0 |
| Wall owner view (`wall_owner`), 0 (closed) | worst block 9.3, mean +0.06 | 9.3, +0.10 | 10.5, +0.20 | 10.5, +0.20 |
| Wall, 1 (in its pocket) | 0 | 0 | 0 | 0 |
| Wall end face, 1 | 0.49 \|d\|, worst 2.8 | 0.49, 2.7 | 0.49, 2.6 | 0.09, 0.7 |
| Room ceiling band B, 0 (lowered) | **4.75 → 0.76** | → 0.22 | **→ 0.09** | → 0.88 |
| Room ceiling band B, 1 (raised) | −0.82 | −0.90 | −0.92 (worst 4.0) | +0.14 |
| Room ceiling band A, 0 | 3.40 → 0.50 | → 0.11 | → 0.01 | → 0.52 |
| Room floor (12c crop), 0 / 1 | excess 2.2 → 1.9 / 6.7 → 6.5 | → 1.9 / 5.5 | → 1.8 / 5.6 | → 2.0 / 8.8 |

- Every ramp form removes the lowered room's resting ceiling-band leak (band B 4.75 → 0.09 at `r40`). Elsewhere, rest moves by under 1 level on average, with single blocks up to 7–10.
- `o40` is the form that moves rest the most (the sweeper at t′ 1, worst block 16).
- AC 1 compares per-probe channel contributions, so it is untouched: these forms change no baked byte. Frame goldens and C10's frame case would need new baselines.

### Eighth-power redo (task 12c's floor metric, head bake)

Each cell is the max excess over the local median / the count of local peaks more than 3 levels above it. Sheet: `a2-sheet-floor-excess.png`.

| t′ | cur | r10 | r25 | r40 | o40 | id 57 stripped |
|---|---|---|---|---|---|---|
| 0.50 | 8.7 / 8 | 6.5 / 6 | 6.5 / 6 | 6.3 / 6 | 6.1 / 3 | 3.8 / 1 |
| 0.70 | 9.9 / 9 | 5.5 / 2 | 5.5 / 2 | 5.4 / 2 | 5.8 / 6 | — |
| 0.80 | 7.3 / 3 | 5.3 / 5 | 5.9 / 5 | 6.0 / 4 | 9.4 / 9 | 2.5 / 0 |
| 0.86 | 9.4 / 9 | 7.6 / 6 | 7.5 / 7 | 7.5 / 6 | 9.8 / 10 | 3.1 / 0 |
| 0.90 | 10.3 / 8 | 8.1 / 5 | 8.1 / 4 | 8.1 / 4 | 10.0 / 10 | 3.7 / 0 |
| 0.95 | 9.2 / 8 | 6.9 / 6 | 6.9 / 6 | 6.5 / 6 | 6.8 / 5 | 3.3 / 0 |
| 1.00 | 6.7 / 5 | 6.5 / 6 | 5.5 / 8 | 5.6 / 7 | 8.8 / 12 | 2.2 / 0 |

- Captured at 17 t′ values, 0–1. Every ramp form lowers the max excess at every mid-travel t′, by 0.8–4.5 levels against `cur`. Peak counts stay at 2–9.
- No lattice of dots appears in any excess map. The ramp floors every probe behind the floor at 0.03 whatever its lateral offset, so mechanism B (dark probes beside the foot of a probe) gets no lateral variation to draw.
- The remaining excess is the large-scale wedge and the far-edge band 12c already attributed (present at t′ 1 and under the hard form). `o40`, with today's cosine, is the only form that gets worse (t′ 0.80: 9.4 / 9).
- On the Mac, 12c measured k = 8 at 0.80 as 6.5 / 3 and at 0.90 as 8.9 / 3. This machine's `cur` reads 7.3 / 3 and 10.3 / 8: the same magnitude, a different adapter.

## Ruling B

### The in-wall / above-roof split, measured first

- **Method.** Capture with light-term mask 0x02 (static indirect, which is the whole band: the empty-box check reads exactly 0 in both segments). For each probe class, re-render keeping only that class's share of the mover indirect numerator; the weights and their sum are untouched, so the shares add. Shares are taken in linear light, by sRGB decode of each pixel.
- **Classes,** per t′, by the probe's map position:
  - `wenc`: the x 846.93 column inside the room's solid, from the floor-slab bottom to the roof top;
  - `wabove`: that column above the roof top;
  - `xabove`: probes over the room's interior x (850–1160) above the roof top.
- Sampled at t′ 0.10–0.94 every 0.02. Full table: `split.txt`. Chart: `a2-ruling-b-band-split.png`.

| Share of band light (mean over t′; max at a pulse peak) | Segment B, over the wall | Segment A, over the door |
|---|---|---|
| In-wall enclosed (`wenc`) | **0.77** (0.62 at the peaks, up to 0.99) | 0.50 (0.23–0.95) |
| In-wall above the roof (`wabove`) | 0.05 (max 0.19) | 0.04 (max 0.13) |
| Interior above the roof (`xabove`) | 0.07 (max 0.30) | 0.07 (max 0.44) |
| Above the roof, both | **0.12 (0.38 at t′ 0.24)** | 0.11 (0.55 at t′ 0.24) |
| Everything else | ~0.11 | ~0.39 |

- The above-roof probes make the **rise** of each pulse. Their share grows from 0 when the roof top is 7–20 u below a z layer to its peak just before the roof encloses the layer.
- The in-wall enclosed column carries the **level**.
- The drop at each roof crossing (t′ 0.251, 0.475, 0.698, 0.922) is the above-roof share vanishing, as the inside knot stores the dark tile and the values blend across.
- Under `o40` the above-roof share falls to 0.011 on average (peak 0.10), while the in-wall enclosed share rises to 0.83. That matches the consensus's arithmetic, 0.49 → 0.19 trilinear.
- Under `r40` the above-roof numerator is below 8-bit resolution (reads 0).

### Band per option (segment B; mask 0x02 luma; mean for t′ 0.10–0.56 / 0.56–0.94; max; largest per-frame step)

| Option | Mean | Max | Step max | Notes |
|---|---|---|---|---|
| Today (`cur`) | 10.8 / 10.9 | 15.8 | 4.7 | Pulses 3 → 15.8 each cycle |
| (iii) larger offset alone (`o40`) | 8.1 / 9.0 | 12.0 | 2.8 | The pulse moves later and shrinks; it is not removed |
| Ruling A's `r40` | 5.3 / 6.6 | 6.8 | 1.2 | No pulse; a steady edge line remains |
| (i) lattice shifted 32 u (`shift`, today's form) | **0.9 / 2.5** | 3.5 | 2.0 | Segment A over the door still 2.0 / 7.6, max 11.8 |
| (i) shift + `r40` | 0.6 / 2.3 | 3.6 | 1.6 | |
| (ii) dark value state, estimate, today's form | 2.1 / 4.0 | 7.6 | — | The peaks remain: they are the above-roof share |
| (ii) dark value state, estimate, under `r40` | **0.5 / 2.5** | 4.3 | — | |

- **(ii) is an estimate, not a prototype.** It drops the `wenc` numerator and keeps its weight, which is what a black stored value would give where both neighbouring knots are enclosed mid poses.
- It is therefore an upper bound on the gain. Near a terminus the enclosed terminus still reads the base, and the value blends from it, so the band at t′ 0 and 1 is unchanged by (ii).
- A bake prototype would mean editing `mover_light_gate/`, which is the concurrent worker's area, so I did not build one. It also could not show the dark-sphere risk without new captures of the sweeper's faces near their termini (the risk recorded at 2026-11-10).

### (i) The authoring warning: how many layers, and can a phase key clear them?

- **Rule tested** (`tools/warnscan.py`).
  - A lattice layer along an axis the mover never travels on, lying strictly inside one of its brushes, or within k cells in front of a face of that brush.
  - Only brushes thinner than one cell along that axis count. A thicker brush always contains a layer, so a warning there could never be cleared.
  - The layer needs at least one valid probe inside the brush's swept footprint.
  - Faces that move along their own normal cross every layer and are ruling A's matter, so they are not warned.
- **Scope.** All 24 shipped `content/dev` maps that have a mover and a PRL. The two `mover-closet-*-kinematic-receiver` maps have no PRL. The scan reads AABBs and waypoint paths, so it treats rotating movers as translating: an approximation.

| k (lookup offset) | Maps warning | Layers | Inside a slab | Near a face |
|---|---|---|---|---|
| 0.1 (today) | 10 | 32 | 13 | 19 |
| 0.25 | 14 | 46 | 13 | 33 |
| 0.4 | 15 | 50 | 13 | 37 |

- **At k = 0.1:**
  - `mover-pins` 14;
  - `ghost-links` and `ghost-links-luau` 5 each;
  - `mover-setpiece` 2: room wall x 846.93, door slab y 1048.82;
  - `campaign-test` 1;
  - `mover-closet` 1;
  - the three `teleporters*` maps 1 each;
  - `test_mover_light_gate` 1.

  `enemy-movers` has none: the sweeper's thin axis is its travel axis.
- **Shifted setpiece.** The room's wall layer clears at k = 0.1. At k = 0.25 or more, the shifted layer 854.3 sits 6.3 u in front of the wall's inner face and warns as near-face. So a larger offset (ruling A) widens what the warning must cover.
- **A phase key cannot always clear the warnings.** For each axis I swept every lattice phase and asked whether some phase clears every thin slab:
  - `mover-setpiece` y: **no phase clears it at any k, k = 0 included.** The 32 u door slab plus the room's 16 u south and north walls leave none.
  - `mover-setpiece` x: 31 % of phases clear it at k = 0, 12 % at 0.1, none at 0.25 or more.
  - `ghost-links` x: no phase clears it at any k.
  - `campaign-test` x: 31 %, 11 %, then none.
  - `mover-pins`: x 0.67 / 0.47 / 0.17 / 0; y 0.79 / 0.59 / 0.29 / 0; z 0.30 / 0.10 / 0 / 0.

  So a phase key is an author remedy that works per map and per axis when the slabs agree. It cannot be a compiler guarantee, and with a 0.4-cell lookup offset it never satisfies a near-face rule.

## IP check per candidate

Rules: `index.md` "IP rules bind unchanged"; `ip-372` levers 2, 3, 5 and 7. Every valid probe must keep a continuous weight of at least 0.03, and nothing may flag, zero or exclude a probe.

| Candidate | Continuous, ≥ 0.03, never zero? | Flags, zeroes or excludes a probe? | Stores visibility? | Lever-6 non-goal ("runtime geometric or half-space test against the posed mover")? |
|---|---|---|---|---|
| r10 / r25 / r40 (surface-point ramp) | Yes. smoothstep(s) + 0.03 is continuous in the probe's position and in t′; the per-probe weight is trilinear × validity × (ramp + 0.03) × Chebyshev (≥ 0.03) | No | No; runtime arithmetic on the receiver only | **Flag for the owner.** It uses only the receiving fragment's own position and geometric normal, the same inputs as today's cosine. It is no test of the probe against the mover's brushes. But s is the probe's signed distance from the receiver's plane: exactly the half-space quantity counsel question 6 asks about (`max(dot(probe − point, n), 0)`), made smooth and floored as lever 5 allows. On movers the receiver *is* the posed mover face. If the owner reads lever 6 as covering the receiver's own plane, today's cosine is covered too, and the ramp is closer in form to the half-space than the cosine is. Lever 5: the owner's call |
| o40 (offset only) | Yes, unchanged form | No | No | No new surface; the offset size is lever-5-adjacent |
| (i) warning + phase key | Unchanged | No; compile-time placement of static probes. It must never turn into invalidating in-slab probes, which would zero a weight | No | No |
| (ii) dark value state, enclosed mid poses | Unchanged; weights untouched, floor kept | No; stores absolute value states blended by mover position (N states, ip-check §Piece 7) | No | No. It reverses the owner's 2026-11-10 enclosing rule and changes the counsel-packet line "an enclosed or skipped mid pose blends across" |
| `attr` numerator filter (measurement tool, **not a candidate**) | n/a | **Yes: it drops classes of probes from the numerator.** Shipped, it would break levers 2 and 7 | — | Diagnostic only; remove before landing |

## Recommendation

- **Ruling A: the surface-point ramp, floored at 0.03, with a 0.4-cell lookup offset (`r40`).**
  - It is the largest cut on every hand-over surface: sweeper 11.2 → 4.3, wall end 19.9 → 8.7, ceiling band 4.7 → 1.2, and wall north 7.4 → 6.5 (crossing steps gone).
  - It tracks the dense truth as well or better: wall end 0.35 against 0.44, ceiling B 0.07 against 0.22.
  - It improves the eighth-power floor metric, with no dot lattice.
  - Its rest changes are small: under 1 level on average, worst blocks 7–10. The lowered room's resting ceiling leak goes, 4.75 → 0.09.
  - **Costs to state.**
    - The ceiling's lintel enclosure sub-step grows to 4.9 at t′ 0.525 (in the truth too).
    - The Chebyshev test point moves 15.7 u off the face, a thin-static-geometry risk Opus flagged. It isn't measured here: no shipped mover face has static geometry that close in these views.
    - Frame goldens and C10's frame case need new baselines.
    - M7 (Mac) should re-measure frame cost: the shader adds a few ALU per corner, unmeasured.
  - **`r25`** is the fallback if the owner wants a smaller offset: sweeper 7.4, wall end 13.3, ceiling 3.6 / 2.1.
  - **Don't adopt `r10`.** It is worse than today on the fast movers.
- **Ruling B: build (i), the warning, and offer the phase key; hold (ii); (iii) is subsumed by A.**
  - With `r40` the band no longer pulses: 1.2 max step, 5.7 range. It is no longer an M9 "stepping" item; what remains is a steady 5–7-luma edge line over the in-wall column (77 % of it).
  - The warning is cheap and IP-clean, and it names real cases on 10 shipped maps. Its message should give the measured remedy: move the slab, or shift the lattice phase where one exists. Shifting the lattice removes this map's west band (0.6–0.9 mean).
  - The phase key cannot be automatic or guaranteed (the setpiece's y axis and `ghost-links` x have no clean phase), so it belongs as an author key, not a compiler pass.
  - (ii) shows the largest estimated gain where no phase works: 0.5 mean under `r40`. But it is a ruling reversal with a recorded dark-sphere risk, and only a bake-side prototype with sweeper-face captures can price that. Prototype it in a follow-up only if the owner wants the steady line gone on maps the phase key can't fix.

## Files

- **Here:**
  - `results.md`;
  - `analyze.txt` (ruling A tables);
  - `split.txt` (ruling B split, per t′);
  - `warnscan.txt` (warning scan, all maps);
  - `tools/` (Windows harness);
  - the PNGs:
    - `a2-ruling-a-steps.png`;
    - `a2-ruling-a-sweeper-series.png`;
    - `a2-sheet-sweeper-crossing.png`;
    - `a2-ruling-b-band-split.png`;
    - `a2-sheet-ceiling.png`;
    - `a2-sheet-floor-excess.png`.
- **Prototype code** (marked `AMEND2-PROTO`, LF, uncommitted; remove or replace before landing):
  - `crates/renderer/src/shaders/sh_sample.wgsl`: the prototype constants, the private surface-to-lookup vector, the ramp branch in `sh_backface_weight`, and the numerator filter in `sample_sh_indirect_corners_pair`;
  - `crates/renderer/src/shaders/kinematic_brush.wgsl`: the lookup offset from the constant, and the surface-to-lookup vector set before each read;
  - `crates/renderer/src/render/kinematic_brush.rs`: `amend2_proto_source`, applied in `shader_source`.

  No test was run against these edits. The existing source-text tests look for the eighth-power lines and the floor constant, both still present, but `cargo test` was not run.

## Ruling B option (ii), prototyped (Windows, 2026-11-12)

Prototype and measurement only: nothing is committed or made the default. Same machine and settings as above (GTX 1660 SUPER, Vulkan, offscreen 960×540, `sync-proof`, one forced mover).

### Answers first

- **(ii) removes the in-wall band, and the dense truth agrees.** Under `r40`, segment B (static indirect) falls from 5.3 / 6.6 to **0.77 / 2.34** mean (t′ 0.10–0.56 / 0.56–0.94), max 4.1. That matches the lattice-shift reference (0.63 / 2.33) and the earlier estimate (0.5 / 2.5). Against a dense truth baked under the same dark rule, the knotted build is within 0.02 mean, 1.0 worst.
- **Without ruling A's ramp it paints dark spheres.** Under today's backface, the sweeper's west face gets a row of lattice-spaced dark blotches, the worst block −16.9 at t′ 0.078. The east face reaches −7.8 and the wall's end face −9.2. The dense dark-rule truth shows them too, so the dark rule causes them, not interpolation. Under `r40` the worst darkening on any mover face is 2.2, and no frame has a block darker by more than 3.
- **Static receivers are untouched.** The arena floor and walls in the fixed sweeper views, and the room-floor eighth-power metric, are unchanged.
- **Rest is unchanged.** Every terminus state is byte-identical, and all 28 rest frames match today's bake pixel for pixel.
- **New residue: the band re-lights into rest.** Termini still read the base, so the band climbs from 4.4 to 6.3 over the last 0.04 of travel. Above the door, the largest step at t′ 0.98 is 8.7, against 4.65 for `r40` alone. The dense dark truth steps 4.6 there, so a stored pose closer to the terminus would halve it.
- **Cost.** id 57 grows 1.8 % on `mover-setpiece` and 3.5 % on `enemy-movers`. Bake time does not change.

### What "dark" means

The switch removes the enclosed-mid-pose exception. An enclosed mid pose stores the state its own posed bake gives, like any other mid pose. Enclosure no longer decides anything at a mid pose. Termini keep today's rule.

| Term | Stored at an enclosed mid pose | Why |
|---|---|---|
| Bounce | The posed gather's own tile. Every lifted ray starts in solid and reads black, so it is zero radiance. It is stored where it leaves the probe's bars against id 34, and reads the base where id 34 is already within them | Measured: the largest \|SH coefficient\| over every enclosed (probe, pose) is exactly 0. That covers 713 pairs on `enemy-movers`, 8,672 on `mover-setpiece` and 2,233 in the dense sweeper. So "what the bake gathers" and "an explicit zero-radiance SH" are the same definition. On the setpiece, 8,017 pairs store dark and 655 read a base already within the bars of zero |
| Direct | The traced q. Every lifted segment starts in solid, so q is 0 wherever the light is statically clear | The same joint trace every pose runs; no special value |
| Depth | Unchanged: the dark tile | The inside-brush rule, as today |
| Across marks | None written | Nothing blends across any more (19 → 0 entries on `enemy-movers`, 31 → 0 on the setpiece; 13,744 B → 4 B) |

- **Switch.** `POSTRETRO_PROTO_ENCLOSED_DARK=1`, read once by `prl-build` and logged as `[AMEND2-PROTO-B]`. Unset, the build is byte-identical to HEAD on both maps. It is not folded into any memo key, so bake with `--release` only.
- **No wire, loader or renderer change.** The states are ordinary stored states, and the loader accepts the sections. The renderer has not changed; the `AMEND2-PROTO` backface switch is ruling A's.
- **A bounce-only variant was not built.** That would mean bounce dark and direct blending across. It conflicts with a loader refusal: `MoverProbeChannelAcrossBounceState` refuses an across-marked probe whose bounce entry stores a state. It would need a wire change, or a baked interpolated q.

### Numbers

Columns are today's bake and the dark bake, each under today's backface (`cur`) and `r40`. The truths are dense: room `light_poses 76`, wall 267, sweeper 113. "today's rule" is the earlier truth bake. "dark rule" is the same truth maps baked with the switch. Full output: `dark-analyze.txt`.

**Segment B, static indirect (mask 0x02), t′ 0.10–0.94**

| Column | Mean, 0.10–0.56 / 0.56–0.94 | Max | Largest mean step |
|---|---|---|---|
| Today | 10.84 / 10.94 | 15.8 | 4.39 |
| `r40` alone | 5.27 / 6.55 | 6.8 | 2.05 |
| Dark, today's backface | 2.19 / 3.69 | 7.4 | 6.88 (the above-roof pulse remains) |
| **Dark + `r40`** | **0.77 / 2.34** | **4.1** | **0.42** |
| Dense truth, dark rule, `r40` | 0.75 / 2.30 | 4.1 | 0.39 |
| Lattice shifted 32 u + `r40` (option i) | 0.63 / 2.33 | 3.6 | 0.37 |

**Per-frame steps (largest 8×8-block |Δluma| between 60 fps frames; frames over 3 in brackets)**

| View | Today | `r40` | Dark, today's backface | Dark + `r40` | Dark + `r40` vs dark-rule truth (mean / worst) |
|---|---|---|---|---|---|
| Ceiling B, t′ 0.1–0.6 | 4.71 (12) | 1.19 (0) | 5.91 (8) | **1.00 (0)** | 0.02 / 1.0 |
| Ceiling A, t′ 0.1–0.6 | 5.52 (7) | 4.90 (1) | 5.07 (8) | 4.29 (1) | 0.11 / 2.6 |
| Ceiling B, whole trip (0.01 steps) | 9.63 | 3.02 | 10.83 | 1.74 (at t′ 1.0, the re-light) | 0.03 / 1.3 |
| Ceiling A, whole trip (0.01 steps) | 7.39 | 4.65 | 9.14 | **8.70 (at t′ 0.98, the re-light)** | 0.15 / 5.5 |
| Sweeper east face | 11.23 (23) | 4.34 (12) | 11.23 (23) | 4.34 (12) | 0.67 / 9.2 (the truth's own t′ 0.027 artifact; `r40` alone reads the same) |
| Sweeper west face | 13.43 (20) | 6.95 (25) | **20.35 (21)** | 7.38 (26) | 0.10 / 4.8 |
| Wall end face | 19.89 (18) | 8.73 (12) | 19.87 (22) | 8.72 (13) | 0.37 / 4.1 |
| Wall north face | 7.37 (11) | 6.53 (4) | 7.39 (10) | 6.53 (4) | 0.11 / 9.6 (`r40` alone reads the same) |

**Dark spheres.** Worst 8×8-block darkening of the dark bake against today's bake under the same backface, with its t′ and the number of frames with any block darker by more than 3. Charts: `a2-ruling-b-dark-blotch.png` and `a2-ruling-b-dark-sheet.png`, each view at its worst t′ with an ×8 difference map.

| View | Today's backface | `r40` |
|---|---|---|
| Sweeper east face (rides the slab) | −7.8 at 0.414 (3 frames) | −1.0 (0) |
| Sweeper west face (rides the slab) | **−16.9 at 0.078** (5 frames) | −2.2 at 0.231 (0) |
| Sweeper, fixed camera east (face, floor, walls) | −6.7 at 0.26 (4) | −1.9 (0) |
| Sweeper, fixed camera west | −13.8 at 0.54 (6) | −2.4 at 0.54 (0) |
| Wall end + north faces | −9.2 at 0.440 (a vertical stripe) | −1.6 (0) |
| Room floor view | −4.2 at 0.80 (1) | −2.2 (0) |
| Ceiling band B / A | −34.2 / −19.9 | −15.2 / −8.2: the band going away, as intended |

- The blotches sit on the mover's own faces, one per probe column behind the face. Floors and static walls show no difference.
- Under `r40` the west face still dips about 0.8 mean luma for a few frames at each lattice crossing (t′ 0.08, 0.23, 0.38; `a2-ruling-b-dark-sweeper.png`). The dense dark-rule truth dips the same way.
- **Floor, eighth power** (task 12c, `dark-floor-timing.txt`): max excess identical at every t′ under both forms. Peak counts move by at most ±2.

### Rest, bytes and time

- **Termini unchanged.** A temporary `#[cfg(test)]` check decoded both builds' id 57 and compared every terminus state. It passed for today against dark on both maps, and for the today-rule truths against the dark-rule truths:
  - direct q at both termini: 3,950 (cell, light, mover, probe) on `enemy-movers`, 14,560 on the setpiece;
  - bounce: terminus masks and tiles of every entry (202 and 412);
  - depth: terminus marks and tiles of every record.

  The only mid-pose q changes are 255 → 0: 4,121 on `enemy-movers` and 33,208 on the setpiece. The check has been removed.
- **Rest frames.** At t′ 0 and 1, every view under both forms (28 frames) is pixel-identical between today's bake and the dark bake.

| | `enemy-movers` | `mover-setpiece` |
|---|---|---|
| id 57 | 4,515,849 → 4,673,165 B (+3.5 %) | 12,030,273 → 12,247,988 B (+1.8 %) |
| Direct payload | 657,097 → 697,929 B; records 201 → 208 | 3,134,893 → 3,259,212 B |
| Bounce payload | 1,930,608 → 2,051,136 B | 4,175,496 → 4,282,632 B |
| Bounce states kept after skipping | sweeper 1,382 → 1,627; lift 184 → 284; crushers 335 → 415, 423 → 556 | wall 1,781 → 1,968; room 1,376 → 1,685 |
| Depth | unchanged | unchanged |
| Cold `--release` bake, paired runs | 14.45 / 14.35 s → 14.29 / 14.33 s | 51.11 / 51.16 s → 51.23 / 50.65 s |

Both payloads stay resident, under their 8 MiB bars. A run of dark states skips cleanly, since a dark state between dark neighbours is within the bars. Only the transitions add states.

### Wall truth 267, by the exact predicate

A temporary `#[cfg(test)]` check placed `light_poses 267` on `setpiece_wall` and called `mover_light_gate::poses_in_band` on its even knots. It found **0** mid poses in a band, and the placed knots equal the even knots bit for bit, so task 23's nudge moved none. The same check gives 0 for room 76 and sweeper 113. The check has been removed.

### IP reading

- **The shape is unchanged.** Each pose stores an absolute value state, and compose blends neighbouring states by mover position (`ip-check.md` §Piece 7, §Tencent). The renderer and every weight are untouched. Each valid probe keeps trilinear × validity × backface × Chebyshev, floored at 0.03, and depth still blends moment sets.
- **Not a flag.** q is the traced surviving fraction, stored as at every pose; q = 0 already occurs wherever a pose fully shadows a probe. The bounce tile is the gathered irradiance. Neither is precomputed from a threshold (lever 2). At a mid pose the prototype removes the one place enclosure chose behaviour; enclosure now decides storage only at the termini.
- **Could it be read as "a per-probe occluded flag" or "a weight forced to zero when a door closes" (lever 7)?** The weight is not touched. The probe's *value* goes to zero because its rays start inside the solid.
  - An excluded probe would drop out of the normaliser, and its neighbours' light would fill in. A dark-valued probe stays in at ≥ 0.03 and pulls the result darker, so it is the opposite of exclusion.
  - The risk to name: a reader may see "the probe goes black when the slab passes over it" as a door-state verdict. That is an argument to make, not a mechanism the code has.
- **Counsel-packet line** would become: "states per pose, not per terminus; an enclosed terminus reads the base; an enclosed mid pose stores the state its own posed bake gives (zero bounce, its traced direct q, the dark depth tile); a skipped mid pose blends across."
  - The across-marks sentence goes, since no across marks remain.
  - Question 9's wording is unaffected.
  - Question 7 gains a note: probes at the 0.03 floor now carry dark values mid-travel.

### Recommendation

- **Adopt (ii) only together with ruling A's `r40`, never alone.**
  - With `r40` it removes the in-wall level the warning could only ask authors to fix. Band B goes to 0.77 / 2.34, matching both the lattice-shift reference and its own dense truth.
  - It needs no author action on maps where no lattice phase clears (the setpiece's y axis, `ghost-links` x).
  - It costs 2–4 % of id 57 and no bake time, and leaves rest bytes and frames exactly as they are.
- **With today's backface it is worse than today.** Dark spheres reach −17 and the sweeper west-face step grows 13.4 → 20.4.
- **Owner calls:**
  - the reversal of the 2026-11-10 enclosing rule;
  - the counsel-packet line above;
  - the re-light into rest: a step of 8.7 above the door at t′ 0.98, against 4.65 for `r40` alone. A follow-up could add a geometry knot just inside the terminus where an enclosure interval reaches it. The dense truth's 4.6 there suggests that halves the step.
- Keep (i)'s warning only if the owner wants authors told about in-slab layers anyway. With (ii) and `r40`, it no longer has to carry the fix.

### Files (option ii)

- **Here:**
  - PNGs: `a2-ruling-b-dark-band.png`, `a2-ruling-b-dark-sweeper.png`, `a2-ruling-b-dark-blotch.png`, `a2-ruling-b-dark-sheet.png`;
  - clips: `clips/a2-ruling-b-dark-ceiling-band.mp4`, `clips/a2-ruling-b-dark-sweeper-east.mp4`, `clips/a2-ruling-b-dark-sweeper-west.mp4`. Each is H.264, 2 × 3 panels: today | `r40` alone | dark + `r40`; dense truth (today's rule) + `r40` | dense truth (dark rule) + `r40` | dark + today's backface;
  - tables: `dark-analyze.txt`, `dark-floor-timing.txt`;
  - tools: `tools/dcap.py`, `danalyze.py`, `dcharts.py`, `dclips.py`, `dregions.json`, `dbake.sh`, `dbakes.sh`, `dcaps.sh`. They extend `acap.py` with the sweeper west-face and fixed views, and take the scratch root from `A2_ROOT`.
- **Prototype code** (marked `AMEND2-PROTO-B`, LF, uncommitted; remove before landing): `crates/level-compiler/src/mover_probe_channel/`:
  - `mid_pose.rs`: the switch, and enclosed probes classifying their own gathered state;
  - `direct.rs`: enclosed mid poses traced;
  - `across.rs`: no across marks under the switch.
