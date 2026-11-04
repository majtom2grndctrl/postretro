# reflection-probe-capture-exclusion-held-kept-solid — plan of record

mode: compact
status: active
read at: cc660f45b (no source under `reflection_probes/` changed since the brief's ffd61b87b; Decision reads reused)
re-checked at: main 0297abebe merged (2026-11-03). Main's only changes under `reflection_probes/` and `ray_traversal/` are visibility and a test-harness wrapper. After the merge: R6 and later-hull rows green (and still red uncarried); oracle refuses the same 13 rays as before the merge, the first A–D; the disabled-check oracle test still fails as required; every other level-compiler test passes.

## Corrections
- Path names `held_at`, `Held`, `nearest_held` and `ProjectionBox` for the split → `held_at` calls `outside_by`, which `capture_fill` also uses; it moves with them so the new module owns the hull-distance rule. The carried depth's type lives there too. Module: `capture_bounds.rs` (what bounds an excluded trace's reads).
- The brief says "builds after `watertight-tracer` lands on main" → landed (2f94024ec, merge of #638); this branch is cut from main after it.
- Acceptance, no-leak oracle: "the first point … inside a kept contact of a hull it crossed or passed beside" → one exception added, for the owner to confirm: a read the unmasked capture makes too is not refused, and kept solid entered before the unmasked read does not count, since up to there the ray is the unmasked one (`research.md` §Audit). "Crossed or passed beside" is the row's: the hull's planes, grown by τ, meet the ray before its read. Without the exception the row refuses 137 rays: 87 read as the unmasked capture does, 29 were already inside kept solid at their first excluded hit, and the 21 named below remain.
- An earlier oracle counted a hull only when the ray took a hit on its faces, on the premise that a ray taking no hit on a brush reads the same with it kept or excluded. Review disproved that (N in R6 reads differently kept), and the narrowing hid 8 real leaks, reported to the owner as read-the-same false positives. Replaced by the plane test above.

## Delegated answers
- Slack at the held depth — τ (`TRACER_TOLERANCE_METERS`, 0.5 mm) along the ray. It covers the `f32` hit distance (60 µm at 1 km) and the mesh's `f32` corner rounding (15 µm at ±500 m) seen along a ray up to ~88° off the entry face's normal, and stays a third of `CONTACT_TOLERANCE_M`. The oracle uses the same constant. Past 88° a kept hit at the depth on a surviving entry face, far from the origin, can land past the slack and take the guard: a safe-side error, sub-texel, needing the fill bypassed.
- Read-point check, or clamped re-trace reach plus a fill check — a check at each read point (kept hit in `settle`, fill in `filled`, `Exit::Open`), through one function the trace calls. One seam then covers all three read kinds and is what the read-kind rows pin; a clamped reach would split the rule between the tracer's reach and a separate fill check.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| R6 layout | `r6_*` row in the new sibling test module; fails pre-fix with a hit on the wall behind | achievable as stated |
| A later hull | split-N row in the same module | achievable as stated |
| Every read kind, both sides | seam unit test on the carried-depth check: hit / fill / open exit, at depth and past it; the nested-K row drives the open-exit arm through the trace | achievable as stated |
| Kept-behind row | existing row, plus an assertion that its front lies at the held depth | achievable as stated |
| Split-platform, exact-tie, selection-rule, beside budget, crate-seam, all capture/fill tests unchanged | `cargo test -p postretro-level-compiler --bin prl-build reflection_probes` | achievable as stated |
| Off-box invariant | existing seam test + R6 layout with the box short of the kept brush, kept vs excluded, on a 16-ray grid including R6's ray | achievable as stated |
| No-leak oracle | generated-layout test, plane-derived bound | achievable as stated |
| The oracle sees the class | same oracle with the carried-depth check disabled (test-only switch): fails on ≥1 generated case and on R6 | achievable as stated |
| Oracle in default `cargo test`; seeds, count, runtime, known limits recorded here | not `#[ignore]`; recorded below | achievable as stated |
| Review addition: no false guards | `the_carried_depth_changes_only_reads_the_oracle_refuses_without_it` over the same rays | added in review |
| Capture stage version +1, tracer epoch unchanged | diff | achievable as stated |
| Warm build over pre-change cache misses capture, hits tracing stages before it; cold differs in id 54 at most | prl-build on `reflection-probes-test.map`, pre-change binary vs this branch | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | R6 row alone, before any fix: falsify the premise (builder gains a no-near-brush parameter and per-brush tops) | integrating executor | — | done: preconditions hold; on cc660f45b the row fails with `Hit` on the wall behind at 1.711 m (map x = 128). Premise holds |
| 2 | Split `held_at`, `Held`, `nearest_held`, `outside_by`, `ProjectionBox` into `capture_bounds.rs`, behavior-preserving, own commit | integrating executor | 1 | done: 183 `reflection_probes` tests pass, R6 still red |
| 3 | Carried kept depth through `cross` and `trace_counting`, checked at each read point; read-kind seam rows; `STAGE_VERSION` bump | integrating executor | 2 | done: R6 green; read-kind seam row green; 185 `reflection_probes` tests pass; `STAGE_VERSION` 12 → 13 |
| 4 | Later-hull row, kept-behind at-depth assertion, off-box R6 variant | integrating executor | 3 | done: all green; R6 and later-hull rows also assert the uncarried trace reads the wall behind |
| 5 | No-leak oracle over generated layouts, and its disabled-check run | integrating executor | 3 | done: green with 21 rays named as known limits (see Known limits); 194 `reflection_probes` tests pass |
| 6 | Cache and bytes check on `reflection-probes-test.map` | integrating executor | 3 | done: pre-change binary is main 0297abebe. Warm over its cache: `reflection_probes` misses, the other 12 cached stages hit. Cold `--release` before and after: byte-identical, all 36 sections; id 54 (148 630 B) did not move |

## Oracle record
- Seeds 0..160, one generated layout each, 24 rays each (3 840 rays), plus R6 and kept behind as fixed cases. Three tests, each about 4 s in the default test profile, run in parallel with the rest of the suite.
- Generator: an excluded-or-kept block over map x 100–116, its bottom 0.3, 0.8, 1.2 or 1.6 mm over the platform; two to four platform pieces under it, butted, 0.8 mm (CSG contact), 1.6 mm (past capture contact) or 0.5 u apart, tops −0.5 to +0.7 mm off nominal; an optional near brush. Rays cross x = 100 from −0.3 to 2.2 mm over the platform, slopes 0.001–0.03 log-uniform.
- The generator passes over at most one kept depth per ray, at the block's exit. Rows pin the rest of the class: a later hull, a pass beside, a second hull of one classification, and an open exit past the depth. Mutation-checked: last-depth-wins, first-hop-only and a deleted open-exit arm each fail a row.
- Carried-depth check enabled: 21 rays refused, all named below. Each must still leak and reach its read carrying no depth, asserted by a trace that refuses any carried depth, so none is a class case. Disabled: those 21, R6, and the 10 generated class cases the fix closes. The carried depth changes no read the oracle would accept; that test shares the slack with the trace, so the kept-behind row stays the proof at the depth, and kept behind joins the oracle as a fixed case, pinned as a ray that carries a depth and reads.
- Named known limits: the 21 rays below, in `KNOWN_LIMITS`, skipped per ray so the rest of their layouts stay checked. The oracle fails on an unnamed leak, and on a named ray that stops leaking or starts carrying a depth. A named ray's group is not machine-checked. The disabled-check test counts only unnamed rays, so it still proves the generator reaches the class.

## Known limits
The oracle found leaks outside R6's class, which stopped the build under Decision "Leaks outside R6's class are recorded, not chased". Owner, 2026-11-03: record them as §5.2 known limits with their constructions, skip them in the oracle by name, and give them a roadmap entry (`reflection-probe-capture-exclusion known leaks`). That covered groups A–D as first reported, 13 rays. Review then found group E and a second D ray, 8 more, recorded under the same Decision's rule for review findings. All 21 predate this change. Grazing rays in sub-mm gaps, as the class is.

- **A. Coplanar pass stepped over** (seed 20 rays 7, 8, 23; seed 43 rays 4, 22; seed 150 ray 16). An excluded near brush's back and the excluded block's front share map x = 100. The ray crosses the near brush and leaves it there into open space; its tolerance hit on the block's front lies one `f32` ulp before that exit (seed 20: 0.99999994 vs exit 1.0). The re-trace restarts at the exit (`min_distance = (exit as f32).max(skipped.distance)`), past the hit. Classified, it would be a pass beside the block whose exit kept solid holds: the leak guard. The ray reads kept solid behind through faces CSG removed.
- **B. Tied entry never classified** (seed 29 rays 0, 4, 9, 15, 16). The ray takes hits on the excluded block and an excluded piece at one `f32` distance, at least one a real entry; hit order takes the piece. Rays 0, 4, 9, 15 enter the block, and its exit, held by the piece and a kept piece, is never examined. Ray 16 lies under the block and enters the piece's front, so what is lost is the pass beside the block.
- **C. Excluded neighbour passed over** (seed 57 ray 8). Two excluded pieces hold the block's exit; the nearer is taken and the other dropped, as R6 drops kept solid. The ray later enters the dropped piece through its top CSG removed under the block, unseen, and leaves it into a kept piece through a face CSG removed against it. R6's sibling with an excluded depth in place of a kept one.
- **D. Exposed CSG hole** (seed 23 ray 16; seed 64 ray 7). With an excluded brush gone, the ray reaches a kept piece's front, which CSG removed against an excluded piece the ray passes less than 0.5 mm over without a hit (no front-facing plane near it), and reads through.
- **E. Unseen excluded hull** (seed 20 rays 0, 18; seed 43 rays 5, 10, 15, 18; seed 48 ray 10). The ray leaves an excluded near brush into open space, then enters an excluded piece through a face CSG removed, taking no hit, so the trace never classifies it. It leaves that piece into a kept piece through a face CSG removed against it, and reads what lies behind.
