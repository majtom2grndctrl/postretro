# Lighting Scale — Delta-SH Probe Coarsening

> **Status:** draft, revised after review-draft-spec (3 lenses) — findings in `review-findings.md`, gating-spike evidence in `context/research/coarsening-gating-spike/`. Measurement-first v2 of the stopped adaptive-density work; replaces the archived `lighting-scale--adaptive-base-probe-density` and superseded `lighting-scale--adaptive-sh-probe-density`.
> **Build strategy:** two tiers (see Tasks). The CPU/compiler/wire substrate (Groundwork G1–G4) is built **directly and tested in-container** before `/orchestrate`, resolving every review-cluster decision into tested fact and producing the golden CPU reference; only the GPU thin-slice + fan-out (Orchestrated B1–B4) reaches `/orchestrate`. Groundwork not yet started.
> **Track:** Lighting / build pipeline — a strategic bake-time choice that reduces per-frame runtime load.
> **Builds on:** `context/plans/done/lighting-scale--delta-sh-valid-probe-compaction/` (merged) — the variable-stride compact delta substrate this generalizes.
> **Related:** `context/lib/rendering_pipeline.md` §4 (Animated SH delta volumes / Baked direct for dynamic receivers / Animated direct SH), §7.1 step 5 (SH compose passes) · `context/lib/build_pipeline.md` §PRL section IDs (27/34/35/41/45).

## Goal

Make the SH compose passes read **less delta storage per frame**, so a denser, better-lit bake costs less at runtime — PostRetro stays snappy by having the bake make the strategic choice, not the frame. The lever: per 4×4×4 brick, drop **valid** probes in low-variance regions and reconstruct them by trilinear interpolation from a coarser kept lattice (L0 = 64 / L1 = 8 corners / L2 = 1 brick-mean), chosen from composed-receiver reconstruction error. Lossy, unlike the merged valid-probe compaction (which dropped only in-solid invalid probes).

The per-frame delta read equals the stored payload (each active-light delta byte is read once per composed-atlas texel). Coarsening shrinks that payload — measured **≥32% smaller at 1.5 m shipping-ish density** on the arena-bearing showcase, at a gate stricter than this spec's operating point (`sw-1p5m.json`; ~12% at the pessimistic 2 m floor). Supporting certain wins that fall out for free: the smaller raw payload drops under the 256 MiB `--sh-delta-max-size` cap, so denser bakes that currently hard-fail can ship, and at-rest/disk size shrinks by the same margin.

**The per-frame win is not automatic and the spec owns it.** Each compose pass dispatches per composed-atlas texel at `@workgroup_size(8,8,1)`, which is not brick-aligned; a dropped probe reconstructs from up to 8 kept corners, and under a naive per-texel read those corners are re-fetched per texel, so a smaller payload does not by itself mean less global read traffic. Realizing the goal requires the compose to read each kept corner ~once per brick (a brick-local dispatch with a shared-memory kept-lattice load). This spec measures the per-frame read on the thin slice and commits the brick-local restructure as the remedy if the naive path does not deliver — the bandwidth drop is a gate, not a hope.

## Scope

### In scope
- **Per-brick, per-section level classifier** (L0/L1/L2) in the compiler, keyed on composed-receiver reconstruction error relative to local irradiance magnitude, at the precommitted operating point (`operating-point.md`). Two-phase (a map-wide magnitude pass, then a gate pass) because the darkness floor is a map-wide statistic.
- **Coarsening the three delta sections' stored probe set** from "valid" to "valid ∧ on-level-lattice", by threading a per-cell level into the shared compaction mask stage (`delta_sections.rs`). L2 stores one synthesized brick-mean tile (a computed write, not a copied probe).
- **A per-probe three-state descriptor** — invalid (skip), kept (read by rank), dropped-valid (reconstruct) — recoverable at every consumer.
- **Intra-brick trilinear reconstruction** of dropped-valid probes in the three GPU compose passes and the CPU reference, writing the existing dense composed atlas.
- **A compose reconstruction access pattern whose per-frame delta read scales with the kept (coarsened) payload, not the dense payload** — measured on the thin slice; brick-local dispatch + shared-memory kept-lattice load committed as the remedy if the naive per-texel path does not cut the read.
- **Mapper-authored protection volumes** (brush entity → world AABB, with a dilation margin) that force intersecting bricks to L0.
- **Per-map uniform-grid fallback**: if the coarsened payload does not clear the cap, fail the build — never coarsen harder to fit. `--sh-coarsen` defaults off.
- **One-level seam-smoothing** (fixpoint) so no brick is more than one level coarser than a face-adjacent neighbor.
- **The reconstruction math relocated** from the measurement-only `sh_analyze.rs` into a crate both the compiler and `render-cpu` depend on, made `pub`.

### Out of scope
- **Composed-atlas compaction and forward/billboard/fog sampler rework.** The composed atlas stays dense; forward samples it unchanged. Separate spec — wins composed-atlas VRAM and forward-sample bandwidth.
- **Base-atlas (id 34 / id 35) coarsening** and activating the reserved `OctahedralShProbe.density_level`. The base atlas stays dense-composed / compact-at-rest.
- **Visual A/B calibration** of the error threshold. Owner chose metric-only; the operating point is data-selected with a Weber rationale (`operating-point.md`).
- Host-RAM bake cost at shipping density.

## Direction

**Problem.** The SH compose passes stream the whole delta payload every frame — at 1.0 m shipping density that is gigabytes/second for one pass. Compaction removed the in-solid probes losslessly; the valid-but-redundant probes in low-variance regions (open air, uniformly-lit volumes) remain, buy no image quality, and dominate the per-frame read. The cause is stored density, not encoding.

**Prior commitments this touches.**
- **The compaction substrate (merged) — reuse is real but shallow.** The delta payload length and a probe's in-cell rank are pure `popcount(valid_probe_masks[cell])` (`delta_sh_volumes.rs:valid_probe_mask_payload_f16_count`; rank `(mask & (bit-1)).count_ones()` in `sh_analyze.rs:DeltaView::resolve_probe_f16_offset` and its runtime mirror `render-cpu/src/sh_compose.rs:resolve_delta_f16_offset`). A coarser kept mask therefore yields a correctly shorter payload with correct ranks and **no change to the payload encoding — for L0 and L1**, whose kept tiles are copied originals. **L2 is the exception:** its stored tile is the brick-mean (`reconstruct_l2_tile`), a value equal to no original probe, so an L2 brick's single kept tile is *computed and written*, not copied — the copy-loop reuse (`delta_sections.rs:compact_dense_valid_probe_payload`) covers L0/L1 only. The warrant for the popcount claim is verified in source this session; the L2 carve-out is finding B from review.
- **Classification precedes the dense compaction pass; the mask predicate is shared.** `delta_sections.rs:valid_probe_mask_for_affinity_cell(base, affinity_dims, cell)` derives the mask from base validity only, is called for all three sections by `compact_indirect/direct/animated_valid_probes`, and asserts a dense 64-probe payload — so it runs once, on dense data. Coarsening does not "refine it after compaction" (that would re-hit the dense assertion). Instead the classifier produces a per-cell level array *before* compaction, threaded into the mask stage so the kept set is `validity ∧ lattice(level)`. Levels and seam smoothing are computed **per section** — ids 27/41/45 carry independent masks and different composed content (indirect / direct-subtract / direct-add), so a level acceptable for one is not assumed acceptable for another.
- **Reconstruction is intra-brick, matching what the classifier measured.** `sh_analyze.rs:reconstruct_l1_tile` interpolates each local from the brick's own 8 corners (`corner_locals() = {0, AF-1}³`), never reading a neighbor cell; `reconstruct_l2_tile` is the brick-mean. The gating spike measured composed error against exactly this intra-brick reconstruction, and seam-smoothing exists precisely because bricks reconstruct independently. Runtime reconstruction is therefore intra-brick — corners never cross affinity-cell boundaries (finding A). This math is private to the measurement-only `sh_analyze.rs`; it is relocated to a shared `pub` crate so the compiler classifier, the CPU reference, and the GPU shaders share one definition (finding D).
- **The handoff's "rework every SH sampler" framing — diverges, deliberately.** The composed atlas the forward/billboard/fog samplers read is dense and written by the compose passes (`sh_compose.wgsl:compose_main` writes `sh_total_atlas`); coarsening changes the compose *read/reconstruct*, not the atlas write, so forward samplers are untouched. Compacting the composed atlas too is the separate deferred win.
- **The base-grid-first alternative** stays rejected for blast radius; see Alternatives.

**Alternatives rejected.**
- **Coarsen the base grid first, inheriting the level into the deltas.** The layer the archived plan argued for; the composed-receiver classifier fix applies there too, so the reason that plan stopped does not protect the delta-first choice. Rejected for blast radius, not correctness: base coarsening makes the composed base atlas sparse, forcing the composed-atlas compaction and the forward/billboard/fog sampler rewrite the project has repeatedly sequenced away from. The per-frame delta-read win — the actual goal — lands without any of it. The archive's "you'll rebuild the delta contract" objection: the three-state descriptor, reconstruction, classifier, protection, and seam machinery all survive a later base-grid coarsening; only the baked bytes re-bake. The throwaway risk is re-baking, not re-architecting.
- **Load-time expansion** (coarsen at rest, expand to dense in the loader, compose unchanged). Banks the cap/at-rest wins cheaply but reads the full dense payload every frame — it forgoes the per-frame bandwidth cut, which is the goal. Rejected on that ground.
- **Compact the composed atlas now** — full blast radius, deferred. **Two-level** — L2 (the brick-mean uniform volumes) is the majority of coarsenable bricks at the operating point, so dropping it forfeits most of the win. **Per-probe octree** — `AFFINITY_FACTOR = 4` and the compose addressing lock the lattice.

## Acceptance criteria

- [ ] A bake with `--sh-coarsen` emits delta sections whose raw payload is smaller than the compaction-only baseline on the arena map, by a margin consistent with the operating point at the baked density. Compiler payload accounting log; no GPU.
- [ ] Every emitted delta section round-trips (`to_bytes` → `from_bytes`), and the payload-length invariant holds for the coarsened masks — including an L2 brick whose single kept tile is the synthesized brick-mean. CPU test.
- [ ] The relocated reconstruction is one shared `pub` definition; the CPU reference compose reconstructs a dropped-valid probe as the intra-brick trilinear blend of the brick's kept corners (L1) or the brick-mean (L2) within f16 tolerance, and skips invalid probes. CPU test — the CI-enforceable value proxy for the GPU path.
- [ ] Invalid / kept / dropped-valid probes are distinguished correctly by each of the three consumers, on a constructed section. Direct/animated-direct, which have no base `probe_indirection`, prove their carried validity signal here. CPU test.
- [ ] Classification is two-phase: a dark brick (local magnitude below 2% of the map p95) coarsens to its coarsest evaluable level rather than being forced dense by the exploding relative ratio; a level whose reconstruction is unevaluable scores `+∞` and is never chosen. Compiler test on constructed magnitude/validity fields.
- [ ] A brick intersecting a protection volume is L0 regardless of error, and stays L0 after seam-smoothing. Compiler test feeding a protection AABB.
- [ ] Seam-smoothing reaches a fixpoint with no brick more than one level coarser than a face-adjacent neighbor, on a constructed field that would otherwise place adjacent L0/L2, including a forced-L0 protected brick beside a would-be-L2 neighbor. Compiler test.
- [ ] When the coarsened payload still exceeds `--sh-delta-max-size`, the build fails naming the cap and overage; coarsening runs exactly once (no re-threshold, no forced global L2). Compiler test.
- [ ] **Per-frame delta read on the id-27 compose drops with coarsening on the arena map** — measured on the GPU thin slice (B1). If the naive per-texel reconstruction does not cut the read, the brick-local restructure lands within B1 and the measurement is repeated until the read drops. This is a gate; the goal is not met until it passes. Local/dev GPU (`POSTRETRO_GPU_TIMING=1` `sh_compose`), self-skips headless.

## Tasks

Built in two tiers. **Groundwork (G1–G4)** is the CPU / compiler / wire substrate — no GPU, buildable and testable in-container. Build it **directly** (tested, in a git worktree — never a self-selecting `implement-task`, which forks its own plan), before `/orchestrate`. It resolves every review-cluster design decision into tested fact and produces the golden CPU reference. **Orchestrated (B1–B4)** is the GPU thin-slice-plus-fan-out that reaches `/orchestrate`: port the validated reconstruction into the compose shaders and prove the per-frame bandwidth win on a GPU host. The tier boundary is the container/GPU boundary and the reference/port boundary at once.

### Groundwork — build first, directly (CPU-only, in-container)

**G1 — Relocate the reconstruction math to a shared `pub` crate.** Move `reconstruct_l1_tile`, `reconstruct_l2_tile`, `corner_locals`, `trilinear_weight` (+ the `Tile`/level helpers) out of the measurement-only `sh_analyze.rs` into a crate both `postretro-level-compiler` and `postretro-render-cpu` depend on (e.g. `postretro-level-format` or a small lighting crate), made `pub`, with `sh_analyze.rs` re-importing so its measurement stays byte-identical. Behavior-preserving refactor (finding D). (AC 3)

**G2 — Pin the wire encoding, with round-trip tests.** Decide and implement the descriptor changes: the per-cell level for id 27 (rides in `delta_compaction_meta`, built at load; bump `DELTA_SH_VOLUMES_VERSION`); the id-41/45 validity signal (per-cell level byte or a second per-cell mask; bump `DIRECT_SH_DELTA_VOLUMES_VERSION` / `ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION`); the L2 synthesized-mean tile at its representative kept rank. Feed whichever mask sizes the payload to `valid_probe_mask_payload_f16_count`; keep `validate_wire_contract` (id 45) consistent. `to_bytes`/`from_bytes` round-trip tests including an L2 brick and stale-version rejection. Turns the review's "implementer picks the encoding" into a decided, tested fact. (AC 2)

**G3 — CPU classifier + coarsened-payload producer + render-cpu golden.** The compiler core for all three sections. Two-phase gate (phase A: per-brick composed magnitude via `tile_magnitude` semantics + the map p95; phase B: darkness-floor bypass at 2% of map p95 → coarsest evaluable level, else relative p95 ≤ 10% AND relative max ≤ 25% of local magnitude, unevaluable level = `+∞`) — findings E, F. Per-section per-cell level; protection-force-L0 (input to smoothing); fixpoint seam-smoothing (each sweep demotes the **coarser** endpoint of any >1-level face-adjacent pair — adjacency derived from `affinity_grid.rs` x-fastest, no existing helper — repeat to zero demotions; findings F5–F7). Thread the level into the shared `delta_sections.rs:valid_probe_mask_for_affinity_cell` / `compact_dense_valid_probe_payload` stage **before** it consumes the dense payload (finding C); synthesize L2 mean tiles (finding B); `enforce_payload_cap` runs once, fails on overflow, never re-thresholds (finding F10). Add the `render-cpu/src/sh_compose.rs` intra-brick reconstruction — the golden the GPU must match and the AC 3 CI value proxy; corners never leave the cell (finding A). All behind `--sh-coarsen` (default off). Tested against the operating point, every Ordering-pins row P1–P14, and the three-state distinction per consumer. (AC 1, 3, 4, 5, 6, 7, 8)

**G4 — Validation bake.** Bake the arena at 2 m with `--sh-coarsen` on, in-container; confirm the coarsened payload emits, round-trips, and shrinks by the expected margin. End-to-end compiler validation, no GPU. (AC 1)

### Orchestrated — GPU thin-slice + fan-out (for `/orchestrate`, GPU host)

**B1 — Thin slice: id-27 GPU reconstruction + bandwidth gate.** Port G3's intra-brick reconstruction into `sh_compose.wgsl:compose_main` (resolve the brick's own kept corners via `within_cell_rank`/`entry_delta_f16_offset`/`read_delta_texel`; trilinear-blend into the dense `sh_total_atlas`), validated against the G3 render-cpu golden. Then measure the per-frame id-27 delta read: if the naive per-texel dispatch does not cut it, restructure the id-27 compose to a brick-local dispatch that loads the cell's kept lattice into workgroup shared memory once and reconstructs all 64 probes from it, re-measuring until the read drops. Falsifies the bandwidth thesis before fan-out. (AC 9)

**B2 — Fan out GPU reconstruction to id 41 / id 45.** Apply B1's reconstruction and bandwidth-safe access pattern to `direct_sh_compose.wgsl` and `animated_direct_sh_compose.wgsl` (subtract for id 41, add for id 45), each against its G3 golden. Storage-slot budget for any new runtime buffer: id 41 has 3 free, id 45 has 1 free, id 27 has 0 free.

**B3 — Full GPU bandwidth confirmation.** On a GPU host (self-skips headless), arena before → after across all three passes (`POSTRETRO_GPU_TIMING=1`: `sh_compose`/`direct_sh_compose`/`animated_direct_sh_compose`); record dispatch time and, where measurable, read volume. Confirms B1 holds for id 41/45 and at scale. (AC 9)

**B4 — FGD + mapper docs + context capture.** Define the protection-volume entity in the FGD (its brush→AABB parse, following `trigger_volumes.rs:resolve_trigger_volume`, lands in G3), document it for mappers in `docs/`, and at promotion capture the durable contract in `context/lib/build_pipeline.md` (delta sections gain a per-cell level; payload = kept-probe count; L2 stores a synthesized mean) and `rendering_pipeline.md` (compose reconstructs dropped-valid probes intra-brick into the dense composed atlas; brick-local read pattern).

## Sequencing

**Groundwork (built directly, tested, before `/orchestrate`):** G1 → G2 → G3 → G4, sequential — G2 needs G1's shared crate, G3 needs G2's encoding, G4 validates G3.
**Orchestrated (`/orchestrate`):**
- Phase 1 (sequential): B1 — thin slice; the id-27 per-frame read must drop before fan-out.
- Phase 2 (sequential): B2 — fan out to id 41/45, consuming B1's proven access pattern.
- Phase 3 (concurrent): B3 (full GPU confirmation), B4 (FGD / docs / context).

## Ordering pins

The compiler coarsening sub-pipeline is a single normative order — classification → protection-force-L0 → seam-smoothing (fixpoint) → mask refinement → cap — inside the existing seam (`apply_exact_zero_drop_policy` → classification+mask stage → `enforce_payload_cap`). The test task cites these rows.

| # | Scenario | Expected outcome |
|---|---|---|
| P1 | `--sh-coarsen` on | Classification feeds the level into the single dense compaction; no second pass hits the dense-payload assertion. |
| P2 | L2 brick | Emit one synthesized brick-mean tile (`reconstruct_l2_tile` over the valid set), computed before valid tiles drop, at a defined kept rank. |
| P3/P4 | Reconstruction | Strictly intra-brick `{0,AF-1}³` trilinear; classifier error and runtime reconstruction use the identical shared definition. |
| P5 | X(L2)–Y(L2)–Z(L0) chain | Fixpoint loop; final state has no diff ≥ 2 pair. |
| P6/P14 | Protected P beside N | Protection forces P→L0 before smoothing, which then demotes N so N–P differ by ≤ 1; protection overwrites the chosen level, not a reporting overlay. |
| P7 | Coarser endpoint is the +neighbor | Demote the coarser endpoint, not "the current brick". |
| P8 | Dark map (mag ≈ 1e-4) | Two-phase: map p95 first; sub-floor bricks bypass the relative gate; map does not bake dense or fail the cap. |
| P9/P11 | No valid corners / all-non-corner valids | L1 = +∞ ineligible; L2 or L0; never L1. |
| P10 | Zero valid probes | Excluded from coarsening and non-participating in smoothing. |
| P12 | Cap exceeded | Build fails; coarsening ran exactly once; no re-threshold, no forced global L2. |
| P13 | `--sh-coarsen off` | Uniform bake, same cap enforced. |

## Wire format

- **Payload (all three ids):** unchanged encoding for L0/L1 — `delta_subblocks` stores one tile per kept probe (`validity ∧ lattice(level)`), x-fastest by kept rank, length via `valid_probe_mask_payload_f16_count` fed the kept mask. **L2 stores one synthesized brick-mean tile** at the kept rank of its representative bit — a computed write, not a copied probe.
- **id 27:** may store the kept mask directly in `valid_probe_masks` and recover validity at runtime from base `probe_indirection`; the per-cell level rides in `delta_compaction_meta` (built at load; id 27 has 0 free storage slots). Bump `DELTA_SH_VOLUMES_VERSION`.
- **id 41 / id 45:** carry validity **and** the kept set (no base `probe_indirection`) — a per-cell level byte or a second per-cell mask — and bump the section version; whichever mask sizes the payload is the one fed to `valid_probe_mask_payload_f16_count`; keep `validate_wire_contract` (id 45) consistent.
- **Constraint, not layout:** the three probe states are recoverable per consumer, the payload is kept-probe-sized (L2 = one synthesized tile), and the per-cell level is recoverable at load. Runtime meta packs into `delta_compaction_meta` where a consumer has no free slot (id 27: 0, id 41: 3, id 45: 1).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Classification runs on dense data, before compaction consumes the payload; the level feeds the shared mask stage | G3 (classifier + mask threading) | `valid_probe_mask_for_affinity_cell` asserts dense; a second pass on compacted data aborts | AC 1, P1 |
| Payload holds exactly the kept probes in kept-rank order; L2 = one synthesized mean tile | G2 (encoding), G3 (producer) | payload-length function must receive the kept mask; L2 tile is written, not copied | AC 2, P2 |
| A probe resolves to exactly one of invalid / kept / dropped-valid at every consumer | G3 (producer), B1/B2 (GPU consumers) | id 41/45 have no base validity source | AC 4 |
| Reconstruction = intra-brick trilinear over the brick's own kept corners (L1) or brick-mean (L2), one shared definition | G1 (relocate), G3 (golden), B1/B2 (GPU) | corners never cross cell boundaries; compiler/CPU/GPU must not diverge | AC 3, P3/P4 |
| Two-phase gate: dark bricks bypass the relative divide; unevaluable levels score +∞ | G3 | map p95 needs a completed magnitude pass; relative ratio explodes near black | AC 5, P8/P9 |
| Protected bricks are L0 and stay L0 through smoothing | G3 (protection) | protection before smoothing; overwrites chosen level | AC 6, P6/P14 |
| Seam-smoothing fixpoint: no brick > 1 level coarser than a face-adjacent neighbor | G3 (seam) | single pass misses cascades; demote the coarser endpoint | AC 7, P5/P7 |
| Coarsening runs exactly once; cap overflow fails the build | G3 (cap) | no re-threshold / forced global L2 | AC 8, P12/P13 |
| Per-frame id-27 delta read drops with coarsening | B1 (gate), B3 (fan-out) | naive per-texel dispatch may re-read corners; brick-local restructure is the remedy | AC 9 |

## Open questions

- **Brick-local restructure depth.** If Task 1 shows the naive read does not drop, how far the compose restructure goes (per-brick workgroup + shared-memory lattice) and whether it composes cleanly with the non-coarsened path or needs a branch. Sized in Task 1 against the measurement, not pre-committed.
- **Shipping-density precision.** The ≥32% floor at 1.5 m is hard-measured on `sw-1p5m.json` at a gate stricter than the operating point; the 32–65% spread narrows with a magnitude-anchored bake at ≤1.5 m on a longer-lived host (the in-container ceiling is ~2 m). Not a blocker — the win is settled as large.
- **Composed-atlas + base-atlas compaction (deferred).** Reserved id-34 `density_level` is the base-atlas slot (v9→v10). Separate spec, priced against measured atlas-VRAM pressure.
- **Protection-volume dilation margin default** (world units); expose as an entity KVP.
