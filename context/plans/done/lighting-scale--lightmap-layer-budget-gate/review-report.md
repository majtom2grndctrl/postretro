# Review panel — lighting-scale--lightmap-layer-budget-gate

Target: current implementation against `e00493c95`, including staged changes. Verdict: **request changes**.

Coverage: three slices, each with a separately bucketed Hygiene + Drift pass. Budget/atlas/CLI received an adversarial tester and contract verifier; shadowmask received an Astra correctness tracer and Sol adversarial tester; persistence/runtime received a contract verifier and correctness tracer. A final Astra correctness tracer followed the complete admission → fused fill/carry → cache → PRL → readback seam. Depth used xhigh effort; hygiene used Sol medium. Reviewers ran no builds, tests, edits or commits. Manual AC34–37 remain pending and outside this code verdict. Approved AC14 clarification was preserved.

## Code findings

### 🔴 Omitted layer-by-light metadata defeats the working-set gate

Anchor: `crates/level-compiler/src/pipeline/lightmap_stage/peak.rs:91` (seam tracer).

The estimate prices the bounded partition window but omits the eagerly collected `(layer, light)` item vector at `pipeline/lightmap_stage.rs:369–371`, which remains resident through the walk. A valid layout of 1,024 separate 40×40 cell blocks, direction scale 2, no cuts, and 10,000 non-SDF static lights produces 1,024 64² bake layers and stays below the runtime pool floor. The predicted working set is 6,565,888 bytes and admits a 7 MiB budget; the item vector alone then needs at least 163,840,000 bytes on 64-bit platforms. Cached builds additionally retain layer/light hash tables at `pipeline/lightmap_stage/partitions.rs:47–53` and `pipeline/lightmap_stage.rs:190–209`. This is separate from the explicitly accepted warm memo-decode overhead.

Fix: derive items lazily and bound hash metadata, or charge the layer-by-light allocations using light counts before admission. Preserve cache-pair semantics. Add a regression showing metadata is bounded or included in the prediction before allocation.

### 🔴 Sparse scaled faces enumerate billions of empty cut windows

Anchor: `crates/level-compiler/src/lightmap_bake/atlas_layout.rs:163` (adversarial tester).

A static-lit triangle with vertices `(0,0,0)`, `(0.001,0,0)`, `(1000,0,1000)`, density `0.04` and an enclosing scale region of `4000` has a compressed polygon minimum of approximately 4,999,999,837 bytes, below 16 GiB. Its chart is 100,000,000², yielding 49,020 pieces per axis. `face_cut.rs:30–38` and `:233–234` enumerate 2,402,960,400 windows and allocate/clip empty polygons before packing or final admission. The retired area cap refused this input before cutting.

Fix: keep the polygon lower bound, prune to conservative polygon-intersecting candidates in the existing row/window order, and retain exact clipping for each candidate. Prove bounded candidate work with a focused sparse-scale regression. Do not restore empty-window area refusal or introduce a new required admission gate.

### 🟡 No-static maps still reject unused chart dimensions

Anchor: `crates/level-compiler/src/lightmap_bake/atlas_layout.rs:157` (adversarial tester).

A unit quad with no lights, density `0.04` and a valid enclosing `_lightmap_scale = 1e9` reaches approximately 25 billion texels per axis and fails `ChartDimensionOverflow` at `charts.rs:186`. Its SH projection is zero, and no placements or animated chunks will consume these dimensions. The sizing behavior predates this diff, but the new AC10 promise “whatever its chart area” remains unmet.

Fix: bypass density-dependent dimensions for the no-static path, retaining safe per-face placeholders/leaf ids as needed. Test a finite valid scale beyond the chart dimension representation.

### 🟡 Retired option is accepted as a positional input when first

Anchor: `crates/level-compiler/src/main.rs:1101` (CLI contract verifier).

AC4 requires the old flag to be rejected as unknown. The positional fallback accepts any first unmatched token as `Args.input`. `prl-build --sh-delta-working-set-max-size 16GiB input.map` therefore reports `unexpected argument: 16GiB`; parsing the retired flag alone succeeds with the flag as its input path. The map-first case rejects the intended flag, so the current test misses the order difference.

Fix: prevent option-prefixed tokens from taking the positional fallback. Test the retired flag before and after the map, asserting the error names that flag.

## Comment drift

### 🟡 Cut-owner release description uses the old lifetime

Anchor: `context/lib/build_pipeline.md:405` (Hygiene + Drift).

Live text: “an estimated owner that bakes nothing is cleared after the walk.” The new implementation releases it when its last sub-face layer closes (`shadowmask_bake/fill/layers.rs:55`), potentially before later layers. This also contradicts the corrected preceding paragraph. Replace “after the walk” with “when its last sub-face layer closes.”

## Clean coverage

Independent lifetime and adversarial reviews found no shadowmask carry, owner ordering, shared-block retention, byte preservation or probe-accounting defect. Persistence verification found unchanged wire bytes, correct streamed cache framing/hash and bounded PRL/readback operations. Runtime tracing verified finalized layout/decoded-index placement parity, unchanged allocator behavior, shared floor use, id-51 portal validity and retained-file streaming prerequisites. Aside from the findings above, gate ordering and equality/warm behavior matched the brief.

## Repair verification
- Metadata finding: repaired with lazy work indexing, streamed legacy-order hashes and cache-independent source-count pricing. Per-stage cache-use tables isolate growth from earlier SH records. Focused independent source review found no new consequential defect; cache keys, empty pairs, journals, reports and pruning retain their semantics. Focused runtime verification remains pending.
- Documentation lifetime: corrected to the last sub-face layer close.
- Sparse-cut/no-static/CLI findings: repaired and approved on focused independent source review. Exact first-axis clipping bounds inclusive row candidates; ordered sweep preserves outer rows, boundary pieces and final clipping. No-static placeholders retain face/leaf correspondence. Unknown option errors name the retired option in all tested orders. No new consequential defect found.
- All four code findings and the documentation correction are addressed. Focused execution passed for all repaired paths, including explicit cold/warm frozen bytes. Final formatting and strict lint pass. Full workspace testing is recorded in plan.md; two unrelated pre-existing content expectations still fail.
- Full verification exposed two fixture assumptions invalidated by the approved no-static Decision. Source diagnosis confirmed the production empty-chunk guard. Adapted fixtures without weakening assertions; animated33 and runtime-floor2 focused tests and final compiler workspace target pass.
