# world-poses

Durable tooling behind Measured rows M1, M2 and M5–M7: world-space pose record and replay, the ray-grid guard, the bake-gap guard, and structure counts. It pairs the before and after builds by world pose only, never by cell id. The code is `crates/postretro/src/session/lightmap_residency/walk_measurement/world_poses/`, whose header comment holds the env-var reference. This file pins how the brief runs it.

## Why ignored tests in the `postretro` bin

The bake-gap read must drive the real lightmap session, pool mirror and completed-reads gate. Those are `walk_measurement` internals, test-only code in the `postretro` binary. The other rows share its PRL load, walk camera and record conventions, so all five entries live beside it as `#[ignore]` tests, as `lightmap_residency_walks_from_prl` and `candidate_cull_probes` do. Nothing outside `#[cfg(test)]` changed, and nothing in `crates/visibility`, `crates/level-format` or `crates/renderer` (F3). The shared harness gained two things: `FrameOutcome` now reports deferred and left-behind texel area, and its session setup is factored into `Rig`, so the walks and the replay build their runs the same way.

## The five maps

Bake with identical flags before and after: the guards compare bakes, not configurations. prl-build's warm cache gives an approximate SH bake, which shifts no cell, portal or lightmap block. Use `--no-cache` on both sides or on neither.

| Map | Bake (from the workspace root) | Notes |
|---|---|---|
| occlusion-test | `cargo run -p postretro-level-compiler --bin prl-build -- content/dev/maps/occlusion-test.map -o content/dev/maps/occlusion-test.prl` | 45 s warm on this Mac. |
| stress-warren-hallway-inspection | `cargo run -p postretro-level-compiler --release --bin prl-build -- content/dev/maps/stress-warren-hallway-inspection.map -o content/dev/maps/stress-warren-hallway-inspection.prl` | Default SH spacing is 1.0 m, and the precedent bake at that spacing took 11.15 h (`context/research/gi-rtx-lite/index.md`). Run it unattended, or pass a coarser `--sh-probe-spacing` on both sides: SH spacing moves no cell, portal or lightmap block, only the probe count. |
| campaign-test | `cargo run -p postretro-level-compiler --release --bin prl-build -- content/dev/maps/campaign-test.map -o content/dev/maps/campaign-test.prl` | |
| arena: stress-warren-showcase | `cargo run -p postretro-level-compiler --release --bin prl-build -- content/dev/maps/stress-warren-showcase.map -o content/dev/maps/stress-warren-showcase.prl --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache` | Flags from `content/dev/maps/stress-warren.README.md`. Bake the committed `.map`: today's `gen_stress_map.py --preset warren` output differs from it. |
| street: stress-street | `cargo run -p postretro-level-compiler --release --bin prl-build -- content/dev/maps/stress-street.map -o content/dev/maps/stress-street.prl --sh-probe-spacing 4.0 --lightmap-density 0.25` | Flags from `content/dev/maps/stress-street.README.md`. The map is Task 8's. If it lands after Task 5 is wired, bake its before PRL from a pre-Task-5 commit, where prl-build output equals main. |

Per plan.md's Baselines note, before-bakes come from this branch before Task 5. Keep each before PRL, or at least its records, beside the pose file.

## Commands

All commands run from the workspace root. `R` is a records directory outside the repo, for example `$TMPDIR/ofp`, and `M` is the map name.

**1. Record poses, once, in the before build.**

```bash
POSTRETRO_WORLD_POSES_PRL=$PWD/content/dev/maps/$M.prl \
POSTRETRO_WORLD_POSES_FILE=$R/$M.poses \
  cargo test -p postretro --bin postretro world_poses_record -- --ignored --nocapture
```

For occlusion-test, also add `POSTRETRO_WORLD_POSES_IMPORT=$PWD/context/plans/done/occluder-first-partition/measurement/frame-cost/poses.txt`, which appends the nine live frame-cost poses as `fc:<label>`. Their y is the pawn's capsule centre, so the eye is y + 0.5.

**2. Ray-grid sweep, M1 and M6.** In the before build, write the before record:

```bash
POSTRETRO_WORLD_POSES_PRL=$PWD/content/dev/maps/$M.prl \
POSTRETRO_WORLD_POSES_FILE=$R/$M.poses \
POSTRETRO_WORLD_POSES_OUT=$R/$M.before.sweep \
  cargo test -p postretro --bin postretro world_poses_ray_sweep -- --ignored --nocapture
```

In the after build, use the same pose file and add `POSTRETRO_WORLD_POSES_BEFORE=$R/$M.before.sweep` and `POSTRETRO_WORLD_POSES_OUT=$R/$M.after.sweep`.

**3. Bake gap, M2.** In the before build, record:

```bash
POSTRETRO_WORLD_POSES_PRL=$PWD/content/dev/maps/$M.prl \
POSTRETRO_WORLD_POSES_FILE=$R/$M.poses \
POSTRETRO_WORLD_POSES_OUT=$R/$M.before.bakegap \
  cargo test -p postretro --bin postretro world_poses_bake_gap -- --ignored --nocapture
```

In the after build, replay the before build's frames. FILE is not read when replaying:

```bash
POSTRETRO_WORLD_POSES_PRL=$PWD/content/dev/maps/$M.prl \
POSTRETRO_WORLD_POSES_BEFORE=$R/$M.before.bakegap \
POSTRETRO_WORLD_POSES_OUT=$R/$M.after.bakegap \
  cargo test -p postretro --bin postretro world_poses_bake_gap -- --ignored --nocapture
```

A level that does not stream its lightmap prints that the guard does not apply, and passes.

**4. Diff two records offline.** This takes either two sweep records or two bake-gap records, and loads no PRL:

```bash
POSTRETRO_WORLD_POSES_BEFORE=$R/$M.before.sweep POSTRETRO_WORLD_POSES_AFTER=$R/$M.after.sweep \
  cargo test -p postretro --bin postretro world_poses_diff -- --ignored --nocapture
```

**5. Structure counts, M5, and an arena region, M7.**

```bash
POSTRETRO_WORLD_POSES_PRL=$PWD/content/dev/maps/$M.prl \
  cargo test -p postretro --bin postretro world_poses_structure -- --ignored --nocapture
```

Add `POSTRETRO_WORLD_POSES_REGION=x0,y0,z0,x1,y1,z1`, in engine metres, to list the interior cells and walkable portals inside a box, with portals grouped by plane. On a stair, riser boundaries show as vertical planes 0.7112 m apart (28 u treads) and tread boundaries as horizontal planes 0.3048 m apart (12 u risers). Engine metres from Quake units: x = −y_q · 0.0254, y = z_q · 0.0254, z = −x_q · 0.0254. The regions below come from the committed `stress-warren-showcase.map` brushes:

- Arena interior, inside the walls, floor top to ceiling underside: `-45.5168,1.6256,-61.7728,13.0048,17.8816,61.7728`.
- Arena staircase, the 32 step brushes: `6.5024,1.6256,35.7632,13.0048,11.3792,58.5216`.

## What each row reads

- **Poses.** A column grid over the interior cells' xz footprint. Spacing is the larger of 1 m and √(area / 600), rounded up to 0.25 m. Each column holds every floor a downward ray meets with interior space 5 cm above it and solid 5 cm below. The eye stands 1.5 m above the floor, is located interior, has no geometry between floor and eye, and has none within the 0.2 m capsule radius horizontally. Each floor gets four poses, at yaw 0, 90, 180 and 270 and pitch 0. The pose file is the pin: floats are written shortest-exact, and replay checks them bit for bit.
- **M1.** The camera is the walk's own: `render_camera`, 100° HFOV at 16:9, near 0.1 m, far 4096 m, no blocked portals (doors open, matching the static mesh the rays hit). The grid is 160×90 rays through pixel centres, unless `POSTRETRO_WORLD_POSES_RAYS=WxH` says otherwise. Rays start on the near plane and are cast with `postretro_physics::collision::cast_ray` against the static world trimesh. Each hit is pulled 1 cm back toward the eye, located in this build's partition, and held to the walk's reach (`fog_reachable`). A miss is one of three kinds:
  - **drawn:** fails when new.
  - **culled:** the ray entered the unreached cell across a boundary between interior cells that no portal spans, which is an opening the unchanged portal-area cull removed. Reported, per the M1 row.
  - **faceless:** the cell has no faces, so nothing of it draws. Reported.

  A step-cap trip fails. Rays run only on portal-path poses. Paired, a miss is new unless the same ray, by grid index at the same pose, missed before.
- **M6 per pose.** Each pose reports:
  - portals tested and accepted;
  - drawn and reached cells, and cells a ray saw;
  - faces, and triangles in drawn cells' BVH leaves, all and frustum-kept;
  - drawn lightmap blocks and their texel area;
  - SH clusters of drawn cells;
  - active fog volumes;
  - drawn cells outside the camera cell's id-51 entries, with the camera cell's size.

  Lights admitted by the chunk light lists at the eye are not read.
- **M2.** Three sequences run in the completed-reads mode at the default levers (L 16 m, cap 15): the residency walks' random walk and far-point tour (2000 steps, `WALK_SEED`, frames materialised exactly as `lightmap_residency_walks_from_prl` runs them), plus a tour of the pose file holding each pose for 2 frames. The record holds each frame's eye, yaw and pitch in radians, shortest-exact. The replay sets the camera cell to wherever the eye locates. The metric is the deferred blocks' texel area (width × height from the id-22 index), summed per sequence. It fails when after > before, which with before = 0 means any deferral after. Reported beside it: deferred block counts, left-behind (unadmitted) texel area, outside-baked-set blocks, trip frames, and frames whose deferral grew.
- **M5.** Structure counts are read from the PRL:
  - cells by kind, with interior AABB volume, smallest-extent and largest-extent spreads;
  - portals, walkable or not, with area, aspect and normal histograms;
  - faces, triangles and BVH leaves;
  - lightmap blocks, the cells holding them, and texel area;
  - SH probe grid;
  - SH clusters and indivisible-oversize clusters;
  - cell-visibility components and coupled pairs;
  - id-51 entries per camera cell;
  - lights and fog volumes.

  Classifier output, chart counts, chunk light-list drops, id-51 cells without an eye point, leak and door-sealing lines (M3, M4) are prl-build log lines. Capture them with `RUST_LOG=info` on each bake.

## Pinning (testing_guide §Resource bounds)

- **Fixture.** Each run prints `bake read: id51=… walk_epoch=… size=… mtime=… prl=…`, which identifies the bake. Records carry the same line.
- **Inputs.** The pose file. The bake-gap before record's frames are the after run's inputs.
- **Machine class.** Everything here is deterministic CPU work, and no timing is a metric. Any machine gives the same counts.
- **Worker count.** The sweep spreads poses over all cores (16 here) and writes rows in pose order, so results do not depend on the core count. The bake-gap run is single-threaded.
- **Cache mode.** As the bake table says.
- **Metric.** M1: new drawn misses and trips, which must both be 0. M2: deferred texel area summed per sequence, after ≤ before.
- **Baseline.** The before records.
- **Cleanup.** Records go in `$R`, outside the repo. Delete the before and after PRLs once their records are kept.
- **Upstream limits.** The walk's 5,000-test step cap (a trip fails M1). `MAX_DWELL_FRAMES` 60 per walk step. The completed-reads gate's 120 s read timeout. No measurement override changes the workload: rays and replay read the bake as loaded.

## Validation on today's build (8a9a34177 plus this tooling)

**occlusion-test**, fresh warm bake (the PRL predated the Cornell-room map commits):

- Structure: 555 cells (287 interior, 171 solid, 97 exterior, 287 drawable); 960 portals (693 walkable); 1,018 faces, 2,036 triangles; 287 lightmap blocks, one per interior cell, 30,976,240 texels, streaming; SH grid 71×15×68 = 72,420 probes; 197 SH clusters, none oversize; 173 cell-visibility components, 6,601 coupled pairs; id 51 has 81,199 entries (p50 283 per interior camera cell); 9 lights; 7 fog volumes.
- Poses: 3 m grid, 24×23 columns, 302 floors, 1,208 grid poses plus 9 frame-cost poses, 1,217 in all.
- M1: PASS. 0 trips, 0 misses of any kind, 0 hits locating solid. Portals tested per pose: mean 464, p95 1,749, max 2,084, against the 5,000 cap. Drawn cells: mean 66.6, max 256. Seen cells: mean 43.8. Sweep time 1.4 s.
- M2: 54,370 random-walk frames, 70,432 far-point frames and 2,434 pose-tour frames. 0 deferred blocks and 0 texels in each. The pool never grows on this map: 10 layers, 0 growths. These frame counts and zeros match `lightmap_residency_walks_from_prl` with completed reads. Replaying the record on the same bake reproduces it byte for byte, and the paired verdict is PASS.

**stress-warren-hallway-inspection**, existing bake of 2026-10-14, run only to exercise the tooling (not a baseline):

- Poses: 9 m grid, 1,149 floors, 4,596 poses. Sweep time 6.1 s.
- M1: PASS. Reported misses: 2 faceless poses, 3 rays each into a 0.08 × 0.41 × 0.18 m sliver cell, and 1 culled pose, 1 ray into a 0.16 × 0.2 m sliver whose only opening falls under the 0.1 m² cull.
- M2, unpaired: 93,837 random-walk frames, 95,392 far-point frames and 9,192 pose-tour frames, all with 0 deferred texels. Left-behind (unadmitted) texel area was 335,650,832, 2,659,943,136 and 22,386,057,440, which exercises the texel tally. The run took 231 s.
