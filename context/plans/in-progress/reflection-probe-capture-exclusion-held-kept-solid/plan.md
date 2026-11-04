# reflection-probe-capture-exclusion-held-kept-solid — plan of record

mode: compact
status: blocked
read at: cc660f45b (no source under `reflection_probes/` changed since the brief's ffd61b87b; Decision reads reused)
re-checked at: main 0297abebe merged (2026-11-03). Main's only changes under `reflection_probes/` and `ray_traversal/` are visibility and a test-harness wrapper. After the merge: R6 and later-hull rows green (and still red uncarried); oracle refuses the same 13 rays (A–D below, same seeds); the disabled-check oracle test still fails as required; every other level-compiler test passes.

## Corrections
- Path names `held_at`, `Held`, `nearest_held` and `ProjectionBox` for the split → `held_at` calls `outside_by`, which `capture_fill` also uses; it moves with them so the new module owns the hull-distance rule. The carried depth's type lives there too. Module: `capture_bounds.rs` (what bounds an excluded trace's reads).
- The brief says "builds after `watertight-tracer` lands on main" → landed (2f94024ec, merge of #638); this branch is cut from main after it.

## Delegated answers
- Oracle's "a hull it crossed or passed beside" → an excluded hull whose post-cut faces the ray takes a hit on before its read (a real entry or a tolerance hit), asked of the tracer with every other face set aside. §5.2 defines a pass beside as a tolerance hit; a ray that takes no hit on a brush reads the same with it kept or excluded. Also: kept solid the ray is already inside at its first excluded hit is not counted, since up to that hit the ray is the unmasked one (`research.md` §Audit). The bound itself comes from brush planes only. Both narrowings removed false positives where the unmasked capture reads the same (seeds 0, 5, 12, 68 in early runs).
- Slack at the held depth — τ (`TRACER_TOLERANCE_METERS`, 0.5 mm) along the ray. It covers the `f32` hit distance (60 µm at 1 km) and the mesh's `f32` corner rounding (15 µm at ±500 m) seen along a ray up to ~88° off the entry face's normal, and stays a third of `CONTACT_TOLERANCE_M`. The oracle uses the same constant.
- Read-point check, or clamped re-trace reach plus a fill check — a check at each read point (kept hit in `settle`, fill in `filled`, `Exit::Open`), through one function the trace calls. One seam then covers all three read kinds and is what the read-kind rows pin; a clamped reach would split the rule between the tracer's reach and a separate fill check.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| R6 layout | `r6_*` row in the new sibling test module; fails pre-fix with a hit on the wall behind | achievable as stated |
| A later hull | split-N row in the same module | achievable as stated |
| Every read kind, both sides | seam unit test on the carried-depth check: hit / fill / open exit, at depth and past it | achievable as stated |
| Kept-behind row | existing row, plus an assertion that its front lies at the held depth | achievable as stated |
| Split-platform, exact-tie, selection-rule, beside budget, crate-seam, all capture/fill tests unchanged | `cargo test -p postretro-level-compiler --bin prl-build reflection_probes` | achievable as stated |
| Off-box invariant | existing seam test + R6 layout with the box short of the kept brush, kept vs excluded | achievable as stated |
| No-leak oracle | generated-layout test, plane-derived bound | achievable as stated |
| The oracle sees the class | same oracle with the carried-depth check disabled (test-only switch): fails on ≥1 generated case and on R6 | achievable as stated |
| Oracle in default `cargo test`; seeds, count, runtime, known limits recorded here | not `#[ignore]`; recorded below | achievable as stated |
| Capture stage version +1, tracer epoch unchanged | diff | achievable as stated |
| Warm build over pre-change cache misses capture, hits tracing stages before it; cold differs in id 54 at most | prl-build on `reflection-probes-test.map`, pre-change binary vs this branch | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | R6 row alone, before any fix: falsify the premise (builder gains a no-near-brush parameter and per-brush tops) | integrating executor | — | done: preconditions hold; on cc660f45b the row fails with `Hit` on the wall behind at 1.711 m (map x = 128). Premise holds |
| 2 | Split `held_at`, `Held`, `nearest_held`, `outside_by`, `ProjectionBox` into `capture_bounds.rs`, behavior-preserving, own commit | integrating executor | 1 | done: 183 `reflection_probes` tests pass, R6 still red |
| 3 | Carried kept depth through `cross` and `trace_counting`, checked at each read point; read-kind seam rows; `STAGE_VERSION` bump | integrating executor | 2 | done: R6 green; read-kind seam row green; 185 `reflection_probes` tests pass; `STAGE_VERSION` 12 → 13 |
| 4 | Later-hull row, kept-behind at-depth assertion, off-box R6 variant | integrating executor | 3 | done: all green; R6 and later-hull rows also assert the uncarried trace reads the wall behind |
| 5 | No-leak oracle over generated layouts, and its disabled-check run | integrating executor | 3 | blocked: oracle refuses 13 rays outside the class (see Block) |
| 6 | Cache and bytes check on `reflection-probes-test.map` | integrating executor | 3 | |

## Oracle record
- Seeds 0..160, one generated layout each, 24 rays each (3 840 rays), plus R6 as a fixed case. Runtime 1.4 s per oracle test in the default test profile (both tests together ≈ 2.7 s).
- Generator: an excluded-or-kept block over map x 100–116, its bottom 0.3, 0.8, 1.2 or 1.6 mm over the platform; two to four platform pieces under it, butted, 0.8 mm (CSG contact), 1.6 mm (past capture contact) or 0.5 u apart, tops −0.5 to +0.7 mm off nominal; an optional near brush. Rays cross x = 100 from −0.3 to 2.2 mm over the platform, slopes 0.001–0.03 log-uniform.
- Carried-depth check disabled: 23 generated rays refused plus R6. Enabled: 13 refused, every one with no kept depth carried at any exit (traced) and the same on the uncarried trace. The fix closes 10 generated class cases and R6.

## Block
The oracle finds leaks outside R6's class. Decision "Leaks outside R6's class are recorded, not chased": a failing oracle case outside the class stops the build for the owner. All 13 predate this change (the uncarried trace reads them identically, and `held` stays unset through each trace). Grazing rays in sub-mm gaps, as the class is. Groups, by construction (ray = crossing of map x = 100, over the platform, and slope):

- **A. Coplanar beside pass stepped over** (seed 20 ×3, seed 43 ×2, seed 150). An excluded near brush's back and the excluded block's front share map x = 100. The ray crosses the near brush and leaves it there into open space; its tolerance hit on the block's front lies one `f32` ulp before that exit (seed 20: 0.99999994 vs exit 1.0), and `closest_hit_masked`'s open lower bound skips it. Classified, it would be a pass beside the block whose exit kept solid holds: the leak guard. The ray reads kept solid behind through faces CSG removed.
- **B. Tied entry never classified** (seed 29 ×5). The ray enters the excluded block's front and takes a tolerance hit on an excluded piece's front at the same `f32` distance; hit order takes the piece, which it then crosses (it enters through the top CSG removed). The block's own exit, R6-shaped (the piece and a kept piece both hold it), is never examined.
- **C. Excluded neighbour passed over** (seed 57). Two excluded pieces hold the block's exit; the nearer is taken and the other dropped, as R6 drops kept solid. The ray later enters the dropped piece through its top CSG removed under the block, unseen, and leaves it into a kept piece through a face CSG removed against it. R6's sibling with an excluded depth in place of a kept one.
- **D. Exposed CSG hole** (seed 23). With the block excluded, the ray reaches a kept piece's front, which CSG removed against an excluded piece the ray passes 0.06 mm over without a hit (no front-facing plane near it), and reads through.

Owner decides: record A–D as §5.2 known limits with their constructions and add their layouts to the oracle as named known limits, skipped with their reasons (the rule the brief sets for review findings); or widen scope to fix some (A looks local to the re-trace bound; C is the class's excluded-depth sibling); or something else. Branch state: `generated_layouts_never_read_through_kept_solid_touching_a_met_excluded_hull` is red on purpose; everything else in `reflection_probes` is green.
