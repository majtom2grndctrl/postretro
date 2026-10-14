# lighting-scale--lightmap-layer-budget-gate — plan of record

mode: compact
status: blocked
read at: e00493c95

## Corrections
- Relevant compiler, format, render-cpu placement and loader portal symbols are unchanged since 45c4fe5. Other renderer changes concern SH streaming and do not alter the cited lightmap boundaries.
- The brief has 33 automated rows and four manual rows; all remain required. Manual proof blocks landing because the brief does not permit landing first.

## Delegated answers
- Placement worker found AC14 cannot be proved literally: `LightmapHeader::block_alignment` is `lcm(4, direction_texel_scale)` and `LightmapBlockIndex::from_prefix` calls `validate_record`, which rejects nonmultiple extents. At scale 8 no decoded id-22 record can have the requested unaligned block extents. Serialization worker independently confirmed this. No Decision premise was found false.

## AC-to-proof

| AC | Acceptance | Proof | Status |
|---|---|---|---|
| 1 | A fixture whose blocks pack into more than 256 bake layers compiles (a small test pool edge keeps it cheap). Its planned chart area alone exceeds 256 pool-edge layers, so both the pack and the pre-cut area check would have failed before the change. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 2 | A budget exactly at the predicted working set compiles. One byte below refuses before the lightmap stage bakes: before the shadowmask memo probe, the cut-face estimate and any lightmap partition (pin P1). The error names the predicted bytes, the budget, the fully packed layer count and the flag. End to end, the fixture's SH-delta projection sits below its lightmap prediction, so the shared flag reaches the lightmap gate (pin P12). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 3 | The gate refuses and permits at the same budget with `--no-cache` and with a warm cache whose lightmap section and shadowmask memos both hit. A warm refusal reads no lightmap or shadowmask cache entry (pin P1). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 4 | The SH-delta gate still refuses and permits at its own projection when driven by the shared flag. Permits is read from its admitted line, even where the lightmap gate later refuses (pin P12). The old flag is rejected as unknown. No `sh-delta-working-set-max-size` remains in `crates/` or `context/lib/`, ignored tests included. Grep gate. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 5 | Before any face is cut, an area whose minimum cost exceeds the budget refuses by name; one whose minimum cost equals the budget passes to cutting (pin P11). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 6 | A face whose cut grid windows mostly miss its polygon is counted by its polygon area: a layout the gate admits is not refused before cutting, though its window area alone would exceed the budget (pin P14). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 7 | The pathological scale regions of pin P11 fail by name before face cutting at the default budget, each in under 5 s. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 8 | On the over-256 fixture and the synthetic cut fixture, with compressed and uncompressed irradiance, the pre-cut minimum never exceeds the predicted working set (pin P14). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 9 | A fused stage with no static lights, or with no placed chart, predicts zero bytes (pin P8). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 10 | A map without static lights plans no placements and emits no animated chunks, whatever its chart area. It compiles at any budget its SH-delta projection admits, the lightmap gate never refusing it (pin P8). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 11 | A map without static lights but with animated lights compiles without a warning that names its cell blocks or the runtime limits as the reason it has no animated chunks. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 12 | The pool-layer check, given block extents and alignment that place into 257 pool layers at the runtime pool edge, refuses by name for a level that cannot stream and warns by name for one that can, each giving 257 and the floor. At exactly 256 neither fires, on a layout whose bake-layer count exceeds 256. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 13 | One cheap compile shows the check runs on the packed layout and reads whether the compiler emitted id 51: under a test floor, a fixture without usable portals refuses and the same fixture with them warns. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 14 | On a synthetic layout with direction texel scale 8 and block extents that are not multiples of 8, spanning several pool layers, the compiler's pool-layer count equals the renderer's all-resident placement run on the id-22 index decoded from the same section. Not `#[ignore]`. | Blocked: format decoder rejects the required synthetic section | needs restatement |
| 15 | A warm rebuild that hits the lightmap section memo logs the same pool-layer warning, or refuses with the same error, as the cold build (pin P13). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 16 | A level with no static lights whose charts, packed, would need more than 256 pool layers compiles with neither the warning nor the refusal, whether or not it can stream: its id 22 has no blocks. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 17 | The residency dry run still labels layouts against a 256-layer runtime floor. The check and the dry-run label read one named constant, and no compiler pack or area check reads it. Grep gate. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 18 | `postretro-level-compiler` has no normal dependency on `postretro-render-cpu`; `cargo tree -p postretro-level-compiler -e normal` lists neither it nor `postretro-entities`. Grep gate. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 19 | `render-cpu`'s all-resident placement entry point keeps its name and signature, and the existing `render-cpu` lightmap-pool and renderer static-pool planning tests pass unchanged. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 20 | Before the first slice lands, id-22 and id-42 digests are recorded from a cold bake of every `GATE_FIXTURES` map and of the existing synthetic four-layer cut fixture. After the change they match, cold. The cross-layer cut case is that synthetic fixture; nothing joins `GATE_FIXTURES`. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 21 | One `#[ignore]` loop over `GATE_FIXTURES` bakes each map once and checks id 22, id 42 and the id-22 cache entry together, a warm rebuild matching cold. No row adds a second `GATE_FIXTURES` loop. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 22 | On a fixture of at least three bake layers and no cut face, live raw-fill bytes peak at exactly one bake layer's raw bytes, on the compose pass and on the lightmap-hit, shadowmask-miss pass (pins P2, P7). The pre-change fill peaks at every layer and fails it. The peak is measured across every worker thread the consumer runs on, scoped to one bake. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 23 | On a cut fixture, live raw-fill bytes peak at no more than one bake layer plus the raw bytes of the cell blocks holding sub-faces of cuts still open (pins P2, P3, P4, P7). A cut whose sub-faces share one layer adds nothing. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 24 | On a one-layer map the first layer is the last: its fill closes once, and its bytes match pre-change. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 25 | Owner assignment and drop reports for a cross-layer cut face match pre-change output. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 26 | Pins P2–P7 each hold on a synthetic fixture: id 42, owners and drop report match the pre-change digests, and the live raw-fill bound holds. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 27 | Live raw-fill bytes sampled at the id-22 cache write are at most one bake layer plus the carry of cuts ending on the last layer (pin P9). | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 28 | Two prepared atlases that differ only in bake-layer count, with no cut face, predict the same shadowmask term. With cuts, the term grows by the largest set of cell blocks held open by cuts at once. It also charges the per-layer encode scratch, which now runs during the walk. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 29 | A build on an empty cache writes the id-22 entry through the streamed path once. No single write into the entry is larger than the larger of the index bytes and one block's irradiance plus direction bytes, so a whole-section buffer fails. A `--no-cache` build writes no entry. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 30 | The streamed id-22 entry equals the contiguous entry, header included, on a synthetic multi-block section, outside `#[ignore]`. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 31 | A cold build writes ids 22 and 42 into the PRL file without a contiguous serialized copy of either section; the file's bytes match the pre-change digests. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 32 | The PRL read-back validates ids 22 and 42 without reading either section into one whole buffer. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 33 | The predicted working set counts each encoded section once. | Focused compiler/format/placement regression; final preflight | achievable as stated |
| 34 | Hallway map at 0.025, `--no-cache`, release profile: the compile succeeds. Record bake-layer count, runtime pool-layer count, predicted and measured lightmap-stage peak, and wall time. Name the SH stages as the upstream limit on total wall time and RSS. | Owner/manual stage RSS or engine run; see runbook below | manual |
| 35 | Hallway map at 0.04: measured lightmap-stage peak no higher than the baseline. | Owner/manual stage RSS or engine run; see runbook below | manual |
| 36 | The 0.025 output loads in the engine with lightmap streaming on. Lightmaps render, with no placeholder-lightmap error. | Owner/manual stage RSS or engine run; see runbook below | manual |
| 37 | A compile that warned, loaded with `POSTRETRO_LIGHTMAP_STREAMING=all-resident`: the placeholder renders and the renderer's rejection names the pool-layer count the warning gave. Use the hallway at 0.025 if it warns, else a fixture past the floor. | Owner/manual stage RSS or engine run; see runbook below | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Capture pre-change cold id-22/id-42 digests for every GATE_FIXTURES map and four-layer cut fixture, then implement per-layer shadowmask close and cut carry with cross-worker scoped memory proof | shadowmask worker + integrating executor | — | pending |
| 2 | Move all-resident placement below compiler and render-cpu without behavior changes; share named runtime floor and verify placement consumers | placement worker + integrating executor | baseline capture | pending |
| 3 | Stream id-22 cache write, ids 22/42 PRL writes and read-back; verify exact bytes and bounded writes | serialization worker + integrating executor | baseline capture | pending |
| 4 | Replace cap and pre-cut area refusal with shared working-set budget; gate cold/warm before memo and bake, price per-layer/carry and one section copy, skip unconsumed no-static layouts; enforce runtime floor from emitted id 51 | integrating executor | 1, 2, 3 | pending |
| 5 | Focused readiness, review-panel/fix loop, final preflight once, and complete AC evidence table | integrating executor | 4 | pending |
| 6 | Manual hallway release-stage RSS/compile and engine loading proofs | integrating executor / owner | 5 | pending |

## Manual runbook
- Pin baseline 45c4fe5 and implementation, machine class, worker count, release profile, density, cache mode and exact command. Sample process RSS across the lightmap stage only. Earlier SH stages remain the upstream wall-time and RSS limit; never substitute whole-process peak.
- Hallway density 0.025, --no-cache: record bake layers, runtime pool layers, predicted stage bytes, sampled stage RSS peak and wall time; compile must succeed.
- Hallway density 0.04: compare stage peak to the same baseline machine/flags; must not increase.
- Load 0.025 PRL with streaming: lightmaps render without placeholder error.
- Load a compiler-warning layout with POSTRETRO_LIGHTMAP_STREAMING=all-resident: placeholder renders and renderer names the same pool-layer count.
- Delete session outputs and cache after measurements. Outstanding manual rows never become inferred passes.

## Blocking acceptance clarification

AC14 currently reads:
> On a synthetic layout with direction texel scale 8 and block extents that are not multiples of 8, spanning several pool layers, the compiler's pool-layer count equals the renderer's all-resident placement run on the id-22 index decoded from the same section. Not `#[ignore]`.

Evidence at e00493c95 (unchanged since the brief reference):
- `crates/level-format/src/lightmap.rs:105`: `block_alignment` returns `lcm(4, self.direction_texel_scale.max(1))`.
- `crates/level-format/src/lightmap.rs:251` and `:273`: the prefix decoder derives that alignment and validates every record against it.
- `crates/level-format/src/lightmap.rs:315`: `validate_record` rejects width or height not divisible by alignment, naming the extent and required multiple.
- `crates/level-format/src/lightmap.rs:849`: existing `rejects_misaligned_extent` regression enforces this contract.

A scale-8 id-22 section with nonmultiples of 8 cannot decode. Silently rounding the serialized section would test different extents from the literal row. Changing the format or loader would violate the brief's no-format/loader-change Decision.

Proposed replacement (owner approval required):
> On synthetic placement inputs with direction texel scale 8 and block extents that are not multiples of 8, spanning several pool layers, the shared placement produces the same pool-layer count as those extents rounded up to alignment 8. Separately, on a valid scale-8 id-22 section spanning several pool layers, the compiler's pool-layer count equals the renderer's all-resident placement run on the index decoded from that same section. Both checks are non-ignored.

Resume: after the owner approves wording, update only AC14 as authorized, re-run its feasibility check, restore `status: active`, and resume task 1 from baseline capture. No baseline digests were captured and no implementation has landed. The baseline test build was interrupted before tests ran; temporary harness edits are removed. All other Acceptance rows and Decisions remain unchanged.
