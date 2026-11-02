# Coverage proof: vertex weld + strict balance — consensus design

**Status.** Reviewer and orchestrator consensus, 2026-11-02.

## Owner items (before building)

1. **Replace owner rulings 1b (c) and (c′).** Those are the seam-overlap excuse and the corner fan,
   recorded in `plan.md` task 1b. The weld below replaces them. It relaxes the brief's
   "one-for-one on every sub-span" only where two copies of one corner, from different triangles and
   no more than δ apart, are merged.
2. **Widen named gap 3.** Today it reads "a solid region whose faces are all missing". It should add
   "or narrower than τ". The reason is a blind spot strict balance has today:
   - A face narrower than τ matches against itself, because its own opposite edges lie within τ of
     it.
   - So a solid whose remaining faces are all such slivers reads zero unproven. An example is diamond
     strips tapering to points at two dropped faces' rims.
   - Verify mode guards it. The weld does not widen it, because of rule 1.3(b).
3. **The proof may now report a hole that is not one.** Gap-shaped corner copies more than δ apart
   (see Named costs) are reported as holes. That costs speed, never bytes. A gate map that needs
   stand-ins stops the build for a decision (δ procedure, step 6).

## Context

- **Worktree:** `/Users/dhiester/Projects/Personal/postretro/.claude/worktrees/lightmap-reach-cull`.
- **Branch:** `feat/lightmap-reach-cull`. Base: 9050813f7.
- **Code:** `crates/level-compiler/src/coverage_proof.rs` and `coverage_proof/`.
- **Brief:** `context/plans/in-progress/lightmap-reach-cull/index.md`, Decisions on the coverage proof
  and Acceptance. Research P6–P8 are in `research.md`.

## Why

The proof checks oriented edge balance over the faces the bake BVH holds. Copies of one corner,
computed from different brushes' planes, land micrometres apart along a line. On clean geometry they
leave overlap stubs: two same-direction edges both cover a few µm against one opposite edge.

Three rounds of local excuses were each broken by adversarial review:
- the end-stub waiver;
- the seam overlap;
- the corner fan.

So were the stand-ins: when uncapped, they let a chain of collinear edges supply two partners per
piece (R18).

The weld merges the copies into one vertex. Strict balance then follows, with no excuses and no
stand-ins.

## The rule

### 1. Weld: identify, never measure

1. **Inputs.** Take every distinct stored triangle corner, by f32 bits with −0.0 read as 0.0, as
   `edges.rs` keys them. Widen each to f64.
2. **Link.** Link every pair of corners within δ (3D, f64). Clusters are the connected components:
   single linkage over **all** pairs, built with a spatial query and union-find. No grid snapping, no
   greedy assignment.
3. **Refuse** a cluster that meets either condition. A refused cluster's corners stay unwelded.
   - (a) Its diameter (max pairwise distance) exceeds δ.
   - (b) It holds two corners of one triangle. Fan diagonals and oversize-cut triangles count. So no
     triangle loses a corner, no edge collapses, and no face folds away.
4. **Representative.** Each accepted cluster has one, chosen from geometry: the member with the
   smallest bit-key. Never use a buffer or face index. This keeps the verdict and the reported spans
   identical in every face order (research P8).
5. **Scope.** Welded identity, and the representative's position, are used only to:
   - build edge keys for fan-diagonal cancellation (`cancel_reversed_pairs`). Cancel on welded ids.
     Rule 3(b) means no edge becomes zero-length, so `DirectedEdge::direction` stays valid;
   - place edge ends on the cluster axis (`Placed::t_from` / `t_to`), so welded copies share a break.
6. **Every distance stays on true geometry.**
   - A piece end on an edge is that edge's **true** point at the same fraction of its welded span.
     `Placed::at` returns the stored true corner at the welded ends, and true-edge interpolation
     between them.
   - Ends are measured against the partner face's **true** triangles.
   - The limit is today's `limit`: τ less the rounding margin, with τ read from
     `TRACER_TOLERANCE_METERS`. There is **no δ margin**.

### 2. Matching: strict, no excuses, no stand-ins

- Every edge running over a piece is required, and only those edges are nodes. Each piece gets one
  maximum bipartite matching between the two directions (`matching.rs`'s `uncovered`, without the
  seam path). An uncovered edge is an unproven span.
- A pair needs both:
  - the edges are opposed (`MIN_OPPOSITION`);
  - each one's piece-stretch lies within `limit` of one triangle of the other's face
    (`Measure::covers`, unchanged).
- Clustering of edges into lines (`clusters.rs`, `LINK_RADIUS_TAUS`) and the reference axis are
  unchanged. They run on the welded ends.
- With no stand-ins, every node is required. `PieceGraph`'s `required` and `total` collapse into one
  count per side.

### 3. Fallback, only if the δ procedure needs it: capped stand-ins

Do **not** build this unless step 6 of the δ procedure triggers it, and the owner and orchestrator
approve.

- Stand-ins return as today: an edge may partner on pieces within τ (axis units, from its welded end)
  past its own end. The point evaluated is its **true** end.
- Add a count cap. On a piece, a direction may use stand-ins only up to `window_max − present`:
  - `present` is that direction's present-edge count on the piece.
  - `window_max` is the largest present count of that direction on any piece within τ.
- The cap is enforced **jointly**. One matching must cover every present edge on both sides and use
  at most each side's cap. Do not combine two one-sided runs, which is how `cover` works today.
  Solve it as a flow with lower bounds, or as a bounded exhaustive search that fails toward unproven.
- If built, it needs its own adversarial review: the uncapped version was broken once.

### 4. Delete

- `coverage_proof/seams.rs`, which holds `Turns`, `seam_overlap`, the fan, `FAN_CAP` and
  `SEARCH_CAP`.
- From `matching.rs`: `proven_with_seams`, `disjoint_choices`, `joined` and `SEAM_CAP`.
- From `settle.rs`: `seam_pairs`, the seam branch, the `near` side lists and the stand-in window.
- From `Measure`: `end_reach` and `turns`.
- The header of `coverage_proof.rs`, lines 4–31: rewrite it to state the rule and the invariant
  below.

### 5. Constant

- Add `WELD_RADIUS_METERS: f64` beside the proof, in `coverage_proof.rs`. It is a named literal,
  **not** derived from `TRACER_TOLERANCE_METERS`: the copies come from face extraction's f64
  arithmetic, not from the tracer.
- Its comment names this procedure, the gate-map test and the measured δ_min, and states the τ/4
  ceiling.
- A unit test asserts `WELD_RADIUS_METERS <= TRACER_TOLERANCE_METERS / 4`.
- A `#[cfg(test)]` seam sets δ = 0 to turn the weld off, for the negative-control rows.

## Soundness (single invariant)

- **Weld map.** w maps each corner to its cluster's representative, with |w(v) − v| ≤ δ. By rule
  1.3(b), w is injective on every triangle's corners: every edge survives with nonzero length, and no
  face folds.
- **Closed stays closed.** w is a chain map, so the image of a closed oriented surface is closed.
  Closed shells stay balanced on every welded piece, whatever their copies.
- **Where the weld can cancel a leftover.**
  - On a line, an edge's welded extent differs from its true extent only between a true end and its
    representative, which is at most δ.
  - A path of collinear edges keeps net count ±1 between its welded ends.
  - So the weld cancels a leftover stretch only between two corners that belong to different
    triangles, share a cluster, and are joined by no edge: two copies of one corner.
  - A path of edges cannot be shortened to nothing. Under rule 1.3(b), consecutive corners within δ
    share a triangle and are refused.
  - Diameter refusal stops clusters chaining along a line, so each cancellation is isolated and at
    most δ long.
- **Distances.** Every true point of every edge is checked against its matched partner's true face,
  within τ. This is the brief's crack-width metric, unchanged.
- **Net effect.**
  - Beyond strict balance, the welded proof misses only corner gaps no wider than δ at copy
    clusters.
  - Every attack found so far is caught by edge counts: the hinge drop, stub chains, axis skew,
    staggered strips, cap boxes, the full leak, the deficit-side chain, slot-aligned strips and the
    fold comb.

## δ selection procedure (with the stand-in decision)

1. **Instrument** the weld behind the `#[ignore]` gate-map test (`coverage_proof/gate_map_tests.rs`).
   Per map and configuration, log:
   - accepted clusters and the largest accepted diameter;
   - refused clusters, by reason (diameter or same triangle);
   - unproven spans, each classified as overlap-shaped or gap-shaped, with its copy distance in 3D
     and along the line.
2. **Sweep matrix**, on hallway, hallway-mini and movement-feel, post-cut geometry:

   | config | δ | stand-ins |
   |---|---|---|
   | baseline | 0 (weld off) | off |
   | ladder | 5, 10, 20, 30, 50, 75, 100, 125 µm | off |
   | ladder | the same | on (today's uncapped stand-ins, measurement only) |

   The "on" column only says whether any gap needs stand-ins. It never ships uncapped.
3. **δ_min** is the smallest ladder δ at which all three maps read 0 unproven with stand-ins **off**.
4. **Set δ** to 2·δ_min, rounded up to the next ladder value or to a round 10 µm. All three maps must
   also read 0 at the chosen δ with stand-ins off. Refusals grow with δ, so the count is not
   monotone.
5. **Ceiling:** δ ≤ τ/4 (125 µm). Stop and report to the owner if δ_min > τ/8 (62.5 µm).
6. **Stand-in decision.**
   - If step 3 finds a δ_min ≤ τ/8, stand-ins stay deleted.
   - If no δ ≤ τ/4 reads 0 with stand-ins off, but some δ does with them on, the leftover is
     gap-shaped. Stop: report those spans with their shapes and distances, and propose the §3
     fallback for owner and orchestrator approval.
   - If nothing reads 0, stop and report.
7. **Anchor.** With the weld off, movement-feel should show the pieces that fail plain strict balance
   (expected 6), and the hallways 0. If not, report the difference before going on.
8. **Record** the matrix in `plan.md` task 1b's row, and δ_min in the constant's comment.

## Named costs

All of these are false holes. They cost speed, never bytes. One false hole turns the cull off for the
whole map, about 1.75× Lightmap Bake on the hallway.

- **Gap-shaped copies more than δ apart in 3D.** These were the stand-ins' job. They include:
  - along-line gaps in (δ, τ];
  - gaps along the line under δ at the end of a crease crack wider than δ across, because the corner
    copies sit a crack-width apart and do not weld. movement-feel's widest crease crack is 198.7 µm
    (watertight-tracer census), so this is the case the sweep is most likely to find.
- **Overlap-shaped copies more than δ apart.** For example, a cracked corner whose ends overlap along
  the line (R12).
- **Refusals.**
  - A cluster refused by diameter or by sharing a triangle stays unwelded, and its stubs read as
    holes.
  - Example: a face's own T-junction corner within δ of its corner, next to a copy of that corner.
- **T-junctions.**
  - A corner within δ of another edge's interior, but not of that edge's corners, is not welded. It
    only breaks the line into pieces, as today, and moves nothing, so it cannot manufacture balance.
  - It costs a false hole only if it links two copy clusters into a chain that the diameter test
    refuses. That shows in the refusal log.

## Fixture rows

Kinds:
- **F** fails on an unsound build: the hole must be found.
- **C** marks cost or behaviour.

Placement and frame:
- Put the rows in `coverage_proof/stub_tests.rs`, or in a new sibling test module once that file
  passes about 400 lines.
- Use the `oblique` `Frame` unless stated.
- `L` is the hinge length.

Assertions:
- The weakest acceptable form is "unproven ≥ L − 2δ along the line".
- Never assert only that some span exists.

| id | scenario | expected | kind |
|---|---|---|---|
| R1 | `beam_corner(frame, 10e-6)` (existing) | 0 unproven | C |
| R2 | R1 minus the sloped face (existing) | unproven span within 1e-4 of the corner | F |
| R3 | R1 with the weld off (δ = 0) | unproven at the corner; this shows the weld does the work and no seam or fan code remains | C (negative control) |
| R4 | `beam_corner` with copies δ·(1 − 1e-3) / δ·(1 + 1e-3) apart | proven / unproven | C |
| R5 | three collinear corner copies of different faces at c, c + 0.6δ and c + 1.2δ (diameter 1.2δ) | cluster refused, unproven; identical in every face order | F |
| R6 | existing hinge counterexamples with D dropped: axis skew, sub-τ chain, staggered 0.98 mm strips, strips plus cap boxes | unproven ≥ L − 2δ along the hinge (`assert_hinge_hole_found`: loosen its 1e-6 to 2δ, no further) | F |
| R7 | staggered strips with stagger 0.8δ and strip width > 2δ, so the copies weld; D dropped | unproven ≥ L − 2δ: edge count, not stagger size, catches it | F |
| R8 | full leak: R6's structure on all 8 rims of brush 1's x = 0 and x = s faces, both dropped | unproven along **each** of the 8 rims | F |
| R9 | crease crack 0.99τ (as `coverage_proof_proves_a_crease_crack_just_under_tau_and_not_just_over`) whose ends also carry δ/2 along-line copies of different faces | 0 unproven; this fails if a δ margin creeps in | C |
| R10 | R9 at 1.01τ | unproven along the creases (P6) | F |
| R11 | true gap τ + 0.9δ between an edge end p and partner corner q; p and q each have a copy 0.95δ toward the other | unproven; this fails if distances use welded positions | F |
| R12 | crack 3δ wide across, its ends overlapping 0.2δ along the line | unproven at the ends: the documented cost | C |
| R13 | `hinge` fixture: its four faces in all 24 orders × a permuted vertex buffer × translation by (1000.123, −517.9, 33.3) | identical verdict and unproven spans (up to the translation), intact and with one face removed | F |
| R14 | sliver face 0.5δ wide and 1 m long between two neighbours; then with one neighbour dropped | 0 unproven; then unproven ≥ 1 m − 2δ | F |
| R15 | two closed boxes corner to corner within 0.5δ, plus a face dropped 1 m away | corner proven; the drop found ≥ its rim length − 2δ | F |
| R16 | gate maps at the chosen δ, stand-ins off | 0 unproven on hallway, hallway-mini and movement-feel; the matrix is logged | C |
| R17 | gate maps with the weld off | movement-feel shows the pieces strict balance fails; the hallways read 0 | C |
| R18 | hinge (`chained_hinge` layout), D dropped; brush 0's x = 0 face R1, the remaining −y edge, carries collinear hinge corners every 0.4 mm | unproven ≥ L − 2δ | F |
| R19 | as R18, with R1 cut into 0.4 mm strips with 0.1 mm slots, and brush 1's bottom present only under R1's slots | unproven ≥ L − 2δ | F |
| R20 | intact hinge whose D edge ends 0.3 mm (> δ, < τ) short of R1's end, at both ends | unproven at the two 0.3 mm gaps: the gap-shaped cost (proven only under the §3 fallback) | C |
| R21 | fold comb: box [0, 2 cm]³ with its x = 0 and x = 2 cm faces dropped. Its other four faces are strips along x, 0.5δ wide, spaced 0.5δ + 100 µm apart (slots under τ, over δ) | unproven at the strip ends on all four rims of both dropped faces; without rule 1.3(b), every strip folds away and it reads 0 | F |
| R22 | a face whose own T-junction corner lies within δ of its corner, next to a copy of that corner from another face | cluster refused (same triangle); the stub reads unproven and is logged as a refusal | C |

**Tests that pass on a build that still leaks.** Never treat these as evidence of soundness:
- R1, R16, and the brief's gate rows (`cmp`, verify, 0 unproven on the gate maps). They pass when δ
  is too large, refusal is missing, rule 1.3(b) is missing, a stand-in is left in, or seam code
  remains.
- Any assertion that only counts spans.

## Build order

1. **Confirm the counterexamples at 9050813f7, before changing anything.**
   - Write R18 and run it: the hand trace says today's uncapped stand-ins read it **proven**.
   - Write R21 and run it against a weld prototype without rule 1.3(b); the hand trace says it reads
     **proven**. Today's code has no weld, so R21 should read unproven there; record that.
   - If either trace is wrong, report before going on.
2. Delete the seams, the fan and the stand-ins (§4). Add the weld (§1), true-point measurement and
   strict matching (§2).
3. Make rows R1–R15 and R17–R22 pass. They are written against the stand-ins-off build. R20 asserts
   the cost.
4. Run the sweep matrix and the stand-in decision (δ procedure). Set the constant. Run R16. Stop at
   step 5 or 6 if either triggers.
5. Self-review the diff once against this file.
6. Hand off for a fresh adversarial review of the weld, rule 1.3(b) and strict matching together. Give
   the reviewer the invariant above and ask it to break the invariant, not to confirm it.
