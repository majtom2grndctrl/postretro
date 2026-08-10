# review-draft-spec findings — delta-SH probe coarsening

Three independent lenses (broad / codebase-anchor / temporal), Opus, run on the
committed draft. They converge on the same architectural cluster — the signal
worth acting on. Not yet applied: these reshape Tasks 1/2/5, Wire format, and
Invariants, so they are surfaced for an owner decision, not auto-applied.
`review-implementability` is deliberately NOT run yet (its findings key to task
paragraphs that this cluster will rewrite).

## Warrants verified TRUE (banked, not re-litigate)

Anchor + broad both confirmed against source: the popcount/rank payload identity
(`valid_probe_mask_payload_f16_count`, `DeltaView::resolve_probe_f16_offset`);
the 0/3/1 free-storage-slot budget (id27 at the 8-ceiling, `compose_layout_keeps_eight`);
id-27 validity recoverable from `probe_indirection`; the reconstruction fns exist
with the claimed roles; the section version consts and `validate_wire_contract`
(id45 only); `ProtectAabb`/`intersects_any`, `resolve_trigger_volume`,
`enforce_payload_cap`, the CLI flags; and the pipeline insertion window
(`apply_exact_zero_drop_policy` → `apply_valid_probe_compaction` → `enforce_payload_cap`).

## Blockers (implementer would build the wrong thing)

- **A — Reconstruction lattice is intra-brick, not cross-boundary** (temporal F3, broad #1). "Corners cross affinity-cell boundaries" (Task 1/2, Invariant 3) contradicts `reconstruct_l1_tile`'s own `{0,AF-1}³` corners, which never leave the brick — and which is what the classifier measured error against and what makes seam-smoothing coherent. Fix: reconstruction reads the brick's own kept corners; delete "crossing affinity-cell boundaries" everywhere. (Also removes false work.)
- **B — L2 stores a synthesized brick-mean, not a copyable probe** (temporal F2, broad #2). `reconstruct_l2_tile` is the mean over valid probes; a copied original probe fails the AC3 match. So the "reuses the copy loop, no wire change" warrant holds for L0/L1 only; L2 is a compute-and-write. Fix: pin L2 to write one recomputed mean tile at a named slot/rank; amend Prior-commitment (1) to L0/L1-only.
- **C — The mask predicate is shared across all three sections and has no level channel** (anchor-1, broad #4, temporal F1). `valid_probe_mask_for_affinity_cell(base, affinity_dims, cell)` is called by `compact_indirect/direct/animated_valid_probes`, derives from base validity only, and runs once on dense data (asserts dense). "Refine the id-27 predicate after compaction" is mis-scoped (touches all three), understated (level must be plumbed), and self-contradictory (dense assertion vs after-compaction). Fix: classification produces a per-cell level array threaded into the compaction mask stage; classification completes before the single dense compaction pass consumes the payload; the seam is shared, not id-27-only.
- **D — Reconstruction math is private + measurement-only + cross-crate** (anchor-2, broad #3). `reconstruct_l1_tile`/`reconstruct_l2_tile`/`corner_locals`/`trilinear_weight` are private fns in `sh_analyze.rs`, a module doc-marked "never touches an emitted `.prl` byte"; `render-cpu` does not depend on `level-compiler`. AC3's cross-crate equivalence has no home. Fix: relocate the math to a crate both depend on (`level-format` or a lighting crate), make pub; state where the equivalence test lives.
- **E — The gate needs a two-phase map-wide magnitude pass** (temporal F8). The relative gate divides by local magnitude and the darkness floor is 2% of *map* p95 — a global statistic a streaming classifier can't have mid-build; near-black bricks explode the ratio and force L0 unless the floor short-circuits first. Fix: phase 1 computes per-brick magnitude + map p95; phase 2 gates, floor-check before the relative division.
- **G — Per-section vs shared classification is unstated** (broad #5). ids 27/41/45 carry independent masks and different composed content (indirect / direct-subtract / direct-add); a level fine for one need not be for another. Fix: state levels and seam smoothing are computed per section over the shared affinity grid.

## Complicates (guessable, might guess wrong)

- **F — Unevaluable-level sentinel** (temporal F9): a level whose reconstruction is `None` must score `+∞`, never 0; a brick with no valid corners is never L1; no valid probes → never coarsened. State it.
- **Seam-smoothing mechanics** (temporal F5/F6/F7): it is a fixpoint loop (repeat sweeps to zero demotions; monotone → terminates, ≤2 demotions/brick), not a single pass; protection-force-L0 runs *before* smoothing (rewrite Task 5's local wording, which reads as smoothing-right-after-classification); each violating pair demotes the *coarser* endpoint.
- **Cap single-shot** (temporal F10): classification+protection+smoothing+refinement run exactly once; cap overflow fails the build with no re-coarsening; `--sh-coarsen off` enforces the same cap.
- **No adjacency helper** (anchor-3): `affinity_grid.rs` has none; face-adjacency must be derived from the x-fastest layout.
- **FGD path untested** (broad #6): AC5 feeds a raw AABB, bypassing FGD parse → brush-union → dilation. Add an AC for the mapper-facing path end to end, or state AC5's raw-AABB stand-in is the only coverage.
- **Goal overstates the shrink** (broad #7): only arena 8 m / ~12% is hard-measured; ~65% at 1.5 m is proxy-extrapolated (the spec's own Open Questions concede this). Label ~65% provisional.

## Nits

- Casing: `id-27` / `id 27` / `id27` mixed — pick one.
- `--sh-coarsen off … bakes uniform grid unchanged` is an untested regression claim; add an equivalence assertion or demote to non-normative.

## Pin table (temporal reviewer — fold into the spec's ordering section; the test task cites rows)

| # | Scenario | Ordering under test | Expected outcome the spec must state |
|---|---|---|---|
| P1 | `--sh-coarsen` on | coarsening as a second compaction pass on already-compacted data | Must not hit the dense-payload assertion; coarsening feeds the level into the single dense compaction, or operates rank-indexed on compacted data |
| P2 | L2 brick | mask refinement reduces payload | Emit one synthesized mean tile (`reconstruct_l2_tile` over the valid set), computed before valid tiles are dropped, at a defined rank |
| P3 | A=L1, +x B=L2 | reconstruct A near-face from a shared corner owned by B | (Resolves under A: reconstruction is intra-brick, so no cross-brick corner borrow) |
| P4 | in-brick lattice | classifier error vs runtime reconstruction | Both use identical intra-brick `{0,3}³` trilinear |
| P5 | X(L2)–Y(L2)–Z(L0) in x | single sweep vs fixpoint | Fixpoint loop; final state has no diff≥2 pair |
| P6 | protected P adjacent N; P=L2,N=L2 | protection after smoothing | FORBIDDEN; protection forces P→L0 before smoothing |
| P7 | pair with coarser endpoint as +neighbor | demote-current vs demote-coarser | Demote the coarser endpoint |
| P8 | dark map (mag≈1e-4) | gate before vs after map-p95 pass | Two-phase; sub-floor bricks bypass the relative gate; map doesn't bake dense |
| P9 | all 8 corners invalid, some interior valid | classifier picks level | L1 = +∞ ineligible; L2 or L0; never L1 |
| P10 | zero valid probes | classify/reconstruct/smooth | Excluded; non-participating in smoothing |
| P11 | valid probes all non-corner | L1 evaluability | L1 unevaluable; L2 only coarsening |
| P12 | cap exceeded after coarsening | cap handling | Build FAILS; no re-classification, no forced global L2; ran once |
| P13 | `--sh-coarsen off` | uniform bake | Bakes uniform, enforces cap, fails if unmet |
| P14 | protected brick, classifier would pick L2 | classification → protection | Protection overwrites chosen level to L0; feeds smoothing + refinement |
