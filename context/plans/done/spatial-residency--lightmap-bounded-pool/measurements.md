# Task 10 measurements

Raw results of the manual rows, kept for the brief. Paths under `S/` are the build session scratchpad and do not survive it.

## Task 10a results: rebakes, baselines, no-GPU manual rows

HEAD f22f15152, Intel i9-9980HK Mac, machine otherwise idle during the cold hallway bake. All compiler runs are the `dev` profile, the profile of the prior bakes (bakebin sizes matched target/debug). Raw logs: S/t10a/ and S/baseline/.

## Job 1: bake binaries
`cargo build -p postretro-level-compiler --bin prl-build -p postretro-script-compiler --bin scripts-build` at HEAD, copied to S/bakebin/ (2m14s).

## Job 2: rebakes at HEAD, all cold, in place under content/dev/maps/
Every map was baked `--no-cache`, not only the hallway, because the Manual block says "cold PRL rebaked at this commit" and old baselines are cold too.

Command: `S/bakebin/prl-build /Users/dhiester/Projects/Personal/postretro/content/dev/maps/<map>.map -o /Users/dhiester/Projects/Personal/postretro/content/dev/maps/<map>.prl --no-tui --no-cache --cache-dir /Users/dhiester/Projects/Personal/postretro/.build-caches/prl-cache --baked-root /Users/dhiester/Projects/Personal/postretro/baked` (`--no-cache` wins over `--cache-dir`; `--release` is the same code path, stage cache None.)

| Map | Wall s | PRL bytes |
|---|---|---|
| stress-warren-hallway-inspection | 1,429.9 (23m50s), run alone, peak footprint 4.15 GiB | 2,050,165,462 |
| occlusion-test | 19.8 | 73,147,812 |
| specular-shadowmask-capture | 2.6 | 8,079,796 |
| shadowmask-groups-capture | 4.2 | 12,304,762 |
| campaign-test | 62.9 | 252,744,809 |
| reflection-lake-test | 101.7 | 113,347,889 |

Only the hallway's time is clean: the other five ran beside a cargo test build.

Hallway cold sha256 46e0ccda102e49f6f1c18d3d2cdc21daac771debb25296729d663d63e7bfebcc.

Build Summary lightmap pool lines:
- hallway: Lightmap pool memory: 274.0 MiB at 16 m lead (how far ahead of the camera lightmaps load), 34 lightmap and 15 shadow mask layers, sized from a worst case of 27 and 12; demand peaks in cell 4648, near (21.1, 4.9, 39.0) engine metres
- campaign-test: Lightmap pool memory: 128.0 MiB at 16 m lead (how far ahead of the camera lightmaps load), 13 lightmap and 10 shadow mask layers, sized from a worst case of 10 and 9; demand peaks in cell 78, near (-29.7, 1.4, -46.3) engine metres
- occlusion-test: Lightmap pool memory: 64.0 MiB at 16 m lead (how far ahead of the camera lightmaps load), 7 lightmap and 3 shadow mask layers, sized from a worst case of 7 and 3; demand peaks in cell 42, near (-43.1, 10.0, 6.5) engine metres
- specular-shadowmask-capture: Lightmap pool memory: 10.0 MiB, 1 lightmap and 1 shadow mask layers, the whole map: without usable portals every lightmap stays loaded
- shadowmask-groups-capture: Lightmap pool memory: 14.0 MiB, 1 lightmap and 2 shadow mask layers, the whole map: without usable portals every lightmap stays loaded
- reflection-lake-test: Lightmap pool memory: 30.0 MiB at 16 m lead (how far ahead of the camera lightmaps load), 2 lightmap and 2 shadow mask layers, sized from a worst case of 2 and 2; demand peaks in cell 17, near (-74.0, -2.0, -73.2) engine metres

No map warned past 384 MiB. specular-shadowmask-capture and shadowmask-groups-capture have no usable portals, so no id 58 and all-resident (already recorded in plan.md).

## Row 66, Bound cost: PASS, nothing for the owner
Method: the Build Summary's own `Lightmap bound` line. No A/B was needed: it is an `Instant` around all of `resolve_lightmap_pool_bound`'s work (`pool_demand`, prices, `all_resident_layers`, `BoundCellInput`, `worst_cell_demand` over every camera cell; crates/level-compiler/src/pack/lightmap_pool_bound.rs). The figure sits inside Packing and does not add to Total. Not inside the timer: encoding and read-back of a 12-byte section, and the post-bake mask gate, which is one all-resident pack (a part of what the timer already covers, so below it).

Warm command: as above without `--no-cache`, `-o` to scratch. Four runs. The first was already warm: every stage hit entries from an earlier bake.

| Build | Lightmap bound s | Packing s | Total s | Share of total |
|---|---|---|---|---|
| Cold | 0.16 | 29.78 | 1,428.91 | 0.011% |
| Warm 1 | 0.15 | 25.07 | 109.96 | 0.14% |
| Warm 2 | 0.16 | 25.30 | 105.79 | 0.15% |
| Warm 3 | 0.15 | 24.81 | 106.51 | 0.14% |
| Warm 4 | 0.16 | 24.81 | 100.97 | 0.16% |

The run-to-run spread of warm Total (9 s) is about 60 times the bound. Warm PRL sha256 equals the cold one on runs 1 and 4 (whole file, so row 7 holds on the hallway). Dev profile; a release compiler would be no slower. Side note, not the bound: warm listed stages sum to about 54 s of about 106 s, so about half a warm build is unlisted time.

## Row 60, Walks: NO GROWTH AT ANY STEP; 36/16 BEATS 5/4, GOES TO THE OWNER
Harness: `lightmap_residency_walks_from_prl`, real `LightmapPoolModel`, no GPU, rebaked hallway, 2,000 steps, completed reads, default lead 16 m. Frame counts equal research.md's: 90,767 and 97,610.

`POSTRETRO_LIGHTMAP_WALK_PRL=/Users/dhiester/Projects/Personal/postretro/content/dev/maps/stress-warren-hallway-inspection.prl POSTRETRO_LIGHTMAP_WALK_STEPS=2000 POSTRETRO_LIGHTMAP_WALK_READS=completed POSTRETRO_LIGHTMAP_WALK_POOL_LAYERS=32/14,34/15,36/16 <test binary> lightmap_residency_walks_from_prl --ignored --nocapture`

"One step": research.md frames the margin through its tour table, whose rows move 2 lightmap layers (32, 34, 36, naming 32 and 34 in its Margin section). So one step is 2 lightmap layers, with the mask pool at the same ratio since the margin is shared: 32/14 is 7/6 of 27/12 and 36/16 is 4/3 (the retired GROWTH_HEADROOM's ratio). Each is 16 MiB. Those rows compare directly with research.md's 32 and 36. A step of 1/8 in the ratio (9/8, 11/8) gives 31/14 and 38/17, in the supplement below.

Runs are deterministic: the pilot's 34/15 rows equal the ladder's. "Missed" is the harness's not-resident block-frames over all step kinds. Every drawn block outside the baked set was resident in every run (0 drawn non-resident); 45 random and 133 tour outside-set block-frames at every size. Repacks and compactions are lightmap+mask. No compaction rolled back in any run.

Far-point tour:

| Pools | Margin | Held MiB | Growths | Repacks | Compactions | Reads per step MiB | Hit rate | Missed |
|---|---|---|---|---|---|---|---|---|
| 32/14 | 7/6 | 258 | 0 | 98+114 | 22+5 | 16.63 | 87.0% | 3 |
| **34/15** | **5/4** | **274** | **0** | **65+75** | **10+4** | **15.73** | **88.5%** | **3** |
| 36/16 | 4/3 | 290 | 0 | 32+63 | 1+1 | 14.93 | 89.6% | 1 |

Random walk:

| Pools | Margin | Held MiB | Growths | Repacks | Compactions | Reads per step MiB | Hit rate | Missed |
|---|---|---|---|---|---|---|---|---|
| 32/14 | 7/6 | 258 | 0 | 16+58 | 4+10 | 13.88 | 93.4% | 0 |
| **34/15** | **5/4** | **274** | **0** | **10+32** | **3+3** | **13.20** | **94.5%** | **0** |
| 36/16 | 4/3 | 290 | 0 | 5+14 | 0+1 | 12.99 | 94.8% | 0 |

Against research.md's table (single pool, L = 16 m), same lightmap layer count:

| Walk, layers | Peak held MiB then, now | Growths then, now | Reads per step then, now | Missed then, now |
|---|---|---|---|---|
| Tour 32 | 1,050, 258 | 4, 0 | 30.3, 16.63 | 53, 3 |
| Tour 34 | 1,050, 274 | 3, 0 | 28.3, 15.73 | 29, 3 |
| Tour 36 | 1,106, 290 | 3, 0 | 27.3, 14.93 | 27, 1 |
| Tour 38 | 546, 306 | 0, 0 | 26.3, 14.49 | 26, 0 |
| Random 32 | 462, 258 | 0, 0 | 22.9, 13.88 | 0, 0 |
| Random 38 | 546, 306 | 0, 0 | 21.9, 12.74 | 0, 0 |

Repacks went the other way: research.md's tour at 34 layers had 19, this one has 140 across two pools (65+75), and that table's runs had growth. Row 62 should price them.

Supplement, same command, wider ladder, plus each pool moved alone. Format: held MiB; growths; repacks; compactions; reads per step; missed.

| Pools | Tour | Random |
|---|---|---|
| 27/12 | 220.0; 0+0; 191+216; 76+56; 19.29; 29 | 276.0; 0+1; 55+117; 12+28; 15.46; 0 |
| 30/14 | 246.0; 0+0; 128+104; 38+6; 17.14; 4 | 246.0; 0+0; 27+50; 4+6; 14.09; 0 |
| 31/14 | 252.0; 0+0; 118+116; 31+11; 16.77; 3 | 252.0; 0+0; 23+48; 3+8; 13.85; 0 |
| 33/15 | 268.0; 0+0; 81+86; 19+4; 15.98; 3 | 268.0; 0+0; 12+27; 0+5; 13.39; 0 |
| 35/16 | 284.0; 0+0; 44+56; 2+1; 15.15; 1 | 284.0; 0+0; 8+17; 1+2; 12.99; 0 |
| 38/17 | 306.0; 0+0; 11+40; 0+0; 14.49; 0 | 306.0; 0+0; 1+8; 0+0; 12.74; 0 |
| 32/15 | 262.0; 0+0; 97+89; 23+0; 16.20; 4 | 262.0; 0+0; 19+26; 2+3; 13.51; 0 |
| 36/15 | 286.0; 0+0; 33+86; 1+0; 15.29; 1 | 286.0; 0+0; 5+27; 0+3; 13.20; 0 |
| 34/14 | 270.0; 0+0; 65+121; 6+10; 16.02; 2 | 270.0; 0+0; 6+50; 0+13; 13.76; 0 |
| 34/16 | 278.0; 0+0; 65+51; 11+0; 15.38; 1 | 278.0; 0+0; 10+14; 0+3; 12.97; 0 |

Reading it:
- Growth does not separate any step: 0 from 30/14 up on both walks, so 5/4 holds the growth line.
- research.md's knee at 34 is gone. Missed is flat at 3 from 31/14 to 34/15, then 1, then 0 at 38/17. Reads fall smoothly, about 0.3 to 0.4 MiB per step per lightmap layer, with no flattening at 34.
- Repacks are what the margin now buys. Tour lightmap repacks 98, 65, 32, 11 at 32, 34, 36, 38 layers; compactions 27, 14, 2, 0.
- **36/16 is better than 5/4 on every walk figure, for 16 MiB (290 against 274, under the 384 MiB warning).** It halves tour lightmap repacks and takes compactions from 14 to 2. Whether that is worth 16 MiB depends on row 62's cost per repack, so that figure should reach the owner with this one.
- 34/16 is worth a look if the owner wants less: +4 MiB buys tour mask repacks 75 to 51, mask compactions 4 to 0, reads 15.38 and 1 missed. But the margin is one shared value (Decision), so it would need a per-pool margin.
- One route pair on a warren, per research.md's limits.

**At the bound, 27/12, the random walk grew the mask pool once, 12 to 13 layers, peak held 276 MiB, transient included.** The tour did not grow. Plan.md's "Row 43 limit" allows this (a real walk at `bound` can grow; the harness does not assert zero), and it is no margin, not the shipped size. Task 8's "0 growths" was 300 steps; 2,000 steps finds this one. Cause not established: walk mandatory max is 162.8 MiB, under the bound, so it is either drawn demand outside the set or a no-spare compaction miss (the mask pool built 4 target layers over a layer in that run; 0 rolled back). Both are already accepted paths. Not a false row, but the owner should know a real walk at no margin does grow.

## Row 65, Planar mirrors: 0 GROWTH, nothing for the owner, but the row shows little
Harness: `POSTRETRO_LIGHTMAP_WALK_PLANAR=1` runs the play frame's mirror step at the very high preset. No size passed, so each pool takes its first generation from id 58, as the runtime does. Off with `POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_OUT`, then on with `..._TRIP_RECORD_IN`, `POSTRETRO_LIGHTMAP_WALK_RUNS=default`, 2,000 steps, completed reads. Both maps have 1 candidate plane and the mirror ran in the walked route. Mirror frames and outside-set counts equal research.md's table exactly (52,581; 56,117; 8,541; 19,355; 1,429; 2,297; 23; 75).

Small harness change, uncommitted on the feature tree and kept separate: pool_mirror.rs prints all-resident layers beside the first generation, because id 58 and the Build Summary cannot give the all-resident figure when the margin does not reach it. Patch: S/t10a/harness_all_resident_print.patch. No behaviour change. Commit it or drop it.

| Map | First layers L/M | All-resident L/M | Bound L/M | At the cap |
|---|---|---|---|---|
| reflection-lake-test | 2/2 | 2/2 | 2/2 | both pools |
| campaign-test | 13/10 | 13/10 | 10/9 | both pools |

| Map, walk | Mirror | Frames | Growths L/M | Repacks L/M | Compactions rolled back | Reads per step MiB | Outside set | Not resident | Refusals |
|---|---|---|---|---|---|---|---|---|---|
| Lake, random | off | 89,005 | 0/0 | 0/0 | 0 | 0.00 | 0 | 0 | 0 |
| Lake, random | on | 89,005 | 0/0 | 0/0 | 0 | 0.00 | 1,429 | 0 | 0 |
| Lake, tour | off | 91,572 | 0/0 | 0/0 | 0 | 0.00 | 0 | 0 | 0 |
| Lake, tour | on | 91,572 | 0/0 | 0/0 | 0 | 0.00 | 2,297 | 0 | 0 |
| Campaign, random | off | 93,326 | 0/0 | 1/4 | 0 | 2.37 | 3 | 1 | 135 |
| Campaign, random | on | 93,326 | 0/0 | 1/4 | 0 | 2.37 | 23 | 1 | 135 |
| Campaign, tour | off | 97,308 | 0/0 | 47/28 | 0 | 3.14 | 2 | 7 | 415 |
| Campaign, tour | on | 97,308 | 0/0 | 53/28 | 0 | 3.18 | 75 | 8 | 434 |

"Outside set" is adjacent-step block-frames, the figure research.md tabled; the lake adds 24 and 18 on the spawn step with the mirror on, all resident. Campaign-test did compact, 13+15 on the tour, none rolled back. Every paired gate line reads deferred block-frames 0 vs recorded 0: within. Every pool sits at its all-resident cap, so growth could only have meant compaction failed; it did not. As research.md says, these maps fit whole, so this cannot exercise a mirror reaching past a pool. A large water map remains unmeasured. Campaign-test's refusals are R33's expected misses at the cap.

## Job 6: old-compiler baselines
Built from 6a813bdcd in a worktree at S/baseline/wt, `CARGO_INCREMENTAL=0`, own target (4.3 GB), dev profile, `-p postretro --bin postretro --features postretro/capture` plus prl-build and scripts-build (10m21s, beside the walks). Binaries copied; target deleted; worktree removed with `git worktree remove`. Content is byte-identical between 6a813bdcd and HEAD (`git diff` over content/, sdk/, core/ is empty), so the repo's .map files were used.

Bake: `S/baseline/bin/prl-build /Users/dhiester/Projects/Personal/postretro/content/dev/maps/<map>.map -o S/baseline/content/dev/maps/<map>.prl --no-tui --no-cache --baked-root S/baseline/baked`. No shared cache was read or written.

| Map | Wall s | Old PRL bytes | New PRL bytes |
|---|---|---|---|
| stress-warren-hallway-inspection | 1,340.4 | 3,252,447,988 | 2,050,165,462 |
| campaign-test | 56.0 | 311,786,215 | 252,744,809 |
| occlusion-test | 22.8 | 117,298,418 | 73,147,812 |
| specular-shadowmask-capture | 2.9 | 10,910,884 | 8,079,796 |
| shadowmask-groups-capture | 3.5 | 12,304,762 | 12,304,762 |

Section-by-section hash of old against new (t10a/cmp_sections.txt): in all five maps every section is byte-identical except id 42, and id 58 is new on the three that stream. Id 42 old to new bytes: hallway 1,451,952,620 to 249,670,060; campaign 91,747,280 to 32,705,840; occlusion 51,928,976 to 7,778,336; specular 5,662,408 to 2,831,320; groups 5,662,408 to 5,662,408, same size but tag SMB7 against SMB8 and different bytes. So the frame baselines differ from the HEAD PRLs by the masks alone, and shared poses and cell ids hold.

Layout matters: capture takes its materials root from the map path (`content_root_from_map`, then `derive_prm_root_dev_layout`) and ignores `--baked-root`. A PRL elsewhere would load with silent placeholder materials. So the PRLs sit at baseline/content/dev/maps/, sidecars at baseline/baked/materials/ (34 sidecars here, written by the old compiler), and baseline/content/dev/{textures,models,scripts,sounds,ui,...} are symlinks into the repo's content/dev. Keep that tree whole.

Run:
`cd S/baseline/check && POSTRETRO_SH_STREAMING=sync-proof RUST_LOG=warn S/baseline/bin/postretro --capture <scene.json>`
with `"map"` the absolute path under S/baseline/content/dev/maps/. Without `POSTRETRO_SH_STREAMING=sync-proof` capture refuses. Example scene: S/baseline/check/hallway.scene.json. Old scene files use the old field `lightmap_pool_cap_layers`; the old build has neither `lightmap_pool_layers` nor `mask_pool_layers`.

Confirmed before deleting the target: the old binary loaded the old hallway PRL headless and wrote a 320x240 capture and report on Metal, 5300M, mode stream, 1,840 blocks, 58 resident, pool 15 layers, 0 growths, no warnings (check/hallway_old.png, .report.json). specular-shadowmask-capture also captured. Report `revision` is null because the binary ran outside a checkout. The new walk harness refuses the old hallway PRL by name: "shadowmask section uses the retired every-plane-stored SMB7 layout; re-bake the level".

## Disk
| Point | Free |
|---|---|
| Start | 41 GB |
| After cold hallway | 40 GB |
| After job 5, old target present | 34 GB (low point) |
| End | 36 GB |

Went to: target/ 32 to 34 GB (test binary, 1.6 GB incremental), scratchpad to 3.8 GB (baseline 3.7 GB, of which old hallway PRL 3.0 GB). .build-caches stayed 30 GB: the warm builds spared the hallway's record. In-place PRLs shrank by about 0.35 GB. No other target dir remains.

## Left behind
- Uncommitted: pool_mirror.rs print, as above. Rebuilt test binary in target/ includes it.
- S/baseline/ (3.7 GB), needed by the frame rows.
- target/debug/postretro is not a capture build. The frame rows must build HEAD with `--features capture`.
- No process running; no caffeinate change. Worktree gone: `git worktree list` shows only the main checkout.

## Task 10b results: frame rows 61 and 63

HEAD 3af5af001 with `--features postretro/capture` against 6a813bdcd. AMD Radeon Pro 5300M, Metal, headless capture, 960x540, fov 100, every light term on. New PRLs from content/dev/maps/ (cold, f22f15152), old PRLs from baseline/.

Review folder: S/frames/index.html (old, new streamed, new all-resident, diff, mask reach, per pose).

## Result
**Row 61: pass on the rule as written, with limits below. Every old against new frame is byte-identical on all five maps.** 754 pairs compared, 754 byte-identical, largest channel difference 0, 0 pixels differ. No shot excluded. Nothing past noise, so nothing to chase.

**Row 63: recorded, frames byte-identical.** Figures below.

| Map | Poses | Noise floor (same side twice) | Largest old vs new, streamed | Largest old vs new, all-resident | Poses reading mask data |
|---|---|---|---|---|---|
| stress-warren-hallway-inspection | 23 | 0 (old and new) | 0 | 0 | 18 |
| campaign-test | 14 | 0 | 0 | 0 | 14 |
| occlusion-test | 10 | 0 | 0 | 0 | **0** |
| specular-shadowmask-capture | 5 | 0 | 0, all-resident map | 0 | 5 |
| shadowmask-groups-capture | 6 | 0 | 0, all-resident map | 0 | 6 |

## Method
Run per the brief. The new side also refuses capture without `POSTRETRO_SH_STREAMING=sync-proof` (checked), so both sides set it. All-resident is `POSTRETRO_LIGHTMAP_STREAMING=all-resident`. Streamed is unset, with no size override, so the new side sits at its shipped first generation.

Materials: both sides load 16 textures on the hallway (info log), and frames show real textures. The `[Material] Unknown prefix` warnings are identical on both sides and are material-property defaults, not placeholders.

Shots per pose:
- `full`, t = 2/60 s (measurement warmup 1, sample 1): old, old again, new, new again, old all-resident, new all-resident.
- `t0`, no measurement, t = 0: old, new.
- `t30`, t = 30/60 s: old, new, new all-resident.
- `spec`, term mask 0x40: old, new, old all-resident, new all-resident.
- `nospec`, term mask 0x13F: new only, for the reach figure.

Pairs by kind: noise 116, old vs new 232 (58 poses x 4 variants), old all-resident vs new all-resident 116, new streamed vs new all-resident 174, old streamed vs old all-resident 116.

Clock: capture sets time itself. No measurement renders t = 0; measurement steps 1/60 s per frame, the same on both sides. So every pair is at equal clock with no pin needed. Clock is live in some shots: frames change between clocks on hallway spawn-yaw180 and door3 (up to 110,644 px), campaign spawn-yaw270, and six occlusion-test poses. Old and new still match at each clock.

Movers: capture has no VM and draws movers at their authored rest pose, so no mover moves between runs. **Excluded shots: none.** The clock-sensitive pixels are animated lighting, not movers.

Poses: chosen from map entities, Build Summary demand peaks, existing test poses, the sh-compose arena pose, and id 17/19/42 parsed to aim at faces with owners, plane-B owners first. Pose lists: S/frames/tools/poses.json; scenes: S/frames/scenes/. Pilot was one pose per map on old, new, new all-resident, all valid, before the batch.

## Is the diff able to see a mask error? Yes, except on occlusion-test
A zero diff means little if the frame does not depend on the data, so three controls ran on copies of the new PRLs, against the real new frame.

**1. All stored mask bytes zeroed** (records unchanged). Pixels that change really read mask data. Per-pose figures below. On the hallway 18 of 23 poses react, up to 453,537 px and 24 levels. Two of those are barely sensitive: peak-yaw0 22 px, arena 13 px.

**2. Plane B alone zeroed.** Groups map 6 of 6 poses, up to 451,773 px. Campaign 4 poses (spawn-yaw270 21,402 px, spawn-yaw90 222, b67 3,315, b71 132,829). Hallway: b1049 166,805 px, b1055 144,946 px at 960x540, and pb1382 2,094 px, pb1356 789 px at 480x270. So plane B reaches frames from 4 of the hallway's 7 two-plane blocks. Blocks 852, 842 and 728 did not react from any pose tried, 2 sides of each of their faces.

**3. Planes A and B swapped** on shadowmask-groups-capture (blob bytes swapped, records kept). 4 of 6 poses change, max 34, up to 447,503 px. b9 and mid do not react to the swap, though they react to control 2. Images are at the top of the index.

**Id 42 byte check** (tools/planes.py), which covers what frames cannot, every plane of every block, old against new:

| Map | Blocks | Planes | Present A/B | Absent | Blocks both/one/neither | Present plane bytes differ | Nonzero old plane dropped | Zero old plane kept | Owner table equal |
|---|---|---|---|---|---|---|---|---|---|
| hallway | 1,840 | 3,680 | 689/7 | 2,984 | 7/682/1,151 | 0 | 0 | 0 | yes |
| campaign-test | 137 | 274 | 68/10 | 196 | 10/58/69 | 0 | 0 | 0 | yes |
| occlusion-test | 117 | 234 | 22/0 | 212 | 0/22/95 | 0 | 0 | 0 | yes |
| specular-shadowmask-capture | 1 | 2 | 1/0 | 1 | 0/1/0 | 0 | 0 | 0 | yes |
| shadowmask-groups-capture | 1 | 2 | 1/1 | 0 | 1/0/0 | 0 | 0 | 0 | yes |

## Limits the owner should weigh
- **occlusion-test frames cannot see masks.** Zeroing every mask byte changes 0 pixels at all 10 poses. Its DebugTextures have no `_s.png`, so owner specular draws nothing, and a scan of the largest owned face in all 22 mask blocks found none either. The row passes there for lightmap frames only; its masks rest on the byte check. A first scan misread this: flat lilac pixels in term-mask frames looked like specular and are clear colour, seen from past a wall.
- **No frame follows a repack or compaction.** Capture runs one preload drain: repacks are 0 on every shot. Moved-texel integrity stays with the automated rows. Three off-portal hallway poses do follow a growth (below). The windowed route, where repacks really happen, was not used for frames; row 61 asks for fixed poses.
- **Promoted-light union subtraction was not forced.** No `force_promotion` shots; the automated sampling rows hold it.
- **Off-portal poses.** Hallway b1356, b1382, pb1382, pb1356, outside-b728 and occlusion-test spec-b6-east, spec-b6-west, spec-b7, spec-b8, outside-a9 have the camera past a wall or in a solid cell (several log `SolidCellFallback`), so they draw blocks outside the baked set. Not player poses. Kept because they read masks or stress placement. On campaign-test and the two all-resident maps every pose is in play space.
- One adapter. Byte-identical is same-adapter only.

## What the streamed path showed
Every new streamed shot: 0 refusals, 0 deferrals, 0 failed installs, 0 drawn blocks not resident.

- **Hallway, in play space: new pools stay 34/15, growths 0 at all 18 in-play poses, including the demand peak (248 blocks resident).** The old build grew at 6 of those 18 (peak-yaw0, peak-yaw180, arena, door3, b852, b842), to 36 layers and 518 MiB at the peak, against 274 MiB new.
- **Growth, off-portal only:** outside-b728 grew to 137/50 (1,084 blocks), pb1382 to 125/43, pb1356 to 79/28. Each pool grew once and warned by name with the camera cell, as the Decision says. These three frames are byte-identical to old and to all-resident, so sampling from grown pools holds. b1356 and b1382 are also off-portal and stayed at 34/15.
- campaign-test 13/10 against old 15. occlusion-test 7/3 against old 9. No growth on either side.

| Map | Lightmap-family total, old streamed | New streamed | Old all-resident | New all-resident |
|---|---|---|---|---|
| hallway at spawn | 335,544,320 | 387,973,120 | 3,330,277,376 | 1,507,852,288 |
| hallway at the demand peak | 643,825,664, after growth | 387,973,120 | same as above | same as above |
| campaign-test | 335,544,320 | 234,881,024 | 320,864,256 | 224,395,264 |
| occlusion-test | 197,132,288 | 117,440,512 | 182,452,224 | 106,954,752 |

Totals include the animated atlas. The new hallway total is higher at spawn because new pools start at their full size and old started small and grew: 388 MB flat against 336 to 644 MB. Per-shot reports: S/frames/raw/<map>/<pose>/full.*.report.json.

## Row 63: shadowmask-groups-capture
No usable portals, so all-resident, no id 58, 1 block, 24 faces, 21 owned, 19 with an owner in plane B.

| Figure | Old | New |
|---|---|---|
| Lightmap pool | 1 layer, irradiance 4,194,304 B, direction 2,097,152 B | same |
| Mask pool | shared layer, one 4096x2048 BC5 layer, 8,388,608 B | 2 layers of 2048x2048 BC5, 8,388,608 B |
| Lightmap-family total | 14,680,076 B | 14,680,076 B |
| Build Summary | n/a | 14.0 MiB, 1 lightmap and 2 shadow mask layers |
| Planes | 2 stored | 2 stored, **0 absent of 2**, 2,831,088 B each |
| PRL | 12,304,762 B | 12,304,762 B |

It costs exactly what it cost, as the Decision says, and is the only map with no absent plane (others: specular 1 of 2, hallway 2,984 of 3,680, campaign 196 of 274, occlusion 212 of 234). Frames: 6 poses, all pairs byte-identical, all 6 react to zeroed masks and to zeroed plane B. Compare specular-shadowmask-capture, whose new mask pool is 4,194,304 B, total 10,485,772 B, against 14,680,076 B old.

## Per pose

### stress-warren-hallway-inspection

| Pose | Camera (pos; yaw, pitch) | Mode new | Resident new | Pools new L/M | Layers old | Growth new L/M, old | Mask-dependent px, max | Plane-B-dependent px, max | Clock changes frame |
|---|---|---|---|---|---|---|---|---|---|
| spawn-yaw0 | [42.27, 3.2, 63.4]; 0, 0 | stream | 58/1840 | 34/15 | 15 | 0/0, 0 | 0, 0 | 0, 0 | no |
| spawn-yaw180 | [42.27, 3.2, 63.4]; 180, 0 | stream | 58/1840 | 34/15 | 15 | 0/0, 0 | 0, 0 | 0, 0 | yes |
| peak-yaw0 | [21.1, 4.9, 39.0]; 0, 0 | stream | 248/1840 | 34/15 | 36 | 0/0, 1 | 22, 5 | 0, 0 | no |
| peak-yaw180 | [21.1, 4.9, 39.0]; 180, 0 | stream | 248/1840 | 34/15 | 36 | 0/0, 1 | 129,722, 18 | 0, 0 | no |
| arena | [21.13, 4.04, 30.48]; 0, 0 | stream | 248/1840 | 34/15 | 36 | 0/0, 1 | 13, 3 | 0, 0 | no |
| door3 | [10.6, 22.6, 84.0]; 0, 0 | stream | 157/1840 | 34/15 | 27 | 0/0, 1 | 0, 0 | 0, 0 | yes |
| closet0 | [53.3, 3.2, -100.0]; 0, 0 | stream | 45/1840 | 34/15 | 15 | 0/0, 0 | 0, 0 | 0, 0 | no |
| closet2 | [-73.7, 36.5, 100.0]; 180, 0 | stream | 88/1840 | 34/15 | 15 | 0/0, 0 | 0, 0 | 0, 0 | no |
| b1049 | [-21.71, 5.69, 13.82]; 295, -8 | stream | 63/1840 | 34/15 | 15 | 0/0, 0 | 186,308, 15 | 166,805, 15 | no |
| b1055 | [63.97, 5.69, 13.82]; 115, -8 | stream | 48/1840 | 34/15 | 15 | 0/0, 0 | 152,025, 15 | 144,946, 15 | no |
| b852 | [-63.4, 21.71, -16.66]; 0, -80 | stream | 215/1840 | 34/15 | 30 | 0/0, 1 | 204,121, 14 | 0, 0 | no |
| b842 | [-21.13, 37.96, -16.66]; 0, -80 | stream | 195/1840 | 34/15 | 24 | 0/0, 1 | 171,383, 12 | 0, 0 | no |
| b728 | [62.8, 21.95, -15.44]; 295, -8 | stream | 170/1840 | 34/15 | 15 | 0/0, 0 | 47,575, 9 | 0, 0 | no |
| b1356 | [-17.31, 5.69, 65.84]; 115, -8 | stream | 172/1840 | 34/15 | 28 | 0/0, 1 | 89,464, 24 | 0, 0 | no |
| b1382 | [-59.57, 38.2, 65.02]; 115, -8 | stream | 92/1840 | 34/15 | 15 | 0/0, 0 | 67,121, 17 | 0, 0 | no |
| a1174 | [-94.69, 5.45, 42.27]; 0, -80 | stream | 132/1840 | 34/15 | 15 | 0/0, 0 | 453,537, 22 | 0, 0 | no |
| a52 | [-63.4, 5.45, -111.35]; 0, -80 | stream | 58/1840 | 34/15 | 15 | 0/0, 0 | 384,984, 21 | 0, 0 | no |
| a1481 | [94.69, 21.71, 84.53]; 0, -80 | stream | 72/1840 | 34/15 | 15 | 0/0, 0 | 420,332, 17 | 0, 0 | no |
| a388 | [-24.43, 5.69, -44.94]; 25, -8 | stream | 86/1840 | 34/15 | 15 | 0/0, 0 | 2,032, 2 | 0, 0 | no |
| a1552 | [17.88, 37.96, 115.82]; 0, -80 | stream | 93/1840 | 34/15 | 15 | 0/0, 0 | 251,467, 14 | 0, 0 | no |
| outside-b728 | [67.22, 21.95, -15.44]; 115, -8 | stream | 1084/1840 | 137/50 | 146 | 1/1, 1 | 125,939, 20 | 0, 0 | no |
| pb1382 | [-68.02, 38.2, 65.02]; 270, 0 | stream | 958/1840 | 125/43 | 134 | 1/1, 1 | 54,030, 22 (no-specular diff) | see text | no |
| pb1356 | [-25.76, 5.69, 65.84]; 270, 0 | stream | 589/1840 | 79/28 | 84 | 1/1, 1 | 84,397, 25 (no-specular diff) | see text | no |

### campaign-test

| Pose | Camera (pos; yaw, pitch) | Mode new | Resident new | Pools new L/M | Layers old | Growth new L/M, old | Mask-dependent px, max | Plane-B-dependent px, max | Clock changes frame |
|---|---|---|---|---|---|---|---|---|---|
| spawn-yaw270 | [-65.84, 2.6, -45.92]; 270, 0 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 125,347, 14 | 21,402, 6 | yes |
| spawn-yaw90 | [-65.84, 2.6, -45.92]; 90, 0 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 47,869, 8 | 222, 3 | no |
| peak-yaw0 | [-29.7, 2.6, -46.3]; 0, 0 | stream | 113/137 | 13/10 | 15 | 0/0, 0 | 142,505, 16 | 0, 0 | no |
| peak-yaw180 | [-29.7, 2.6, -46.3]; 180, 0 | stream | 113/137 | 13/10 | 15 | 0/0, 0 | 138,457, 16 | 0, 0 | no |
| b22 | [-63.35, 14.06, -39.42]; 0, 80 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 463,928, 9 | 0, 0 | no |
| b24 | [-62.18, 14.06, -58.93]; 0, 80 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 444,836, 16 | 0, 0 | no |
| b31 | [-70.14, 9.96, -39.42]; 115, -8 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 326,275, 10 | 0, 0 | no |
| b26 | [-57.3, 2.2, -38.91]; 180, -80 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 36,397, 3 | 0, 0 | no |
| b67 | [-44.09, 2.2, -33.83]; 180, -80 | stream | 113/137 | 13/10 | 15 | 0/0, 0 | 20,274, 6 | 3,315, 6 | no |
| b71 | [-44.09, -0.67, -50.16]; 180, -63.4 | stream | 110/137 | 13/10 | 15 | 0/0, 0 | 183,124, 17 | 132,829, 17 | no |
| a25 | [-70.92, 13.21, -29.02]; 205, -8 | stream | 109/137 | 13/10 | 15 | 0/0, 0 | 373,786, 11 | 0, 0 | no |
| a105 | [-38.0, 1.63, 16.02]; 25, -8 | stream | 60/137 | 13/10 | 15 | 0/0, 0 | 161,572, 52 | 0, 0 | no |
| a0 | [-70.92, 13.21, -63.64]; 25, -8 | stream | 71/137 | 13/10 | 15 | 0/0, 0 | 205,107, 7 | 0, 0 | no |
| a134 | [-60.35, 10.8, 36.17]; 0, 80 | stream | 91/137 | 13/10 | 15 | 0/0, 0 | 151,977, 32 | 0, 0 | no |

### occlusion-test

| Pose | Camera (pos; yaw, pitch) | Mode new | Resident new | Pools new L/M | Layers old | Growth new L/M, old | Mask-dependent px, max | Plane-B-dependent px, max | Clock changes frame |
|---|---|---|---|---|---|---|---|---|---|
| spawn-yaw180 | [-23.16, 2.2, -15.04]; 180, 0 | stream | 107/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| peak-yaw0 | [-43.1, 10.0, 6.5]; 0, 0 | stream | 111/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| spec-b6-east | [-38.77, 1.22, -41.25]; 90, 0 | stream | 25/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| spec-b6-west | [-45.77, 1.22, -41.25]; 270, 0 | stream | 31/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| spec-b7 | [-38.77, 1.22, -39.22]; 90, 0 | stream | 30/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | no |
| spec-b8 | [-38.77, 1.22, -43.28]; 90, 0 | stream | 20/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| lit-l0 | [-5.82, 1.2, -4.57]; 90, -10 | stream | 24/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | no |
| lit-l11 | [-42.47, 5.1, -48.36]; 270, -10 | stream | 107/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | no |
| lit-l2 | [-50.39, 1.2, -11.99]; 270, -10 | stream | 110/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | yes |
| outside-a9 | [-46.94, 1.63, -55.44]; 205, -8 | stream | 117/117 | 7/3 | 9 | 0/0, 0 | 0, 0 | - | no |

### specular-shadowmask-capture

| Pose | Camera (pos; yaw, pitch) | Mode new | Resident new | Pools new L/M | Layers old | Growth new L/M, old | Mask-dependent px, max | Plane-B-dependent px, max | Clock changes frame |
|---|---|---|---|---|---|---|---|---|---|
| spawn | [-4.064, 4.064, -1.626]; 45, 0 | all-resident | 1/1 | None/None | None | None/None, None | 412,857, 14 | - | no |
| a2 | [-6.5, 4.88, -2.2]; 205, -8 | all-resident | 1/1 | None/None | None | None/None, None | 486,746, 11 | - | no |
| a10 | [-6.33, 4.88, -10.57]; 115, -8 | all-resident | 1/1 | None/None | None | None/None, None | 431,394, 12 | - | no |
| a19 | [-11.58, 4.88, -23.81]; 25, -8 | all-resident | 1/1 | None/None | None | None/None, None | 449,021, 10 | - | no |
| a15 | [-10.77, 7.55, -17.68]; 0, 80 | all-resident | 1/1 | None/None | None | None/None, None | 448,952, 17 | - | no |

### shadowmask-groups-capture

| Pose | Camera (pos; yaw, pitch) | Mode new | Resident new | Pools new L/M | Layers old | Growth new L/M, old | Mask-dependent px, max | Plane-B-dependent px, max | Clock changes frame |
|---|---|---|---|---|---|---|---|---|---|
| spawn | [-4.064, 4.064, -1.626]; 45, 0 | all-resident | 1/1 | None/None | None | None/None, None | 466,593, 16 | 286,619, 16 | no |
| north-wall | [-1.016, 4.877, -12.7]; 90, 0 | all-resident | 1/1 | None/None | None | None/None, None | 416,608, 15 | 325,230, 15 | no |
| b9 | [-2.2, 4.88, -17.68]; 295, -8 | all-resident | 1/1 | None/None | None | None/None, None | 452,770, 12 | 448,741, 11 | no |
| a23 | [-9.35, 4.88, -13.99]; 205, -8 | all-resident | 1/1 | None/None | None | None/None, None | 479,495, 12 | 209,405, 12 | no |
| a5 | [-9.35, 4.88, -7.15]; 25, -8 | all-resident | 1/1 | None/None | None | None/None, None | 495,342, 15 | 300,432, 15 | no |
| mid | [-6.1, 3.46, -14.63]; 270, 0 | all-resident | 1/1 | None/None | None | None/None, None | 484,022, 12 | 451,773, 12 | no |

"Mask-dependent" is control 1 at 960x540; pb poses have no control-1 run, so theirs is the full against no-specular diff, which equals control 1 on every other pose but campaign spawn-yaw270 (125,347 against 132,406, dynamic specular). pb plane-B figures are in control 2 above. Layers old is after any growth.

## Left behind
- S/frames/: index.html, shots/ (99 MB), scenes/, raw/ (reports and logs only), tools/, diffs.json, summary.json, maskdata.json, planeb.json, manifest.sha256.txt (hash of all 984 PNGs before pruning: 182 distinct, because identical frames share a hash). 184 MB in all.
- Pruned: raw PNGs outside the index, heat images, scan frames, control PRLs, imgdiff target. Tool source stays in S/imgdiff/src/ and S/frames/tools/.
- S/frames/bin/postretro_new is the HEAD capture build. target/debug/postretro is also a capture build now.
- No repo file changed; no process running. S/baseline/ not changed.

## Disk
36 GB at start, low point 35 GB, 35 GB at end (`df` Avail 35Gi).

## Task 10c results: Metal rows 59, 62, 67, forward pass, margin

Mac, Radeon Pro 5300M, Metal, windowed. Surface 2560x1440, scene 1280x720, divisor 2, reflection tier VeryHigh (from `settings.toml`), vsync at 60 Hz.

- Feature branch HEAD 3af5af001, untouched. Hallway PRL sha256 46e0ccda..., checked before and after.
- Throwaway branch `measure/lightmap-bounded-pool-tour`, local only, final sha 8c84543e5. The engine binary was built at 7d835c329 on it (later commits are scripts only): `--profile profiling --features dev-tools`, the profile of the SH Metal runs. Binary kept at S/t10c/bin/postretro-tour.
- The branch holds `crates/postretro/src/frame_loop/measure_tour.rs`, five small hooks in `frame_loop/mod.rs`, the env-gated `POSTRETRO_MEASURE_POOL_LAYERS` first-generation override in `render-cpu/src/lightmap_pool/static_plan.rs`, and `tools/measure-harness/` (`run.sh`, `batch.sh`, `mst.sh`, `cleanup.sh`, `fwd.py`, `fwd_trace.sh`, `analyze.py`, `pair.py`, `mre.py`, `mst_parse.py`, `fwd_parse.py`, `snap.py`). No pool, compaction or lighting behaviour changed. `route()` is verbatim from the SH harness.
- All raw run data: S/t10c/ (`runs/`, `pair.txt`, `m1.mre.txt`, `s1.summary.txt`, `fwd/`).

The route: 2 km at 7 m/s, fixed 1/60 s step, 15 s hold first, no shots, full lighting (the shot runs' `0x6` term mask not used). It reaches waypoint 214 of 1,209. Log volume at `RUST_LOG=info` is about one line per 5 s, so nothing logs on the frame path.

## Run count

| Set | Runs | Valid | Used for |
|---|---|---|---|
| Short pilots: `pilot0`, `pilot-reload`, `pilot-trace` | 3 | 3 | Harness check only, not data |
| Shipped 34/15, untraced: `a1`, `a2`, `a3` | 3 | 3 | Row 62 (`a1` was the pilot run, valid, so kept) |
| 36/16, untraced: `b1`, `b2` | 2 | 2 | Margin |
| 27/12, untraced: `c1` | 1 | 1 | More compactions for row 62; not a shipped size |
| Metal Resource Events: `m1` | 1 | 1 | Row 59. Frame times not used for row 62 |
| Metal System Trace attached over 45 s: `s1` | 1 | 1 | Row 62 GPU and display intervals |
| Cleanup: `r1`, `r2` | 2 | 0 | Discarded, see below |
| Cleanup: `r3`, `r4` | 2 | 2 | Row 67 |
| 4K old against new, completion time | 72 captures | 72 | Forward pass |
| 4K old against new, Metal System Trace | 14 captures | 14 | Forward pass |

**Discarded: 2, neither for thermals or the frame regime.** `r1` and `r2` were killed by my own harness: the stall check watched only `frames.csv`, and a reload cycle writes `frames.N.csv`. Fixed, rerun as `r3`, `r4`.

**The 150 to 260 ms regime never appeared.** Every 30 s window of every run has a median of 16.7 to 16.8 ms. All runs passed "Window ready". No lock, no screen saver, no compile during a run. Temperature gate: GPU at or under 74 to 75 C before a run (it read 62 to 71 C). Peaks: 79 to 83 C untraced, 86 to 89 C under trace and in the reload runs. `pmset` CPU speed limit was read before runs only; the frame medians show no slowdown.

**Harness did not change what it measures.**
- Sampling `ioreg` every 5 s against every 30 s: p99 20.65 and 20.65 against 20.50, frames over 34 ms 41 and 42 against 43. No effect.
- Against the SH brief's shot-free run (max 86 ms, p99 23 ms): `a` runs max 81 to 83 ms, p99 20.5 to 20.7 ms. Frame rows add nothing visible.
- Tracing does perturb: `m1` p99 23.9 ms, max 153 ms. So `m1`'s frame times are not row 62 data.
- Runs are near-deterministic: `a1`, `a3` and `m1` have the same repack and compaction counts, installs and evictions.

## Row 59, Hallway ceiling: under 2 GiB, no lightmap growth. Finding, recorded

Method: `run.sh` with `XCTRACE`, `xctrace record --instrument 'Metal Resource Events' --launch`, table `metal-resource-allocations`, `resource-size` column, allocation rows paired with deallocation rows by id (`mre.py`). One full-route run, valid; a second adds nothing, because memory is flat and the counters repeat, and it would have gone through the disk floor again.

| Figure | `resource-size` MiB | Previous | Change | Video memory only MiB | Previous |
|---|---|---|---|---|---|
| Steady, 15 s to end | **1,962** (1,960 to 1,963) | 2,170 | -208 | 1,793 | 2,009 |
| Peak | **1,974** at 128 s | 2,574 | -600 | 1,798 at load | 2,415 |
| Lightmap pools | **274** = 140 + 70 + 64 | 490 in that walk | -216 | | |

- `research.md` projected 1,954 steady (2,170 - 490 + 274). Measured 1,962, 8 MiB over. That also supports reading `resource-size` as the column the spike's 2,170 came from.
- 2 GiB is 2,048 MiB: steady is 86 under, peak 74 under.
- The peak is not a pool. It is three 4 MiB and three 1 MiB buffers near t = 128 s, coinciding with the 122 s solid-cell fallback on the tour clock (the trace clock runs about 5 s ahead of it). It is gone by the next 15 s sample.
- Lightmap pool textures: allocated once at load, no pool texture allocated after 20 s. No allocation of 1 MiB or more after 20 s except those buffers and two 1 MiB buffers at 51 s.
- **No lightmap growth on any route run at any size:** 0+0 growths, 0 retiring bytes, transient peak 0, layers constant, in `a1-3`, `b1-2`, `c1`, `m1`, `s1`, and in all ten load cycles of `r3`, `r4`.
- `ioreg` in-use during the walk: 2,536 to 2,626 MiB, of which about 720 to 800 is other processes at idle.

## Row 62, Repack cost: no visible hitch. Worst case is one missed vsync, rarely. Owner to confirm

Method: per-frame wall time from redraw entry to next redraw entry, with `LightmapStreamCounters` read each frame; a frame is a repack frame when `repacks` moves, a compaction when `compactions` moves. Each repack is judged over its frame and the next three, because two frames can be in flight. `pair.py` compares it with the same route position in runs with no repack there; the route is frame-stepped, so positions match exactly. Budget at 60 Hz is 16.67 ms.

Frame time on the route, shipped, three runs: p50 16.70 ms, p99 20.5 to 20.7 ms, max 81 to 83 ms, 41 to 43 frames over 34 ms, none over 100 ms.

| Kind, 34/15, three runs | Per run | Per minute | Own frame worst ms | Worst of next 4 frames ms | Change against same spot, no repack: median, worst ms | CPU in drain ms | Copies |
|---|---|---|---|---|---|---|---|
| Downward, lightmap | 7 to 8 | 1.7 | 25.2 | 34.2 | -0.2, +12.9 | up to 3.6 | 150 to 265 |
| Downward, mask | 6 to 10 | 2.1 | 20.6 | 30.0 | +0.3, +2.2 | up to 2.8 | 66 to 151 |
| Downward, both pools | 0 to 1 | 0.1 | 17.9 | 17.9 | no control | 3.9 | 391 |
| Compaction, lightmap | 1 | 0.2 | 16.3 | 18.1 | +0.6, +0.8 | 2.9 to 3.1 | 247 |
| Compaction, mask | 0 | 0 | none | none | none | none | none |

Figures exclude the three repacks next to a solid-cell fallback frame (see below). All kinds together: 15 to 19 repack frames per run, 3.1 to 4.0 per minute, 0.75 to 0.95 per 100 m. Tour rate in the walk harness, 140 per 97,610 frames, is 1.4 per 1,000 frames; the real route is 0.9 to 1.1.

Compactions at no margin, 27/12, one run (11 lightmap and mask compaction frames, 3 combined with a downward pass, up to 422 copies, 6.3 ms CPU): worst own frame 22.6 ms, worst of next 4 22.6 ms, change against same spot median about 0, worst +4.2 ms.

**Worst repack frame.**
- Worst frame near a repack anywhere: 67.2 ms, `a1`, 209 m. Not a repack. The frame sits right after a solid-cell fallback, and the same frames run 51 to 52 ms in runs with no repack at that spot.
- Worst off that fallback: **34.2 ms** at 1,747 m in `a2`, three frames after a 247-copy lightmap repack. `a1`, `a3` do not repeat it there. Other runs at that spot read 21 ms.
- Worst repeatable: **35.3 and 31.8 ms** at 331 m in the two 36/16 runs, on a 173-copy lightmap repack with 4.0 ms of drain CPU. That is one missed vsync. At 34/15 the same spot has two repacks, lightmap then mask, on consecutive frames and reads 18 to 22 ms.
- The 25 to 33 ms frames at 1,247 m are not repacks: they appear in all five runs, including 27/12, where the repack falls after them.

Of 50 shipped off-fallback repacks, 1 went over 34 ms, against 0.7 to 0.9% of random repack-free windows (fallback frames included). One event is too few to separate those rates. So one missed vsync at most, and rarely.

**GPU and on-screen, `s1`** (Metal System Trace attached 45 s across four repacks). GPU per frame p50 9.74 ms, p99 15.21 ms, max 27.4 ms, so about 7 ms of headroom. One repack was cleanly isolated, 256 copies: 5.77 ms of `Streamed Lightmap Drain` blits, frame GPU 12.7 ms against 9.5 ms beside it, inside the budget. The other three could not be isolated: their blits seem to land under `(wgpu internal) Signal` labels. Displayed-surface intervals: 14 of 2,760 at 33.3 ms, none longer, none at a repack in that window. The task 1 figure of 15 to 16 ms of GPU was a bench time, submit to idle; in play this one repack costs about a third of that.

**Reading it.** At 60 Hz on this card a repack hides in the headroom. It does not show as a spike in ordinary frames, and at worst drops one frame. I would not call it a hitch. The real route spikes are elsewhere: solid-cell fallback frames at 50 to 80 ms and streaming frames at 25 to 35 ms. A card or scene with less than about 6 ms of GPU headroom would lose it, and this was measured at the 1280x720 scene only.

## Row 67, Cleanup: exit returns to baseline, reload does not grow. Owner confirms

Read from outside with `ioreg -c IOAccelerator -r -d 1 -a`, `PerformanceStatistics.inUseVidMemoryBytes`, AMD accelerator (`snap.py`).

The engine has a reload path. `restartLevel` and `Alt+Shift+L` use the level request queue. The harness enqueues `LevelRequest::Load(LevelSource::Path(prl))` through that same queue, which unloads and loads. Each run: 30 idle samples, then five loads with 300 m of walking each (1 + 2 repacks per load), `LevelRequest::Unload`, 30 s hold, clean `event_loop.exit()`, 45 idle samples.

| Run | Idle before MiB, median | After exit MiB, median | Level held, in-use MiB at each load's hold | Unloaded, still running |
|---|---|---|---|---|
| `r3` | 723 | 720 | 2,518, 2,552, 2,551, 2,582, 2,524 | 1,393 |
| `r4` | 732 | 720 | 2,501, 2,523, 2,524, 2,524, 2,523 | 1,449 |

- **Exit:** back to baseline, within idle noise (idle samples range 718 to 888). Same after the full-route runs: `m1` 767 before, 741 to 845 in the 12 s after.
- **Reload:** no trend over five loads. `r4` is flat within 1 MiB after the first load. `r3`'s 30 to 60 MiB steps fall back on the last load, and they do not line up with engine state: pools were 34/15, 274 MiB, 0 growths in every cycle.
- Reload took 3.3 s; `unload_level` 44 to 60 ms.
- **For the owner, not a failure:** unloaded, the running engine still holds about 670 to 730 MiB over baseline, all gone at exit. Part of it is the shadow pools (427 MiB), which are allocated before the level. I did not itemise the rest.

## Forward pass: about +1%, no concern

Both binaries are dev profile with `capture` (`baseline/bin/postretro`, `frames/bin/postretro_new`), so profiles match and no new build was needed. Each uses its own PRLs at matched poses. `POSTRETRO_GPU_TIMING` reports unsupported on this adapter, so two methods: `cpu_completion` (the capture waits on the GPU after each submit), and Metal System Trace on the `Textured Pass` label. Capture at 3840x2160 to pin the clock (each capture peaked at 1,048 MHz or higher), 30 warmup frames, 120 sampled, old and new alternated.

Whole frame, 4K, 6 pairs per pose, median of medians:

| Pose | Old ms | New ms | Change |
|---|---|---|---|
| hallway spawn-yaw0 | 29.74 | 29.90 | +0.16, +0.5% |
| hallway peak-yaw180 | 32.18 | 32.78 | +0.60, +1.9% |
| hallway b1049 | 28.74 | 29.04 | +0.29, +1.0% |
| hallway a1174 | 26.86 | 27.14 | +0.28, +1.0% |
| `shadowmask-groups-capture` a23 | 29.43 | 29.70 | +0.26, +0.9% |
| `campaign-test` a0 | 27.11 | 27.39 | +0.27, +1.0% |

`Textured Pass` alone, per pass, 4K:

| Pose | Old ms | New ms | Change |
|---|---|---|---|
| hallway peak-yaw180, 3+3 | 22.68, 21.20, 22.05 | 21.90, 21.29, 21.09 | -0.55, within spread |
| hallway b1049, 2+2 | 21.38, 21.34 | 21.57, 21.65 | +0.25, +1.2% |
| groups a23, 2+2 | 22.94, 22.75 | 23.10, 23.06 | +0.23, +1.0% |

The new build is consistently about 1% slower at 4K, about 0.25 ms, with one pose's trace inconclusive. The comparison is whole-binary, so it does not separate the flat varying from the mask decode. If it is fragment cost it scales to about 0.03 ms at the 1280x720 scene, where the forward pass is about 5.6 ms of a 9.7 ms GPU frame. Nothing to act on.

## Margin, for the owner: 36/16 against 34/15

Same route, `POSTRETRO_MEASURE_POOL_LAYERS`. Two 36/16 runs were identical in every count, so the spread below is `a` runs only.

| Figure | 34/15, 3 runs | 36/16, 2 runs |
|---|---|---|
| Pool MiB | 274 | 290 |
| Repacks L+M | 9+7 to 9+10 | 3+3 |
| Compactions | 1 | 0 |
| Installs | 4,260 | 4,196 |
| Refusals | 115 to 150 | 97 |
| p99 ms | 20.5 to 20.7 | 20.6 to 20.7 |
| Frames over 34 ms | 41 to 43 | 40 to 43 |
| Max frame ms, a fallback frame | 81 to 83 | 85 to 94 |
| Worst repack frame, off fallback | 34.2 | 35.3 |

36/16 removes two thirds of repacks, as the walks said, but buys nothing a player would see on this route. Repacks are too cheap at 60 Hz for frame time to separate the two. On frame time alone 34/15 stands; the case for 36/16 is only fewer events and reads. This is one route on one card.

## Other things the owner should know

- **Drawn not resident on the real route: 20,723 to 20,862 block-frames, against 0 in the no-GPU walks.** In `a1`, every one is on 39 `SolidCellFallback` frames (44 fallback frames on the route, up to 999 cells drawn), where the tour's straight lines enter solid cells, a limit `done/.../research.md` already states. On 17,994 portal frames: 0 not resident, 24 outside the set. Not a false row: the walks never enter those cells. Whether those fallback frames look wrong was not looked at, and it is not compared with the old build. It is why the route's worst frames are there.
- SH misses 579, unchanged from the SH brief's hallway figure. SH growths 0.
- `s1` shows `Page On 65536 bytes` for the engine on SDMA0 at 2,200 to 3,000 per second, 137 to 190 MiB/s, steady across the 45 s, and `vramFreeBytes` falls to 130 to 370 MB during play. No frame effect seen, cause not chased, out of this brief's scope. It is the "per-frame paging" case in `rendering_pipeline.md` §12, and may matter to a real 2 GB card, which was not tested.
- An attached windowed Metal System Trace took 46 minutes to save a 45 s recording (4.8 GB ktrace). `--launch` capture traces saved in under a minute.

## Disk

| Point | Free GB |
|---|---|
| Start | 35 |
| After freeing `target/debug/incremental`, `cargo clean -p postretro -p postretro-level-compiler`, and the worktree target | 41 |
| **Low point, end of `m1`, with 19.7 GB ktrace and trace on disk** | **14** |
| End | 42 |

**The 15 GB floor was crossed once, to 14 GB, for a few minutes after `m1` finished.** 41 GB was above the brief's 40 GB line, and the run still left 27 GB of use. Raw data was deleted at once. That is why there is one Metal Resource Events run, not two. `df -g` rounds down, so the true figure is between 14 and 15.

## Left behind

- Local branch `measure/lightmap-bounded-pool-tour` at 8c84543e5, not pushed. Worktree removed; its target was deleted before the runs. Delete the branch at landing with the two `measure/recovered-*` refs.
- S/t10c/ (188 MB: binary, run CSVs, reduced traces). No raw traces, no ktrace.
- Main `target/` lost `debug/incremental` and the `postretro` and `prl-build` artifacts, so the next dev build there is colder. `target/debug/postretro` is gone.
- Feature branch unchanged and clean. `S/baseline/`, `S/frames/`, `.build-caches` untouched (binaries and scene poses read only).

