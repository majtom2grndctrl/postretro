# occluder-first-partition: Task 9h behind-the-wall leak sweep (C18)

Recorded 2026-10-28 on `claude/occluder-first-partition` at HEAD `32d740dcb` (working tree: the new test module only). This task tests the direction review's claim: a box model of the occlusion-test hall leaked 2.7% on average and 40% at its worst pose.

## Tool

Both rows run one harness, `crates/level-compiler/src/pipeline/runtime_cells/leak_tests.rs`, a test module of the `prl-build` bin.
- **Walk.** `postretro_visibility::determine_visible_cells`, the engine's camera entry, with the engine camera's projection: 100° HFOV at 16:9, near 0.1 m, far 4096 m, look-at from yaw with pitch 0. No portal is blocked. occlusion-test's one mover is in the east annex, away from the hall.
- **Leak.** The fraction of hidden samples whose cell, from `LevelWorld::locate_cell`, is in the walk's visible set. The tool reports the mean over poses and the worst pose.
- **Hiddenness.** Each lattice point inside a brush is dropped, by a plane test against the parsed `BrushVolume`s. Each remaining point gets a segment from every open-space eye, tested against every PRL triangle (BVH, Möller–Trumbore, 1 mm end slack). A point any eye reaches unblocked is dropped and listed.
- **Cells.** Each leaking cell and each cell holding hidden samples is listed with its Quake bounds. Its bounding planes come from the locator: the path down to the deepest node common to every leaf naming the cell, filtered to the planes the cell's box touches, in descent order. Each plane is labelled with every brush that has a face on it, and with `grid768` or `grid1024` when it lies on a cap world-grid line.

Commands, from the workspace root (the test runs in the crate directory, so PRL paths are absolute):

```
T=$PWD/target/occluder-9g
POSTRETRO_LEAK_SWEEP_PRL=$T/before/occlusion-test/occlusion-test.prl,$T/cap19/occlusion-test/occlusion-test.prl,$T/cap26/occlusion-test/occlusion-test.prl \
  cargo test --release -p postretro-level-compiler --bin prl-build behind_wall_leak_sweep -- --ignored --nocapture > sweep.out
cargo test --release -p postretro-level-compiler --bin prl-build walk_from_in_front_of_a_laned_wall -- --ignored --nocapture > fixture.out
```

Outputs:
- `sweep.out`: the map sweep.
- `fixture.out`: the fixture row, which fails by design (see below).
- `variants.out`: the in-process variants behind §Mechanism. They ran on `Cells::of` plus the loader's visibility-only world, as temporary tests on the pre-nudge lattice, and were removed afterwards.

## Setup

Verified against `content/dev/maps/occlusion-test.map`:
- The hall is x −512..1536, y 672..2720, z 0..512.
- Wall b37 spans [0,1184,0]..[1024,1216,256].
- b102 spans x −400..−300, y 1984..2984, at full height.
- The yellow room is b113..b125, at x −228..220, y 2056..2288, z 0..144.

Engine coordinates are (−qy, qz, −qx) × 0.0254 m.

**Eyes.** x {64, 300, 544, 800, 980} × y {700, 900, 1100}, at z 59 u (1.5 m), with yaws {45, 70, 90, 110, 135}° counter-clockwise from Quake +X. **Deviation:** eye (800, 900) stands inside blue pillar b106 (x 788..852, y 872..936, z 0..96), so the walk takes its solid-cell fallback there and draws every cell. It is dropped, leaving 70 poses. The review's box model has no blue pillars.

**Samples.** A 32 u lattice of cell centres, nudged 0.5 u so that no point lies on a whole-unit brush face. It covers x [0,1024], y [1216,2720] and z [0,256], 32 × 47 × 8 = 12,032 points, against the review's 6,825.
- 494 points are inside brushes: red pillars, blue pillars b108–b110 and yellow-room walls. These are not counted.
- 0 points are seen by any eye on any build, so none were dropped.
- That leaves **11,538 hidden samples**.

Every eye stands within the wall's x span and below its top, so the samples are hidden by construction, and the ray check agrees.

HEAD classifies b37 and b102 as occluders. In the yellow room, b113 (roof), b120, b124 and b125 occlude, and the rest are exempt.

## Results: occlusion-test.map

| Build | Cells / portals | Mean leak | Worst pose | Worst leak | Leaking cells |
|---|---|---|---|---|---|
| before | 555 / 960 | **39.36%** | eye (64, 700), yaw 90° | 98.83% (11,403) | 210 |
| cap19 | 505 / 291 | **0.00%** | none | 0 | 0 |
| cap26 (HEAD) | 484 / 241 | **0.00%** | none | 0 | 0 |

No pose on any build took a non-portal path.

**before.** This is C1's defect at full scale. 186 of the 210 leaking cells reach above the wall top, and 31 reach into a lane. Exempt brushes' planes bound them:
- red pillars: x 408, 648, 688; y 1832, 1872, 1912;
- the yellow room: x −228 and −212; y 2056 and 2288; z 128 and 144.

They include column cells running from floor to ceiling over the wall top, such as cell 51 at [648,1832,0]..[688,2720,512]. They also include slabs crossing a lane, such as cell 223 at [−228,2056,144]..[408,2720,512].

**cap19 and cap26.** Nothing leaks. The space behind the wall is bounded on every open side by b37's own planes:
- the west end, x=0, which b2/b6/b7 share and which is also a cap grid line in both builds;
- the east end, x=1024, a face of b37 alone, and a grid line only in cap26;
- the top, z=256;
- the back face, y=1216.

Every eye stands inside both end planes and below the top, so the walk's facing rule refuses every portal into that space. Inside it, cells are cut by the yellow room's planes (x 204, 212, 220; y 2056, 2072, 2272, 2288), by z=128 and z=144, and by grid lines. Those planes lie wholly inside the wall's shadow, and none of them crosses a lane boundary.

## Mechanism

The review's mechanism is that another occluder's extended plane crosses a lane before the wall's end plane bounds the space. That is **not what happens** on either the map or the fixture.

**Map.** The full locator path to the low cell behind the wall descends z=0, z=512, y=672, z=256, x=1536, z=128, x=−512, x=−212 (yellow room), x=1024 (b37's end), x=220 (yellow room), and then y=1216 (b37's back). Other occluders' planes (z=128, x=−212, x=220) cut the hall first. The slab they leave is narrower than it is deep, which fits b37's end plane winning the cross-section credit over its back face, as the variants below show for the fixture.

The nil leak is robust, measured in process at HEAD against the cap26 PRL's 484 cells (`variants.out`):
- **Whole map shifted** by −256 to +512 u: 0.00%.
- **b37 alone moved** by −256 to +256 u: 0.00%.
- **Brush groups removed** (b102, the yellow room, red pillars, blue pillars, and all of them together): 0.00%.

**Fixture.** The leak is independent of the second occluder. With the brush-102 stand-in removed, the leak is 29.19% mean and 88.83% worst, against 27.97% and 87.77% with it, both in a square hall shifted 256 u.

The cause is the wall's own back face. It splits before the wall's end planes, so the end planes never reach the region behind the wall, which no longer holds the wall. Only cap grid lines (x=0, x=1024, x=2048) and the hall walls bound that region. The grid lines fall inside the lanes, so a cell such as [0,1216,0]..[1024,1984,256] holds both lane space and space behind the wall.

The outcome follows the hall's shape, with the wall at x 256..1280 unless a shift is given:
- **Wider than deep:** 42.00% mean, 100% worst, for a 2048 × 1536 u hall; the committed 2560 × 2048 u fixture gives 28.46%.
- **Square (2048 u):** 25–32% mean, 76–94% worst, at shifts of 128, 256, 384 and 512 u. At shift 0, the grid lines x=0 and x=1024 coincide with the wall's ends and the leak is 0.
- **Deeper than wide:** 0.00% in all four variants, at 1792 × 2048 u and 2048 × 2560 u.

The in-process figures equal the compiled fixture's: 27.97% and 87.77% for the square hall at shift 256, in both.

The map's path fits the same rule. After x=−212 and x=1536, its region is about 1750 u wide against 2064 u deep.

On the fixture's path, the second occluder's y=1984 does split before the wall's faces and bounds two leaking cells, but it does not cause the leak.

## Results: fixture row (C18)

`walk_from_in_front_of_a_laned_wall_draws_no_cell_holding_space_hidden_behind_it` runs the production pipeline: `compile` and then `load_prl`.
- **Hall:** x −512..2048, y 672..2720, z 0..512, 65 × 52 × 13 m.
- **Wall:** b37's size at [256,1184,0]..[1280,1216,256], off the 1024 u grid, with a 19.5 m lane past each end.
- **Second occluder:** a stand-in for brush 102 at [−144,1984,0]..[−44,2984,512], which classifies `Occludes`.
- **Eyes:** the review's lattice carried with the wall: x {320, 556, 800, 1056, 1236} × y {700, 900, 1100}, 75 poses.
- **Samples:** the lattice above over x [256,1280], with 12,032 hidden and 0 seen.

At HEAD it **fails**:
- 28.46% mean and 87.77% worst, at eye (320, 700), yaw 70°.
- 4 leaking cells, each spanning x 0..1024 or 1024..2048 behind the wall and bounded by grid lines.
- The pre-change build leaks 78.18%.

It is marked `#[ignore = "C18 fails until shadow-prism splitters; see plan.md 9h"]`.

## Decision read

On the real map the leak is nil, with 0 of 11,538 hidden samples at all 70 poses in both cap builds. It is robust to moving the wall and to removing every other occluder in the hall. By the plan's rule, nil means shadow-prism splitters become a follow-up.

The guard row as written still fails, because the partition does not bound shadows in general. A free-standing occluder whose back face outranks its end planes leaks everything behind it into the lanes: in the fixture, about 28% mean and up to 100% worst. Only grid coincidence or neighbouring geometry prevents it. This is an owner call:
- **(a)** Treat C18's fixture as a known-open guard and defer prisms (map nil).
- **(b)** Read the fixture as the material case and build prisms here.

Prisms, or a cheaper ordering fix such as ranking an occluder's end planes ahead of its back face, would turn the fixture's 28% into 0.
