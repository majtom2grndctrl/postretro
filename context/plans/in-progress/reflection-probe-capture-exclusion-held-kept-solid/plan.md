# reflection-probe-capture-exclusion-held-kept-solid — plan of record

mode: compact
status: active
read at: cc660f45b (no source under `reflection_probes/` changed since the brief's ffd61b87b; Decision reads reused)

## Corrections
- Path names `held_at`, `Held`, `nearest_held` and `ProjectionBox` for the split → `held_at` calls `outside_by`, which `capture_fill` also uses; it moves with them so the new module owns the hull-distance rule. The carried depth's type lives there too. Module: `capture_bounds.rs` (what bounds an excluded trace's reads).
- The brief says "builds after `watertight-tracer` lands on main" → landed (2f94024ec, merge of #638); this branch is cut from main after it.

## Delegated answers
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
| 3 | Carried kept depth through `cross` and `trace_counting`, checked at each read point; read-kind seam rows; `STAGE_VERSION` bump | integrating executor | 2 || done: R6 green; read-kind seam row green; 185 `reflection_probes` tests pass; `STAGE_VERSION` 12 → 13 |
| 4 | Later-hull row, kept-behind at-depth assertion, off-box R6 variant | integrating executor | 3 | |
| 5 | No-leak oracle over generated layouts, and its disabled-check run | integrating executor | 3 | |
| 6 | Cache and bytes check on `reflection-probes-test.map` | integrating executor | 3 | |
