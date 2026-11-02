# reflection-probe-capture-exclusion-held-kept-solid

Brief · compact · reads: `context/lib/resource_management.md` §5.2 (Exclusion) · `context/lib/build_pipeline.md` §Build Cache · `plans/done/reflection-probe-capture-exclusion/` (index Decisions, Contact holes; research §Orderings O3) · `plans/done/watertight-tracer/plan.md` §Review loop, round 5 · read at ffd61b87b (feat/watertight-tracer) · builds after `watertight-tracer` lands on main

## Problem
The watertight-tracer round-5 plan review recorded this as known limit R6. Its basis is a construction found by reading `ProbeExclusion::cross`; it has not been run (layout and trace: `research.md` §R6 by reading). Cause: when an excluded hull's exit is held by both kept solid and a nearer excluded neighbour, `nearest_held` takes the neighbour and drops the kept depth. So a ray that leaves that neighbour into open space, past its contact range, and then enters the kept solid through a face CSG removed against the first hull, re-traces through the solid and reads what lies behind it. That breaks §5.2 "No leaks". When this is done, kept solid that holds any exit a ray crosses bounds everything that ray reads afterwards. The ray reads a kept face at or before the depth where it enters that solid, or else takes the leak guard. The kept-behind read that round 5 kept still reads the kept face. If the R6 row passes on ffd61b87b, the brief closes as no work; if its layout cannot meet its asserted preconditions, the build stops for the owner.

## Decisions
- **Scope: R6's class, not the layout.** The class: a held kept depth that the classification passes over because an excluded neighbour is nearer, at any exit of the trace. That covers real crossings and beside passes, a second hull in the same classification, and later hulls after a re-trace. Every read kind is bounded: a kept hit, a fill, and an exit into open space. A ray inside held kept solid past its depth is reading through that solid, because CSG removed every face the ray could meet there (`research.md` §Why depth is the bound).
- **Carry the held kept depth.** The trace keeps the least kept depth it passed over. A kept hit at or before that depth reads as it does today. Any read past it takes the leak guard on the first skipped face, as `plans/done/reflection-probe-capture-exclusion/` research O3 rule (3) has kept solid do. The nearest-held rule, its tie to kept solid, and the order of fill and exit stay as they are.
- **Rivals rejected.**
  - Kept-first breaks the kept-behind row (round 5).
  - Carrying each passed hull's contact set either misses R6, if contact is measured against the current hull, or guards the kept-behind row, if it is measured against the hull the kept brush held.
  - No static rule at the exit can tell R6 from the kept-behind row. Both leave the neighbour into open space short of the kept depth, and they differ only in whether the kept entry face survived, which only the re-trace reads (`research.md` §Rivals).
- **No new contract.** §5.2 already forbids the leak. The fix restores "No leaks", and it leaves the projection box and its floor, the fill and the leak guard's surface as they are. A brush wholly off the box is never a contact (contacts are drawn from brushes touching the box), so it adds no depth, and it captures the same whether kept or excluded.
- **Layer.** Compiler only, inside the reflection-probe capture's exclusion trace. No runtime, section-format, shadow-ray or box-fit change (§5.2 Runtime-blind).
- **Cache: capture bytes only.** The reflection-probe capture `STAGE_VERSION` bumps. `TRACER_EPOCH` does not move: per plan.md's cache-key record, capture-only changes bump the stage version.
- **Lands after watertight-tracer**, on main, in its own PR. R6 rides that branch's round-5 rule, so the branch lands first.
- **Non-goal: another review of the classification by reading.** Each of the five rounds found one narrower case by reading and was then fixed, and reading has no stop rule. Whether a generated-layout test takes its place is the open question below.
- **Non-goal: kept contacts that the contact scan misses** past an acute edge (`ProbeExclusion::contacts` doc). This is an accepted limit already, and box layouts never reach it.

## Acceptance
### Automated
R6 and its class:
- [ ] **R6 layout** (map units, the split-platform rows' conventions with no near brush, h = 8; layout and ray in `research.md` §R6 by reading). Asserted preconditions: CSG removed the kept brush's front and top; both brushes hold the block's exit, with the excluded one nearer; the kept brush does not hold the excluded brush's exit. Unmasked, the ray hits the block's front. With the block and the excluded brush excluded, the ray reads the leak guard on the block's front. The row fails on ffd61b87b with a hit on the wall behind.
- [ ] **A later hull.** The R6 layout with the excluded brush split along the ray, with a gap between the pieces. The ray leaves the first piece into open space, re-traces, and crosses the second piece before it reaches the kept brush (sketch: `research.md` §Class rows). It reads the leak guard on the block's front.
- [ ] **Every read kind, both sides.** Once a kept depth has been passed over, a kept hit, a fill and an exit into open space each take the leak guard when they lie past that depth. A kept hit at the depth, within distance rounding, reads the hit. Pinned through the trace's own seam, one case per kind on each side.

Regression guards (each passes before and after):
- [ ] **Kept-behind row.** The row whose kept front lies past the excluded brush's far side, with the 1 u sliver, still reads that front at its distance. It also asserts that the front lies at the depth the kept brush held at the block's exit. That makes it the at-the-depth case, the one that tells this rule apart from kept-first.
- [ ] The split-platform rows, as a beside pass and as a real crossing, the exact-tie row, the selection-rule unit test, the beside-pass budget, the crate-seam rows and every other capture-exclusion, fill and capture test pass unchanged.
- [ ] **Off-box invariant.** The off-box seam test passes in both tie orders. The R6 layout with its projection box ending short of the kept brush gives the same outcome on every ray whether that brush is kept or excluded.

Cache and bytes:
- [ ] The capture stage's version is one higher, and the tracer epoch is unchanged (diff).
- [ ] A warm build of `reflection-probes-test.map` over a pre-change cache misses the capture stage and hits every tracing stage before it. A cold build differs from the pre-change cold build in id 54 at most. The PR records whether id 54 moved.

### Manual
None. The rays affected graze 1 mm contact gaps, below a probe texel, so the visual check would show no change.

## Path
- **Seams.** `cross` already builds the held kept depths and hands them to `nearest_held`. Return the least one beside `Held::Excluded`, or have `cross` keep it, and let `trace_counting` carry the minimum across loop passes. `settle`, `filled` and the `Exit::Open` arm are the read points. Alternatively, clamp the re-trace's `max_distance` to the bound, so that a miss settles to the leak guard by the existing rule. Fills still need their own check.
- **Shape and rival.** Chosen: one carried depth, checked where the trace settles. Rival: verify each final read against the hulls of every kept contact passed (a segment-in-hull test). It closes the same class: CSG removes a kept face only within 1 mm of the hull it touched, so a removed face the ray enters through lies within contact of a hull the ray crossed, where `held_at` already finds it. Run inline it costs what the chosen shape costs, since `cross` already spans every kept contact of each hull it classifies; run after the read it repeats those spans. Chosen because it adds no second rule beside `held_at`'s; the rival stays the oracle's form (Open questions).
- **First slice.** The R6 row alone, on ffd61b87b, before any fix. It falsifies the premise. If it passes, or its asserted preconditions cannot all hold in this layout, stop and report the trace.
- **Files.** `capture_exclusion.rs` is at ~800 lines. First split `held_at`, `Held`, `nearest_held` and `ProjectionBox` into their own module, behavior-preserving, in their own commit. `capture_exclusion_tests.rs` is past 1,500 lines, so the new rows go in a sibling test module that reuses `split_platform_under_a_block`'s builders. A no-near-brush variant is a parameter, not a copy.
- Mm-scale constructions are easy to get wrong by reading. Each row asserts its preconditions, as the round-5 rows do, so a fixture that drifts fails loudly rather than passing vacuously.

## Open questions
- Replace the reading audit with a generated-layout no-leak test? (a) No: R6's class only. (b) Yes, in this brief: box brushes in contact and near contact, with rays aimed into their gaps at grazing slopes, checked against an oracle independent of the classification. The oracle: no read past a point where the ray is inside a kept contact's hull, beyond rounding, computed from the brush planes directly, not through `ProbeExclusion::span`, `held_at` or the carried depth. After this fix the oracle and the trace state one rule, so its independence lies in the implementation. The case count is bounded, and a failure stops the build for the owner. (c) Yes, as its own brief after this one lands. Option (b) adds Acceptance rows and may find cases beyond this class (`research.md` §Audit). — owner — **blocks build**
- Slack at the held depth: how far past it an `f32` kept-hit distance may land and still count as "at". It covers at least `f32` distance rounding and never exceeds `CONTACT_TOLERANCE_M` along the ray. — **delegated**: the executor sizes it and reports it in the plan of record
- Carried-depth check at each read point, or a clamped re-trace reach plus a fill check. — **delegated**
- A new leak case the build's review finds outside R6's class: (a) fix it in this PR, as `watertight-tracer`'s rounds 1–5 did; (b) record it as a known limit with its construction and hand it to the oracle; (c) stop for the owner. Under option (b) above, the oracle's failure rule governs. — owner
