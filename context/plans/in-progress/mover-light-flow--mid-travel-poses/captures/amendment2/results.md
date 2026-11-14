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

## Final build (task 25c, Windows, 2026-11-12)

Verification of the ruled build at f0bd37f9b: the ramp at 0.4 is the only mover backface, and the dark state with edge copy is the only compiler rule (id 57 epoch 7). Same machine and capture settings as above (GTX 1660 SUPER, Vulkan, offscreen 960×540, `sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover).

### Answers first

- **Edge copy passes its gate; no bounding pose is needed.** Over the whole trip at 60 fps frame spacing, the west band B (mask 02) reads a mean of 1.40 against 1.53 for the prototype dark + r40 without edge copy. Its largest per-frame block step is 0.93 against 1.00. It is never brighter than the prototype by more than +0.05 mean (at t′ 0.265), and its worst block differs from the prototype by 1.0. Against its own dense truth, it reads a block mean of 0.09 and a worst block of 1.3.
- **Edge copy removes the exit dip.** At the t′ 0.9716 lintel exit, the prototype's above-door band falls 3.51 below its inside-pose value, with a 6.12 block step over one frame span. The final build rises monotonically (dip 0.00) with a 3.91 step, against 4.33 for the ramp alone. The 0.524 and 0.748 exits behave the same way. No entry undershoots more than the ramp alone does, beyond one 8-bit level.
- **Hand-over is unchanged from the prototype ramp.** Sweeper east 4.34, wall end 8.77, wall north 6.53, ceiling A 3.80 and ceiling B 0.86 per frame, against 11.2, 19.9, 7.4, 5.5 and 4.7 pre-amendment.
- **No dark blotches.** Against the ramp-alone build, no mover face darkens a block by more than 0.34. On the sweeper, edge copy leaves no dark state visible at all: each thin-slab enclosure run is two knots, and both carry clear values. Against the dense truth, the worst darkening (−13.1 on the fixed east view at t′ 0.98, −9.6 on the wall's north face at 0.317) is identical in the ramp-alone and pre-amendment builds, so it is a knot-interpolation miss, not a dark state.
- **Re-light into rest:** 0.93 per frame at t′ 0.985, under the 1.5 bar.
- **Rest frames** are pixel-identical to the ramp-alone captures in all 24 view × terminus frames, so every rest change against pre-amendment is the ramp's (table below).
- **Every gate passes:** K1, K2, K3, D2, C7, C10 (frame, BC6H per-probe, uncompressed per-probe) and AC 1.

### Build and bakes

- `cargo check --workspace --all-targets`: clean, with no warnings and no fixes needed.
- Release `prl-build` and `postretro --features capture` at f0bd37f9b, kept in scratch `amend2\final\bin\`.
- Cold `--release` bakes, each with a fresh cache dir. The scratch ws needs junctions to the repo's `content/dev/{textures,models,sounds,ui,scripts}` and `baked/`. Without them, the truths bake without texture albedo and the engine renders placeholder checkers. The earlier ws had lost those junctions, and they were restored.

| Bake | Wall time | Poses (even + geometry) | id 57 |
|---|---|---|---|
| `enemy-movers` HEAD | 16.5 s | lift 11 (3+8), sweeper 31 (7+24), crusher_open 26 (6+20), crusher_wall 39 (9+30) | 4,551,545 B |
| `mover-setpiece` HEAD | 54.5 s | wall 71 (15+56), room 45 (5+40) | 12,243,232 B |
| `enemy-movers` truth, sweeper `light_poses 113` | 23.9 s | sweeper 113 (113+0) | 5,864,676 B |
| `mover-setpiece` truth, room 76, wall 267 | 115.1 s | wall 267, room 76 | 16,593,246 B |

- No band nudges were logged on any bake (the `held the probe … within the band` line is absent). The knots equal the prototype's bit for bit on both maps.
- id 57 against the prototype dark bake (no edge copy) and the pre-dark HEAD:
  - setpiece: −4,756 B (−0.04 %), and +1.8 % over pre-dark. Direct payload is 3,259,212 B, the same as the prototype. Bounce is 4,277,880 B, against 4,282,632 for the prototype and 4,175,496 pre-dark.
  - enemy-movers: −121,620 B (−2.6 %), and +0.8 % over pre-dark. Direct payload is 657,097 B, the pre-dark size, where the prototype was 697,929 B: edge copy restores every sweeper and crusher enclosure's q. Bounce is 1,970,568 B, against 2,051,136 for the prototype and 1,930,608 pre-dark.
- Bounce states kept after skipping: wall 1,984 (prototype 1,968), room 1,647 (1,685), sweeper 1,447 (1,627), lift 208 (284), crusher_open 371 (415), crusher_wall 483 (556).
- Bake time is unchanged within noise. The prototype's paired runs were 14.3 s and 51 s, and these runs had captures beside them.

### a. Hand-over (largest 8×8-block step per 60 fps frame; vs truth = mean / worst block |HEAD − truth|)

| Surface | Final | Final − truth step, worst | Final vs new-rule truth | Prototype dark + r40 | Ramp alone | Pre-amendment |
|---|---|---|---|---|---|---|
| Sweeper east (t′ 0–0.5) | 4.34 (13 > 3) | +1.92 at 0.043 | 0.66 / 9.2 (the truth's own t′ 0.027 artifact) | 4.34 | 4.34 | 11.23 |
| Wall end (0.10–0.45) | 8.77 (12) | +2.00 at 0.151 | 0.35 / 4.1 | 8.72 | 8.73 | 19.89 |
| Wall north | 6.53 (4) | +3.16 at 0.450 | 0.11 / 9.6 | 6.53 | 6.53 | 7.37 |
| Ceiling A (0.10–0.60) | 3.80 (1) | +0.72 at 0.302 | 0.25 / 3.0 | 4.29 | 4.90 | 5.52 |
| Ceiling B | 0.86 (0) | +0.49 at 0.428 | 0.07 / 1.0 | 1.00 | 1.19 | 4.71 |

- The truth's own steps are larger than the final build's on every surface: 10.8, 10.4, 10.3, 3.2 and 0.9.
- Pre-amendment, the step over its truth reached +10.57 on the wall end, +5.81 on the north face, +3.95 on B and +2.65 on A.

### b. In-wall band and the edge-copy gate (whole trip, one sample per 60 fps frame, 269 frames)

| Band B, mask 02 | Mean (0.10–0.56 / 0.56–0.94) | Max | Step max | vs new-rule truth (block mean / worst) | Mean − prototype (max / min) |
|---|---|---|---|---|---|
| **Final** | 1.40 (0.67 / 2.09) | 6.30 (rest, t′ 1) | 0.93 at 0.985 | 0.09 / 1.3 | +0.05 at 0.265 / −0.56 at 0.228 |
| Dense truth, final rule | 1.46 (0.73 / 2.21) | 6.30 | 1.16 | — | — |
| Prototype dark + r40 | 1.53 (0.77 / 2.31) | 6.30 | 1.00 at 0.250 | 0.07 / 1.3 | — |
| Ramp alone | 5.36 (5.34 / 6.56) | 6.84 | 1.17 | 3.80 / 15.2 | — |

- The full-frame band B matches mask 02 to 0.01, since the band is all static indirect.
- **Verdict:** edge copy did not regress band B anywhere in t′. The edge pose's lit value leaking at the 0.03 floor never shows above +0.05 mean. The second dark pose about 1 u inside is not needed.

### c. Dark blotches (most negative 8×8 block; frames with any block < −3)

| View | Final vs truth | Final vs ramp alone | Prototype vs ramp alone |
|---|---|---|---|
| Sweeper east (rides) | −2.97 at 0.277 (0) | −0.17 | −1.01 |
| Sweeper west (rides) | −1.63 at 0.500 (0) | −0.07 | −2.16 |
| Wall end | −4.13 at 0.317 (12) | −0.21 | −1.62 |
| Wall north | −9.57 at 0.317 (17) | −0.04 | −0.09 |
| Fixed east | −13.13 at 0.980 (19) | −0.34 | −1.88 |
| Fixed west | −2.44 at 0.530 (0) | −0.28 | −2.42 |

The vs-truth worsts on the wall end and north face and the fixed east view are present in the pre-dark builds against their own truth too. At the same frames, ramp alone against its truth reads −4.13, −9.56 and −13.13, and pre-amendment against its truth −3.77, −9.68 and −15.3. A truth-relative bar therefore cannot separate a dark-state blotch from the existing interpolation miss, and the prototype's −16.9 spheres were in its dense truth too.

### d. Enclosure exits and entries (above-door band A; samples every 1/8 frame, one-frame span = 0.00373 t′ at every phase)

Windows run from 0.0035 before the first geometry knot to 0.0035 after the second. The knots come from the room's id 57. Reference: the ramp alone (prototype engine, r40, pre-dark bake), which matches the old `head-r40` captures pixel for pixel. Dip: the worst block below its inside-pose value (exit) or below the window's last, settled value (entry).

| Crossing (knots) | Final dip / step | Ramp alone dip / step | Prototype dark dip / step | Final − ramp alone (dip, step) |
|---|---|---|---|---|
| entry 0.0221–0.0300 | 0.00 / 0.05 | −0.77 / 0.34 | 0.00 / 0.05 | +0.77, −0.29 |
| exit 0.0741–0.0820 | −1.00 / 1.00 | 0.00 / 0.00 | −1.00 / 1.00 | −1.00, +1.00 |
| entry 0.2458–0.2537 | −0.94 / 0.66 | −0.13 / 0.24 | −0.60 / 0.33 | −0.81, +0.42 |
| exit 0.2978–0.3057 | −1.00 / 1.00 | −3.41 / 2.40 | −1.00 / 1.00 | +2.41, −1.40 |
| entry 0.4695–0.4774 | −0.72 / 0.69 | −0.72 / 0.38 | −0.72 / 0.66 | 0.00, +0.31 |
| exit 0.5215–0.5294 | 0.00 / 3.53 | 0.00 / 4.81 | −1.11 / 4.29 | 0.00, −1.29 |
| entry 0.6932–0.7011 | −0.93 / 0.69 | −0.98 / 0.57 | −0.91 / 0.54 | +0.05, +0.12 |
| exit 0.7452–0.7531 | 0.00 / 3.80 | 0.00 / 3.63 | −2.60 / 5.50 | 0.00, +0.16 |
| entry 0.9169–0.9248 | −0.97 / 0.66 | −0.82 / 0.41 | −0.96 / 0.66 | −0.15, +0.25 |
| exit 0.9689–0.9768 | **0.00 / 3.91** | 0.00 / 4.33 | **−3.51 / 6.12** | 0.00, −0.43 |

- The −0.7 to −1.0 entries and the two early exits are one 8-bit level on a band whose mean is 0.2–1.5 luma there, and the ramp alone shows the same −0.7 to −3.4. At 0.0741 and 0.2978 the band settles about 0.25–0.8 mean lower, a level change rather than a dip (`a2-final-exits-A.png`).
- The 0.0741 exit leaves the run that holds the t′ 0 terminus. There the final build equals the prototype, and the ramp alone holds a flat 1.0.
- **Against Fable's proposed bars:** the step bar (≤ 1.0 above ramp alone) is met, exactly at the 0.0741 exit (+1.00) and with ≥ 0.58 headroom everywhere else. The 0.75 dip bar is missed by one quantum at five dark crossings (0.72–1.00), which the ramp alone misses too.

### e. Re-light into rest (band B, t′ 0.95–1.0, 60 fps)

Final 0.93 at t′ 0.985 (no frame over 1.5), prototype 0.92, dense truth 1.16. Bar 1.5: **pass**.

### f. Rest frames (final HEAD at t′ 0 and 1 against pre-amendment, `head-cur`)

| View | t′ 0: mean d / worst block | t′ 1: mean d / worst block |
|---|---|---|
| Ceiling, whole frame | −0.37 / 13.8 | −0.07 / 14.7 |
| Ceiling band A | −3.34 / 9.9 | −0.21 / 1.0 |
| Ceiling band B | −4.70 / 11.0 | −0.92 / 4.0 |
| Sweeper east (rides) | −0.16 / 3.4 | +0.83 / 7.3 |
| Sweeper west (rides) | −0.09 / 5.6 | −0.15 / 4.1 |
| Wall end | 0.00 / 0.0 | +0.11 / 2.6 |
| Wall north | −0.07 / 3.6 | 0.00 / 0.0 |
| Room floor | +0.01 / 1.0 | −0.43 / 10.6 |
| Fixed east | −0.03 / 3.5 | +0.34 / 13.8 |
| Fixed west | −0.18 / 9.2 | −0.02 / 4.3 |
| `wall_owner` | +0.20 / 10.5 | 0.00 / 0.0 |
| `sweeper_east` | −0.13 / 2.8 | +0.99 / 6.5 |

Against the ramp-alone captures, all 24 frames are pixel-identical. The rest changes are the ramp's alone: the dark state and edge copy leave terminus states untouched, as AC 1 confirms below.

### Test rows (`--release` PRLs from the final `prl-build`; baseline `prl-build` at 27c92488f)

| Row | Result | Key numbers |
|---|---|---|
| AC 1 `rest_channel_output_equals_the_pre_change_build` | pass, bit for bit | 36 atlas × mode × t′ comparisons with 0 differing halves on closet, setpiece and enemy-movers. Probes 157, 11,403 (pre-change 11,401) and 3,887 (3,885). Pairs stored by mid poses alone: bounce 0, 60 and 45; depth 0, 257 and 279 |
| C7 `c7_a_lagging_channel_row_…` | pass | Closet: 144 probes; setpiece (resident bar 0): 11,359. 0 halves off on re-entry and on every install frame |
| C10 per-probe, BC6H | pass | Worst fraction of the derived bound: setpiece 0.901 (was 0.968), enemy-movers 0.927, closet 0.493 |
| C10 per-probe, `--uncompressed-irradiance` | pass | Bit for bit in every class on all three maps |
| C10 frame | pass | Setpiece st3 mask 8: 17 against bar 19 (baseline gap 15); st2 mask 8: 11 against 15; mask 2: 2–5 against 6–11 |
| K1 | pass | Closed st2 0.0097 against the static door's 0.0100; open equals stripped (0–1 px) |
| K2 | pass | Receiver: wall closed 27.48, static-wall reference 28.97, open 31.09 (stripped 31.09) |
| K3 | pass | Setpiece pop 5.03 at t 0 against ungated 5.59 + 12; animated 0.00 |
| D2 | pass | Stripped against none: delta 0; without state against depth stripped: 0 |

### Proposed bars (the owner sets them)

| Row | Proposed | Final build | Headroom and discrimination |
|---|---|---|---|
| Hand-over | Largest per-frame step ≤ the truth's step at the same t′ + **4.0** | +3.16 worst (wall north, t′ 0.45, the window end; ramp alone the same) | 0.84 headroom. Pre-amendment fails: wall end +10.57, north +5.81 |
| In-wall band | Band B whole-trip block mean \|d\| ≤ **0.25** and worst block ≤ **2.5** against the same-rule dense truth | 0.09 / 1.3 | About 2× headroom. The ramp alone fails (3.80 / 15.2) |
| Dark blotch | No mover-face block darker than the same build without the dark state (the ramp-alone bake) by more than **3.0** | 0.34 | The prototype under today's backface reached −16.9. A truth-relative bar is not proposed: the existing interpolation misses (−4.1 to −13.1) share it |
| Exits and entries, dip | ≤ **1.5** below the inside-pose or settled value | 1.00, one quantum | The prototype dark fails at 3.51 (0.9716) and 2.60 (0.748). Fable's 0.75 fails here and on the ramp alone at dark crossings |
| Exits and entries, step | ≤ ramp alone + **1.5** over any one-frame span | +1.00 at 0.0741, ≤ +0.42 elsewhere | The prototype dark fails at +1.87 (0.748) and +1.79 (0.9716). Fable's 1.0 is met exactly, with no headroom |
| Re-light into rest | 1.5 (set) | 0.93 | — |

### Files (task 25c)

- Here:
  - charts: `a2-final-band.png`, `a2-final-exits-A.png`, `a2-final-handover.png`;
  - clip: `clips/a2-final-exit-0.9689.mp4`, the 0.9716 exit at 1/8-frame steps, 16× slow, final | ramp alone | prototype dark;
  - tables: `final-band.txt`, `final-exits.txt`, `final-handover-blotch.txt`, `final-rest.txt`;
  - tools: `tools/fcap.py`, `flib.py`, `fanalyze.py`, `fexit.py`, `fcharts.py`, `fclip.py`, `knots.py`, `relabel.py`, `fbake.sh`, `fcaps.sh`, `ftestbakes.sh`.
- Scratch `amend2\final\`: `bin\`, `ws\` (PRLs, logs in `logs\`), `cap\` (about 5,100 frames, 1.3 GB), `base\prl-build-parent.exe`.

## Big room re-capture (Windows, 2026-11-13)

Re-measures the Amendment 2 rows on `mover-setpiece` after df00bd54e added `setpiece_room_big`. Builds are at HEAD dbacdfdf2. Machine and settings are as in 25c: GTX 1660 SUPER, Vulkan, offscreen 960×540, `sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover.

The lattice moved: 52×18×55, cell 1 m. The bake's knots confirm its layers. Every predicted crossing of the room's planes past a z layer at −336 + 39.37k, and of the arch past the x columns 1678.9, 1718.3 and 1757.7, falls between a geometry-knot pair. The west layer is at x 1521.5 and the south layer at y 1354.0, as briefed.

### Answers first

- **In-wall layer: pass.** The ceiling strip over the south wall is the thick-wall case: the layer y 1354.0 sits 6 u inside that wall. Its static indirect over the whole trip is within 0.05 mean and 1.0 worst block of the same-rule truth. Every other band is within 0.00–0.11 mean and 0.9–2.1 worst. Ramp alone reads 0.97 / 7.3 on the south strip, which fails both bars, so the row discriminates there. On the west strip, ramp alone fails only the mean bar (0.51 / 2.0). The room is very dark inside (band means 0.1–2 luma), so most of the discrimination comes from the south and arch strips.
- **Dark blotch: fails as written.** Against ramp alone, HEAD darkens a block by up to 17.3 (wall face above the arch, t′ 0.019). That is not a blotch: ramp alone leaks in-wall light into every interior view, the residue the dark state exists to remove. HEAD is 0.3 from the dense truth there, and ramp alone is 17.6 above it. The fixed chamber and hallway views pass (worst 2.98).
  - A block counts as darker only when it is darker than both ramp alone and the truth. The worst such block is −4.68, on the ceiling over the arch at t′ 0.651, across 10 frames.
  - Static indirect there equals the truth exactly. The shortfall is in the direct share, inside one 16-frame stored interval (knots 0.6348–0.6746) where the truth has a transient peak: HEAD 2.1, ramp alone 4.4, truth 6.8.
  - About 2.2 of the shortfall is the dark state, under 3.0. The rest is long-interval direct interpolation.
  - Everywhere else, darker-than-both is within 1.65.
- **Re-light into rest: pass.** The largest per-frame block step over t′ 0.95–1.0 is 0.81 on the in-wall bands' static indirect and 1.37 full frame (bar 1.5).
- **Enclosure exits and entries.** I measured all 42 visible crossings: 122 view-region windows at 1/5-frame steps (0.000482 t′). The one-frame span is 0.002412 t′.
  - **Step: pass.** The worst is ramp alone + 0.57 (floor-top entry, t′ 0.488).
  - **Dip: fails as written, on HEAD and ramp alone alike.** HEAD's worst is −6.79 (roof-top entry, t′ 0.244), identical to ramp alone. Every dip past 1.5 is a monotone level change on a face rising through a lighting gradient: the roof top, the floor by the arch, the wall face above the arch.
  - **Undershoot: no fall that recovers.** 25c's artifact was a dip and rise. HEAD's undershoot below both its inside-pose and settled values is never more than 0.35 beyond ramp alone's. The worst is −2.80, the same as ramp alone's, on the roof top.
- **Hand-over: fails on one surface, by 0.75.** On the big room's ceiling over the arch, HEAD steps 8.82 in one frame at the ceiling exit of layer z 97.07 (t′ 0.7695), where the truth steps 4.07. That is +4.75 against the +4.0 bar. Ramp alone fails there too, at +4.52.
  - The step is physical. A probe leaves the roof slab into the lit air under the arch, and the block brightens by about 20 levels.
  - Geometry knots place that change exactly on the 2.6-frame knot pair at the enclosure exit. The truth's 98 even poses smear it over about 4 frames, starting 2 frames early.
  - It is not the dark state. The exits row, judged against ramp alone, passes it (−0.32).
  - Every other surface passes: wall door end +1.64, north +0.34, small room ceiling +0.78 and +0.75, big ceiling seen from the west +0.27.
- **Edge copy:** every band's static indirect is within the in-wall bar (worst 2.1). The full frame's worst blocks pass 2.5 at three t′:
  - t′ 0.649: the direct-share miss above, 11.0;
  - t′ 0.912: the arch exit, 5.3;
  - t′ 0.989: 4.9–8.7, where the truth's own step toward its last pose is 2.4–3.4.
- **Rest:** HEAD equals the dense truth pixel for pixel in all 18 view × terminus frames. Against ramp alone it is identical in 17 of 18; in walledge at t′ 0 one pixel differs, with a worst block of 0.01.
- **What changed with the lattice.**
  - The wall door no longer encloses a layer. It bakes 15 even poses and no geometry knots, so HEAD and ramp alone are identical on it, and its hand-over measures Ruling A alone.
  - The small room falls to 37 poses (5 even + 32 geometry) from 45. Its ceiling steps at most 1.0 per frame in every column.

### Build and bakes

- **HEAD builds.** Release `prl-build` (4 min 39 s) and `postretro --features capture` (12 min 29 s) at dbacdfdf2, with `CARGO_INCREMENTAL=0`. They are kept in scratch `amend3\bin\`.
- **Ramp alone.** The prototype binaries in `amend2\bin\`: `prl-build` at f712cfc14 (pre-dark, epoch 6), and the engine with `POSTRETRO_PROTO_MOVER_SH` set to `r40`.
  - Every ramp capture's stderr confirmed `AMEND2-PROTO`. `bcap.py` refuses a capture otherwise.
  - Its knots and rest frames equal HEAD's (rest above).
- **Bakes.** Each is a cold `--release` bake with a fresh cache dir (`tools/bbake.sh`).
  - The scratch ws holds junctions to the repo's `content/dev/{textures,models,sounds,ui,scripts}` and `baked/`, plus a copy of `start-script.js`.
  - The views were checked for textures before the batches ran.

| Bake | Wall time | Poses (even + geometry): wall, room, room_big | Big-room bounce bake | id 57 |
|---|---|---|---|---|
| HEAD | 151.4 s | 15 (15+0), 37 (5+32), 74 (6+68) | 97.2 s | 26,880,284 B, epoch 7 |
| ramp alone | 151.2 s | the same knots | 98.6 s | 26,119,152 B, epoch 6 |
| truth: wall 267, room 76, room_big 98 | 253.7 s | 267, 76, 98 (all even) | 124.1 s | 34,607,171 B, epoch 7 |

- No bake logged a band nudge or a `held the probe` line, so 98 is clean for the big room.
- **The bounce-bake estimate is short.** The Build Summary projects the big room's `--release` bounce bake at 18.7 s; it measured 97.2 s, about 5× that. The other movers are closer: 7.1 s against an estimate of 5.8 s, and 18.9 s against 6.3 s.
- **States kept after skipping.**
  - Big-room bounce: HEAD 6,249, ramp alone 4,454, truth 6,765.
  - Depth: HEAD keeps 24,061.

### Views and regions

- **Riding views** ride `setpiece_room_big`, so regions hold at every t′. Positions are at the lowered pose.
  - `bw` (1640, 1576, −160), yaw 180, pitch 35: the west wall and the ceiling over it.
  - `bs` (1728, 1464, −160), yaw 270, pitch 35: the south wall above the arch and the ceiling over it.
  - `bfloor` (1820, 1720, −180), yaw 225, pitch −30: the floor toward the south-west corner.
  - `broof` (1760, 1640, 40), yaw 225, pitch −45: the roof top.
- **Fixed views:**
  - `barch` (1728, 1180, 80), yaw 90: the chamber, square on to the arch wall.
  - `bch` (1400, 1200, 200), yaw 45, pitch −25: the chamber's south-west corner.
  - `bhall`: README station 4.
- **Regions.** A per-pixel ray cast against the room's inner box (`blib.py`) defines them. It was checked against the captures' corner and arch edges.
  - **Ceiling strips:** W (x < 1560.9), S (y < 1393.4, beside the arch) and A (y < 1393.4, over the arch).
  - **Wall faces:** WF, SF, and AF (above the arch).
  - **Floor strips:** FW and FS.
  - **Whole faces:** `ceil`, `floor` and `top`.
- **East and north layers.** The east (x 1954.5) and north (y 1826.4) in-wall layers sit 34.5 u behind their faces, outside every lookup's trilinear reach. They feed no band, so no view watches them.

### 1 and 6. In-wall bands (whole trip, one sample per 60 fps frame, 416 samples; static indirect, mask 02)

| Band | HEAD mean | HEAD vs truth (block mean / worst) | Ramp alone vs truth | Ramp alone at the bar | Truth mean | Ramp-alone mean |
|---|---|---|---|---|---|---|
| **S, ceiling over the south wall** (layer y 1354.0) | 0.99 | **0.05 / 1.0** | 0.97 / 7.3 | fails both | 0.97 | 1.86 |
| W, ceiling over the west wall (layer x 1521.5) | 0.25 | 0.01 / 1.0 | 0.51 / 2.0 | fails mean | 0.26 | 0.72 |
| A, ceiling over the arch | 2.17 | 0.11 / 2.1 | 0.66 / 4.3 | fails both | 2.14 | 2.63 |
| AF, wall face above the arch | 0.15 | 0.01 / 1.0 | 0.12 / 2.8 | fails worst | 0.14 | 0.26 |
| SF, south wall face | 0.07 | 0.01 / 0.9 | 0.05 / 1.8 | passes | 0.07 | 0.09 |
| WF, west wall face | 0.05 | 0.00 / 1.0 | 0.07 / 2.0 | passes | 0.05 | 0.09 |
| FW, floor along the west wall | 0.63 | 0.00 / 0.9 | 0.38 / 2.9 | fails both | 0.63 | 1.02 |
| FS, floor along the south wall | 1.04 | 0.02 / 1.3 | 0.37 / 2.1 | fails mean | 1.02 | 1.40 |

**Verdict: pass on every band** (bar 0.25 / 2.5). Ramp alone fails six of eight, cleanly on S, A and FW.

**Edge copy: full-frame HEAD vs truth (block mean / worst).**

| Band | HEAD vs truth |
|---|---|
| S | 0.07 / 4.9 |
| W | 0.01 / 1.0 |
| A | 0.25 / 11.0 |
| AF | 0.03 / 5.3 |
| SF | 0.01 / 3.3 |
| WF | 0.00 / 2.7 |
| FW | 0.01 / 0.9 |
| FS | 0.05 / 8.7 |

The full-frame worsts fall at three t′:

- **0.649:** the direct-share miss (row 2).
- **0.989:** the truth's own step toward its last pose, 2.4 on S and 3.4 on FS.
- **0.912:** the arch exit of layer z 97.07, which ramp alone shares (4.9 against the truth).

No band regresses against ramp alone in static indirect, so no bounding dark pose is indicated.

### 2. Dark blotches (most negative 8×8 block, full frame; frames with a block < −3)

| View | HEAD vs ramp alone | HEAD vs truth | Darker than both | Ramp alone vs truth |
|---|---|---|---|---|
| bw ceiling | −2.00 at 0.357 (0) | −1.00 (0) | −1.00 | −1.00 |
| bw west wall face | **−4.74** at 0.015 (15) | −0.91 (0) | −0.76 | −1.00 |
| bs ceiling | **−9.77** at 0.620 (265) | −10.98 at 0.649 (35) | **−4.68** at 0.651 (10) | −9.38 at 0.649 |
| bs south wall face | **−10.52** at 0.015 (103) | −0.91 (0) | −0.87 | −1.00 |
| bs wall face above the arch | **−17.33** at 0.019 (167) | −5.28 at 0.912 (2) | −1.65 | −4.87 at 0.912 |
| bfloor floor | **−13.27** at 0.922 (220) | −2.21 (0) | −1.64 | −2.22 |
| bfloor west wall face | −1.40 | −1.00 | −0.39 | −1.00 |
| broof roof top (0:1:101) | −0.65 | −0.72 | −0.26 | −0.88 |
| barch, fixed (0:1:101) | −2.98 at 0.86 (0) | −4.16 at 0.65 (2) | −0.87 | −4.05 at 0.65 |
| bch, fixed | −2.40 (0) | −4.58 at 0.65 (1) | −0.29 | −4.57 at 0.65 |
| bhall, fixed | −2.42 (0) | −1.70 | −0.62 | −1.74 |

**Verdict as written (bar 3.0 against ramp alone): fail on five interior views.** Each is ramp alone's leak, which HEAD and the truth both remove. On the wall face above the arch at t′ 0.019, HEAD is 0.3 from the truth and ramp alone is 17.6 above it.

**Darker than both: −4.68.**

- The block is on the ceiling at local (1691, 1361), by the arch's west jamb, inside one 16-frame stored interval (knots 0.6348–0.6746).
- Static indirect there is 0.88 in HEAD and the truth at every frame; ramp alone reads 3.2–4.7.
- Full frame at t′ 0.651 reads 3.00 in HEAD, 7.88 in ramp alone and 7.68 in the truth. The direct share is therefore 2.1, 4.4 and 6.8.
- The truth has a transient direct peak at t′ 0.649, and both knotted builds interpolate under it. HEAD's extra 2.2 below ramp alone is plausibly the dark state's direct q at that interval's enclosed probes.
- This is the Motion-proof row's long-interval direct follow-up, not an exit artifact.

### 3. Re-light into rest (t′ 0.95–1.0, 60 fps; largest per-frame block step, static indirect / full frame)

| Band | HEAD | Truth | Ramp alone |
|---|---|---|---|
| S | 0.75 / 0.91 | 0.84 / 2.41 | 0.65 / 0.71 |
| W | 0.76 / 0.85 | 0.86 / 0.93 | 1.00 / 1.00 |
| A | 0.81 / 1.37 | 0.86 / 1.64 | 0.82 / 0.82 |
| AF | 0.28 / 0.71 | 0.32 / 1.51 | 0.27 / 0.39 |
| FW | 0.52 / 0.77 | 0.76 / 0.79 | 0.21 / 0.05 |
| FS | 0.39 / 1.26 | 0.50 / 3.42 | 0.26 / 0.95 |

**Verdict: pass** (bar 1.5): the worst is 0.81 in static indirect and 1.37 in full frame. Leaving rest (t′ 0–0.05) has no bar. There the worst is the arch exit at t′ 0.016 on the wall face above the arch: 3.17 full frame, against 3.45 for the truth.

### 4. Enclosure exits and entries (window 0.003 before the first geometry knot to 0.003 after the second; samples every 0.000482 t′; one-frame span 0.002412 t′ at every phase)

The crossings are every pass of a room plane through a lattice layer. The floor bottom's 6 exits leave into the sealed shaft, which no view sees.

| Plane | Kind | Crossings | Views and regions |
|---|---|---|---|
| Ceiling | exit | 7 | bw W, ceil; bs S, A, ceil |
| Arch bottom (x 1678.9, 1718.3, 1757.7) | exit | 21 | bs A, AF |
| Floor top | entry | 7 | bfloor FW, FS, floor |
| Roof top | entry | 7 | broof topW, topS, top |

**Selected crossings.** Each cell is HEAD / ramp alone. "Undershoot" means the block falls below both its first and settled values. `big-exits.txt` holds all 122 rows.

| Crossing | Region | Dip (25c metric) | Undershoot | Step, one frame | Step − ramp alone |
|---|---|---|---|---|---|
| roof top 0.0991 | broof topS | −4.96 / −4.96 | −0.37 / −0.41 | 2.12 / 2.12 | +0.00 |
| roof top 0.2439 | broof topS | **−6.79** / −6.79 | −2.80 / −2.80 | 3.53 / 3.53 | +0.00 |
| roof top 0.3886 | broof topS | −5.10 / −5.10 | −1.85 / −1.86 | 2.69 / 2.69 | +0.00 |
| arch x 1718.3, 0.4498 | bs AF | −2.26 / −0.85 | −0.02 / −0.33 | 1.57 / 1.20 | +0.37 |
| arch x 1757.7, 0.4809 | bs AF | −2.47 / −6.07 | −0.05 / 0.00 | 2.12 / 2.87 | −0.75 |
| floor top 0.4884 | bfloor FW | −0.19 / −0.12 | 0.00 / −0.03 | 0.98 / 0.40 | **+0.57** (worst) |
| ceiling 0.7686 | bs A | −0.93 / −0.90 | −0.40 / −0.06 | **13.67** / 13.99 | −0.32 |
| floor top 0.7779 | bfloor floor | −3.90 / −3.87 | −0.08 / −0.15 | 1.38 / 1.36 | +0.02 |
| ceiling 0.9134 | bs A | −3.99 / −3.42 | −0.32 / −0.29 | 3.80 / 3.92 | −0.12 |
| arch x 1757.7, 0.9151 | bs AF | −2.69 / −9.64 | −0.38 / −0.30 | 5.18 / 5.33 | −0.14 |
| floor top 0.9227 | bfloor floor | −3.37 / −3.35 | −0.82 / −0.47 | 1.51 / 1.07 | +0.44 |

**Verdicts.**

- **Step bar** (ramp alone + 1.5): **pass**, worst +0.57.
- **Dip bar** (1.5): **fails as written.** HEAD exceeds 1.5 in 25 rows, and ramp alone in 28. Each such series is a monotone level change across the window, not a fall that recovers (`a3-exits.png`). Three cases:
  - the roof top dims as it rises toward the chamber's ceiling light;
  - the ceiling over the arch brightens as a lit probe exits the roof;
  - at t′ 0.4498, the wall face above the arch settles darker in HEAD (worst block 5.6 → 1.5), where ramp alone leaks.
- **Undershoot,** 25c's dip and rise: HEAD's worst is −2.80, equal to ramp alone's, on the roof top. HEAD never undershoots more than 0.35 beyond ramp alone.

**Diagnosis.** 25c's bands were flat outside crossings, so an absolute dip read the artifact alone. On lit moving faces it also reads the scene's lighting gradient. Judged against ramp alone, HEAD shows no edge-copy or dark-state dip at any crossing.

### 5. Hand-over (largest 8×8-block step per 60 fps frame; over truth = HEAD's step minus the truth's at the same t′)

| Surface | HEAD | Over truth | Truth's own max | Ramp alone (over truth) |
|---|---|---|---|---|
| Wall door trailing end face (t′ 0.10–0.45) | 9.65 at 0.168 (12 > 3) | +1.64 at 0.385 | 9.76 | 9.65 (+1.64), identical |
| Wall door north face | 1.18 | +0.34 | 5.81 | identical |
| Small room ceiling above the door (t′ 0.10–0.60) | 1.00 | +0.78 | 1.00 | 1.00 (+0.79) |
| Small room ceiling over the west wall | 0.99 | +0.75 | 0.99 | 1.00 (+0.91) |
| **Big room ceiling, west view (whole trip)** | 0.92 | +0.27 | 0.93 | 1.00 (+1.00) |
| **Big room ceiling, south view (whole trip)** | **8.82 at 0.7695** (13 > 3) | **+4.75 at 0.7695** | 7.99 at 0.902 | 8.59 (+4.52) |

**Verdict: fail on the big ceiling's south view, by 0.75; every other surface passes.**

**Diagnosis.** The block is on the ceiling by the arch, at local (1708, 1366).

- The probe at layer z 97.07 leaves the roof slab into the arch's lit air.
- HEAD reads 7.5, 16.3, 22.7 and 26.2 at t′ 0.7671, 0.7695, 0.7720 and 0.7744. Its depth tile opens over the 2.6-frame geometry-knot pair 0.7671–0.7733, exactly at the enclosure end.
- The truth's even poses are 4.3 frames apart. It reads 16.3, 20.4, 23.7 and 25.6 at the same t′, having started at 7.5 at t′ 0.7623. That spreads the same ~20-level rise over about 4 frames.
- Ramp alone does the same (+4.52), and against ramp alone in the exit row it is −0.32. So the step comes from the geometry knots, which are exact here, while the truth smears it; it is not the dark state or edge copy.
- Clip: `clips/a3-exit-0.7671.mp4`.

### 7. Rest frames (HEAD at t′ 0 and 1)

- **Views:** bw, bs, bfloor, broof, barch, bch, bhall, door_ceiling and walledge, 18 view × terminus frames.
- **Against the dense truth:** all 18 are pixel-identical.
- **Against ramp alone:** 17 of 18 are identical. In walledge at t′ 0, one pixel differs, with a worst block of 0.01.

The dark state and edge copy never touch termini.

### Proposed AC wording (the owner decides)

- **Hand-over.** "Dense t′ series, built `--release`, on em_sweeper's east face, the wall door's trailing end face and north face, the small rising room's ceiling and `setpiece_room_big`'s ceiling. The largest frame-to-frame 8×8-block step is at most the dense same-rule truth's step + 4.0, outside enclosure-exit windows, which the Enclosure row judges (final build +3.16; big-room re-capture +1.64, and +4.75 inside the t′ 0.7686 ceiling-exit window, ramp alone +4.52; pre-amendment +10.6, which fails)." Without the carve-out, the bar would need + 5.0.
- **In-wall layer.** "On `mover-setpiece`'s `setpiece_room_big`, whose 64-unit walls hold probe layers (south y 1354.0, west x 1521.5), with the ramp and the dark state, the ceiling and floor strips over its in-wall layers and the arch are within a block mean of 0.25 and a worst block of 2.5 of a dense truth baked under the same rule, in static indirect over the whole trip (re-capture: south 0.05 / 1.0, worst band 0.11 / 2.1; ramp alone south 0.97 / 7.3, which fails). No mover-face or fixed-view block reads more than 3.0 darker than both the ramp-alone bake and that truth (re-capture −4.68 at t′ 0.651, a long-interval direct miss; see the Motion-proof follow-up)." As written, against ramp alone, the blotch bar fails wherever ramp alone leaks (up to −17.3), so it cannot judge the big room.
- **Re-light into rest.** "On `setpiece_room_big` at 60 fps over the last 5 % of travel, with the ramp and the dark state, the in-wall bands' largest per-frame 8×8-block step is at most 1.5 (re-capture 0.81 in static indirect, 1.37 full frame)."
- **Enclosure exits and entries.** "On `setpiece_room_big`, across every crossing of its roof and floor slabs and arch top past a probe layer that a riding view sees (42 crossings), at t′ steps of 0.0005 or less, with the ramp:
  - no 8×8 block undershoots by more than 1.5 beyond the ramp-alone build's undershoot at the same crossing, where undershooting means falling below both its inside-pose (exit) or first (entry) value and its settled value (re-capture +0.35);
  - the largest block step over any one-frame span (0.00241 t′, at any phase) is no more than 1.5 above the ramp-alone build's (re-capture +0.57)."

  The absolute 1.5 dip below the inside-pose value reads a lit face's lighting gradient as a dip: −6.79, the same in ramp alone.
- **Edge copy.** "… The big room's in-wall bands stay within the in-wall row's bar over the whole trip (re-capture worst band 0.11 / 2.1, static indirect), or a bounding dark pose about 1 u inside keeps them there."

### Files (big-room re-capture)

- **Here:**
  - Charts: `a3-band.png`, `a3-exits.png`, `a3-handover.png`.
  - Clip: `clips/a3-exit-0.7671.mp4`, the t′ 0.7686 ceiling exit at 1/5-frame steps, 10× slow, HEAD | ramp alone | truth.
  - Tables: `big-analyze.txt` (rows 1, 2, 3, 5 and 7), `big-exits.txt` (row 4), `big-bakes.txt`.
  - Tools: `tools/blib.py`, `bcap.py`, `bexit.py`, `banalyze.py`, `bcharts.py`, `bclip.py`, `bbake.sh`, `bcaps.sh`.
- **Scratch `amend3\`:**
  - `bin\`: the HEAD binaries.
  - `ws\`: the PRLs and the truth map.
  - `logs\`: bakes and captures.
  - `cap\`: 11,919 frames, 1.5 GB.
  - `results\`: `bsummary.json`, `bexit.json`.
