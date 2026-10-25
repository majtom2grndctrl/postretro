# portal-walk-bounded-regions — plan of record

mode: resumable
status: approved
read at: d23959def

Branch: `portal-walk-bounded-regions`, cut from the claim commit d23959def. No `crates/`
commit lands between the brief's read commit 800bbe186 and d23959def, so every cited
symbol reads as the assessment recorded it (`portal_vis.rs` `flood` :281,
`camera_on_polygon_plane` :671, `narrow_frustum` :693, `MAX_PORTAL_WALK_STEPS` = 20,000
:26; `visibility.rs` fallback :435–469; `PORTAL_WALK_EPOCH` = 1; the step-limit branch in
`pvs_sampling.rs` :296–306; `CELL_RESIDENCY_SET_STAGE_VERSION` = 3).

## Corrections

- **Orientation data lives on `LevelWorld`.** Brief: "runtime traversal in the visibility
  crate" (Placement), and "each portal's source side is settled before any walk, from data
  every fixture and shipped map carries" (O1) → the walk's facing filter needs each
  portal's plane and which side its front cell lies on. That is static per world, and the
  scratch can't tell two worlds apart reliably. Planning around it by computing per-portal
  plane and front-side sign once in the `LevelWorld` constructors in `level-loader`, next
  to the existing portal validation. Every fixture builds through `new_visibility_only`,
  so fixtures get it too. The evidence is geometric, not the locator. Cells are convex BSP
  leaves, so a cell's other off-plane portal centroids lie on its side. The fallbacks are
  the opposite cell's evidence, reversed, and then the cell bounds centre. A portal with
  no evidence is unoriented: the facing filter skips it, which is superset-safe and still
  terminates. This is the prototype's locator-probe failure (O1), avoided. The traversal
  itself stays in the visibility crate.
- **The walk core lands early, test-only.** Brief: commit order is split → instruments →
  walk with the epoch bump, and the pre-change build carries the old walk and epoch (O19).
  The Path also puts the walk first, as the risk-first slice. Planning around both by
  landing the region walk, oracle and fixtures in the visibility crate under
  `#[cfg(test)]` before the instrument commits. Nothing calls the walk until the wiring
  task, which also bumps the epoch. So the pre-change build (the last instrument commit)
  carries the new code only as test code: its runtime walk, epoch and bake are the old
  ones. That keeps O19's meaning and needs no rebase.
- **The test build still breaks.** The brief says `cargo test -p postretro-visibility`
  doesn't build. At d23959def, `crates/level-loader/src/prl.rs:64` still imports
  `TeleportersSection` without the `load-prl` gate. Task 1 fixes it, as the Path asks.
- **Retired counters.** `VisibilityStage` carries `walk_rejected_narrow`,
  `walk_rejected_cycle` and `walk_rejected_depth`. These are chain-walk concepts with no
  meaning in the region walk. They retire with the wiring task, and the oracle keeps its
  own counters. `walk_considered` keeps O11's meaning.

## Delegated answers

- **Snap grid and cap.** Q = 256 steps per NDC unit, the prototype's measured choice
  (`research.md` §Variant results). The target cap is 5,000, which gives 2.5× headroom
  over the worst prototype walk (1,987). Task 8's sweep fixes the final value: at least 2×
  the worst portal-test count over every fixture and swept map. If the tightened walk's
  worst count passes 2,500, the cap rises with it.
- **"Near the polygon."** The bypass applies when the eye's 3D distance to the closed
  portal polygon is at most `PORTAL_BYPASS_DISTANCE` = 0.01 m. That covers locator and
  float disagreement at a shared plane. Distant coplanar portals, the sweep's worst
  over-inclusion, then clip normally. The fallback's keep test only has to add cells
  touching the eye.
- **Facing sign rule.** A walk crosses portal P out of cell C only when the eye's signed
  distance to P's plane, positive on C's side, exceeds `APEX_ON_PORTAL_PLANE_EPSILON`
  (1e-3). The superset check classifies backward crossings with this same predicate (O36).
  Unoriented portals skip it.
- **Order of rejection tests.** Per outbound portal:
  1. count it (O11);
  2. blocked;
  3. invalid or solid neighbour;
  4. seeded entry only: wholly behind the caller's near plane (O31);
  5. bypass;
  6. facing;
  7. 3D clip against near, rect and far, then project.

  The camera entry never runs step 4, so the bypass reaches neighbours behind or beside
  the eye (O6, O26).
- **Sweep instrument.** An `#[ignore]`-gated test in the visibility crate. It reads `.prl`
  paths from an env var and reaches the test-only oracle directly, so no oracle code ships.
  It runs single-threaded in release:
  `cargo test --release -p postretro-visibility <filter> -- --ignored --test-threads=1`.
  Its rerun command goes in `context/`, not `docs/`.
- **Constant k.** Taken from the worst portal tests per summed degree of reached cells over
  the fixture sweep, rounded up, with at most 2× headroom, and never above 32. Fixed in
  Task 2, rechecked in Task 6.

- **Tangent portals need a clip pad.** Row 13: with the clipper's on-plane rule, a portal
  touching a rect only along an edge clips to two vertices and drops. The walk clips
  against its rect pushed out by half a snap step, then intersects the result with the
  rect, so the pad never widens a region.

## AC-to-proof

Report sets R1–R4 are in `research.md` §Report sets. "Fixtures" means the synthetic
stacked-slab shaft, the open 3D lattice and the cyclic rings, plus the named cases. Each
fixture has its own locator.

| AC | Row | Proof | Status |
|---|---|---|---|
| 1 | Superset over randomized cameras, two exception classes only | `region_walk` superset sweep test vs oracle, classifier | achievable as stated |
| 2 | Fixed seed and count; oracle cap far above today's; a slab camera exceeds today's cap and completes under the oracle; oracle cap and depth hits reported | same sweep test, asserts and report | achievable as stated |
| 3 | *Guard:* existing positive traversal tests pass on their current fixtures (O1) | existing `portal_vis` tests rerun against the new walk | achievable as stated |
| 4 | *Guard:* two paths, wider opening wins, both portal orders | `two_paths…` test, both orders | achievable as stated |
| 5 | Portal crossing the camera plane includes its neighbour | focused test | achievable as stated |
| 6 | *Guard:* camera on polygon or just inside the wall sees through | existing regression probes | achievable as stated |
| 7 | On plane just outside the edge: inside distance both ways, beyond it clipped; shared slab plane (O8, O10) | focused tests | achievable as stated |
| 8 | Eye on a distant portal's plane: misses are classified (O36) | focused test vs oracle | achievable as stated |
| 9 | Within render near of two consecutive portals (O9) | focused test | achievable as stated |
| 10 | Bypass off the camera cell hands at most the near cell's rect; no re-expansion (O23) | focused test, expansion counter | achievable as stated |
| 11 | Late rect growth re-expands descendants (O2) | diamond fixture test | achievable as stated |
| 12 | One-grid-step growth re-expands; a no-op doesn't (O3) | focused test | achievable as stated |
| 13 | Portal tangent to a rect edge or snap boundary included | focused test | achievable as stated |
| 14 | *Guard:* camera cell always visible on the camera entry | sweep assert | achievable as stated |
| 15 | Seeded walk from several cells under an oblique near plane ⊇ seeded oracle; locator cell no seed reaches stays hidden; seed shuffle changes neither set nor tests (O27, O33) | seeded tests | achievable as stated |
| 16 | Cell only behind the caller's near plane stays hidden, even within bypass distance (O31) | seeded test, walk and oracle | achievable as stated |
| 17 | Seeded from the camera cell under the slid frustum equals the camera entry (O27) | test | achievable as stated |
| 18 | No or unusable seeds: empty, no trip; repeated seed is a no-op; each unusable kind skipped (O32) | tests | achievable as stated |
| 19 | Seeded cap trip reported to the caller, distinct, no fallback set (O34) | test | achievable as stated |
| 20 | *Guard:* set shape at every sweep camera; draw set = drawable part of fog reach on walk frames (O4, O12) | sweep asserts through `determine_visible_cells` | achievable as stated |
| 21 | Back-to-back walks match fresh state: door toggle, cell change, larger world, seeded walk between, generation wrap, threads (O7, O25, O35) | scratch tests with a test-only generation setter | achievable as stated |
| 22 | *Guard:* L-corridor far end and straddling side branch stay hidden | existing tests | achievable as stated |
| 23 | *Guard:* small opening whose rect misses the portal stays hidden | focused test | achievable as stated |
| 24 | *Guard:* portal wholly behind the camera rejected | existing or focused test | achievable as stated |
| 25 | *Guard:* blocked portal blocks both ways; closed closet out of draw and fog sets, camera against the door (O37) | existing blocked test plus pressed-camera case | achievable as stated |
| 26 | Distant coplanar portal clipped normally; cell hidden when the opening is outside the source rect | focused test | achievable as stated |
| 27 | Slab fixture: the exact walk exceeds today's cap; new walk ≤ k × summed degree, never trips | cost test | achievable as stated |
| 28 | No cell expands more than 32 times on any fixture | test-only expansion counter | achievable as stated |
| 29 | A rect grown twice while queued expands once (O22) | focused test | achievable as stated |
| 30 | Open lattice finishes under the cap on a thread with a small fixed stack | spawned thread with a fixed stack size | achievable as stated |
| 31 | Every outbound portal counts once, facing, blocked and solid included (O11) | counter test, test-only facing counter | achievable as stated |
| 32 | Exactly cap tests completes; one more trips; no portals never trips (O24) | cap-boundary test | achievable as stated |
| 33 | Cap ≥ 2× worst portal tests on every fixture, seeded included | fixture sweep assert | achievable as stated |
| 34 | *Guard:* portal-list shuffle gives an identical set and fog reach | sweep shuffle assert | achievable as stated |
| 35 | *Guard:* forced trip takes the bounded fallback, not draw-all | existing step-limit test | achievable as stated |
| 36 | *Guard:* a capped walk returns only cells the uncapped walk reaches, plus the camera cell (O5) | test on the bake-facing entry | achievable as stated |
| 37 | Fallback fog reach ⊇ uncapped walk: looking away, bypassed neighbour behind or beside the eye (O6, O26, O30) | fallback tests | achievable as stated |
| 38 | id-51 key differs from its pinned pre-change value; projections and stage version unchanged | key-pin test; key captured at the pre-change commit (Task 5) | achievable as stated |
| 39 | *Guard:* outside-baked-set never-refused demand tests pass unchanged | existing tests | achievable as stated |
| 40 | Dry-run report and `pvs_sampling` restated in every section, the tiles note included; test asserts the string | dry-run test | achievable as stated |
| 41 | Completed-reads mode: deferral counts once per frame; second drain doesn't advance retirement; undemanded block counts as left behind; per-frame trip record splits paired runs (O29, O38) | `walk_measurement` unit tests on a small fixture | achievable as stated |
| 42 | Manual sweep (R1) | sweep instrument, integrating executor, this Mac | measurement |
| 43 | Residency gate (R2, R3) | `walk_measurement`, both builds, integrating executor | measurement |
| 44 | SH reach | SH reach measurement, both bakes, integrating executor | measurement |
| 45 | Settle (R4) | Settling log, fresh launches, integrating executor | measurement, needs the engine |
| 46 | Live: no step-limit marker; frame, walk time and mesh instances vs pre-change | owner, in-engine, CPU timing | manual |
| 47 | Visual: no pops; shadow drops reported | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Split `portal_vis.rs` by responsibility, behaviour-preserving: entry, chain walk and clipping each in its own module, tests beside them. Gate the `TeleportersSection` import. Prove it: the same visibility test count passes before and after. | integrating executor | — | done: 54/54 visibility tests before and after; clippy clean |
| 2 | **Risk-first slice, test-only.** Add per-portal plane and front-side orientation to `LevelWorld` (Correction 1). Add the region walk core under `#[cfg(test)]`: a rect region type behind one seam, outward snap at Q = 256, the front-to-back worklist with ties by cell id, the facing filter, the tightened bypass, 3D clip before projection, and caller-owned scratch with generation stamps. Add the test-only oracle: today's chain walk with the tightened bypass, keeping parent side planes at bypass hops. Add the slab, lattice and ring fixtures with locators. Prove rows 1–14, 22–34 on the core, and fix k. | integrating executor, case tests delegated | 1 | done: `region_walk` 25 tests (sweep + `tests_cases`); 1,800 sweep cameras, 0 unclassified misses; worst 1,093 tests, 1.56 per degree → k = 2, worst 5 expansions; slab oracle 57,888 > 20,000. Rows 3, 4, 22 run on the existing `portal_vis` tests once Task 6 points them at the walk. |
| 3 | Seeded entry on the core and the oracle: seeds queued before expansion, the caller's near plane gating the bypass, skips for unusable seeds, a trip result. Prove rows 15–19. | integrating executor, tests delegated | 2 | done: `region_walk::tests_seeded` 9 tests, incl. rows 21 (seeded part) and 33; worst seeded 771 tests |
| 4 | `walk_measurement` modes: projection and pitch, completed reads, frame-counted retirement, per-drawn-block causes, the per-frame trip record and paired-run split. Prove row 41. | delegated worker (`walk_measurement/`), integrating executor reviews | 1 | |
| 5 | Settling log: step-limit frames, the settle set's peak in lightmap blocks and SH targets, hold time and reveal-frame misses, per entry. Capture the pre-change id-51 key for row 38. Mark this commit as the **pre-change build** in the plan. Build and copy out the pre-change release binary. | integrating executor | 4 | |
| 6 | Wire the walk:<br>• The camera entry goes behind `determine_visible_cells`, with a caller-owned `PortalWalkScratch` through every caller (frame loop, bake threads, `walk_measurement`, probes, mirror, tests).<br>• Lift the `#[cfg(test)]` gate and keep the oracle test-only.<br>• Set the cap and bump `PORTAL_WALK_EPOCH`.<br>• Widen the fallback's fog test with the bypass-distance keep test, and drop the retired counters.<br>• Point the bake's step-limit branch at the new walk.<br>• Restate `pvs_sampling`, the dry run's `visible_set` and the tiles note, and pin the key.<br>Prove rows 20, 21, 35–40. | integrating executor | 3, 5 | |
| 7 | Rebake the swept and measured maps after the bump: hallway, lake, occlusion-test, movement-feel, campaign-test. Check the id-51 bake reports no step-limit walks. | integrating executor | 6 | |
| 8 | Sweep instrument as an ignored test, then run R1 on the hallway, lake and occlusion-test, plus straight up and down at the shaft poses. Finalize the cap (2× rule) and recheck k. | integrating executor | 6, 7 | |
| 9 | Measurements: the residency gate R2 and R3 on both builds, SH reach, and Settle R4. A failed gate stops for an owner decision. | integrating executor | 5, 7 | |
| 10 | Preflight, then review-panel → fix → retest loop | integrating executor | 8, 9 | |
| 11 | Owner: Live and Visual rows | owner | 10 | manual |

Hot-path constraints carried into Task 6: no per-frame allocation once the scratch has
grown to the world (rects, stamps and the heap reuse capacity, and the output reuses the
caller's `Vec<u32>`); the walk is iterative, with no recursion; cost is bounded per
reached cell by the snap lattice, with the cap as backstop; walk time is measured through
`record_walk`.
