# lighting-scale--adaptive-probe-spacing

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 "Variable base-probe density", `context/lib/build_pipeline.md` §PRL section IDs (ids 34/35), `context/lib/experimental_spikes.md` · read at 1b4b227

## Problem

Raised by the developer as an anticipated need for large open maps. The baked SH volume is a uniform lattice (`sh_bake::DEFAULT_PROBE_SPACING`, one `cell_size` for the whole grid) whose stored density is compressed only per 4×4×4 brick: `sh_reconstruct::Level` L0 stores every valid probe, L1 eight corners, L2 one brick mean, and reconstruction is strictly intra-brick. The shipped classifier (`sh_density::classify_base_levels` → `sh_coarsen::classify_levels_with_ceiling`) gates on composed receiver error with a darkness bypass, so a wide dark or evenly lit volume still stores one tile per brick plus one 8-byte record per probe — a quantization floor the gate cannot cross. The renderer sampler (`crates/renderer/src/shaders/sh_sample.wgsl`) is hardwired to that geometry: lattice neighbor addressing (`sh_corner_index`), brick-local corner weights at `SH_AFFINITY_FACTOR`, and a whole-cell fast path that requires all eight corners in one brick. The stored set is shared by the indirect atlas (id 34, read by every surface) and the direct atlas (id 35, read by kinematic movers, skinned meshes, and legacy billboards through binding 15); world surfaces take high-frequency direct light from the lightmap and shadowmask, so the base carries only low-frequency signal. When this is done, a brick group that the existing gate certifies as one flat node is stored as one node — fewer tiles at rest, in VRAM, and in the sampler's working set — with no perceptible change on any receiver, and the measurement that decides whether the win is worth the format and sampler doors is taken before either opens.

## Decisions

- **Shape: a bounded sparse brick hierarchy over the same octahedral atlas.** A node is an aligned cube of 2^k bricks, k ≤ 3, at level L1 (eight node corners) or L2 (one node mean); k = 0 is today's brick exactly. Alignment makes node origin and node-local coordinate arithmetic from the probe index, so the renderer needs no node table and no pointer walk. This diverges from the "no deeper tree" ruling in `context/plans/done/lighting-scale--variable-base-probe-density/research.md`: that ruling priced the *delta* sections, where a smoother-than-L2 brick is a drop candidate; the base has no drop (a brick with one valid probe stores one tile, `stored_tile_set`), so the L2 floor is paid across every open brick. Rivals — tetrahedral/LPPV (strongest), octree, clipmap/DDGI — are argued in `research.md`.
- **Density is decided post-hoc, never forward.** The dense bake (`sh_bake::bake_sh_volume`) is unchanged; nodes are built bottom-up by merging eight aligned same-scale, same-level bricks only when the node's own L1/L2 reconstruction passes the exact shipped gate (`sh_coarsen::CoarsenParams::default()` scaled by `_sh_density_fidelity`) over the composed tiles of every valid probe in the node, darkness bypass included. `context/research/base-density-forward-predictor.md` closes every pre-bake or estimate scheme; a two-pass or low-ray predictor is refused, not deferred. There is no bake-time win here; the win is bytes per quality beyond the 3-tier brick LOD.
- **The delta ceiling extends to scale.** A brick with one or more CSR entries in a present grid-matched delta section (ids 27/41/45) keeps scale 0, in addition to the shipped `density_level ≤ cell_levels` (`sh_density::storage_level_ceilings`, loader `validate_storage_levels_against_delta`). Compose reconstructs deltas per brick, so a node can hold no delta; because id 41 carries an entry for every selected light's reach, every brick under a promotable static light stays a brick and the far-LOD a mover reads at rest is unchanged across the `w` crossfade (`rendering_pipeline.md` §4 "Promoted static lights"). Partial edge bricks and protection AABBs pin scale 0 and L0 as today.
- **Reconstruction stays strictly intra-node with one definition.** The per-fragment lattice corner walk is retained and each corner resolves through its own node; the L1 node arm is `sh_reconstruct::reconstruct_l1_tile` with node-local corner weights, the L2 arm the node mean. The shipped ≤1-level face-adjacency bound applies per brick using the brick's node level; a 2:1 scale-balance rule is not imposed unless Phase 1's seam metric shows a need (Open questions).
- **Wire: id 34 v10 → v11 and id 35 v3 → v4, load-rejected, no shim** (`development_guide.md` §1.6; this brief is the plan that scopes the break). The 8-byte probe record keeps its stride and carries the node scale in its reserved bytes; no per-node table — slots derive by prefix sum as today (`sh_reconstruct::stored_brick_prefix_sum`). A file whose every node is scale 0 has payload bytes identical to v10 except the version words: that is the Phase 2 compatibility case.
- **Renderer: no new binding, no new sampled texture.** The load-derived word (`sh_indirection.rs`) keeps its 32 bits on the `sh_depth_moments` carrier; scale is carved from the slot field and the builder asserts the slot field still covers the largest stored atlas. Forward FRAGMENT stays at 16 sampled textures (`pipeline_budget_tests.rs`). Every consumer — forward, fog, billboard, skinned mesh, kinematic brush — reads through the one `sh_sample.wgsl` helper; compose passes elect one writer per node slot and write copy-through there (nodes hold no delta). Undo cost: the sampler generalization touches every consumer at once; the bake can always emit scale 0, which the Phase 2 renderer reads unchanged.
- **Cost contract holds at every scale.** A sample touches ≤ 8 distinct tiles; a cell whose eight lattice corners lie in one L1 or L2 node takes the whole-cell path and issues ≤ 8 taps. f16 storage, f32 arithmetic, no `enable f16`.
- **Phased, with measurement as the gated first phase.** Phase 1 extends `--sh-analyze` with a byte-preserving hierarchy projection and ends at an owner go/no-go on the measured delta (the ~15–20% reference from `research.md` is a reference, not an AC). Phase 2 lands the wire and bake with L2-only nodes, which the shipped sampler reads without change (an L2 node is repeated L2 words at one slot). Phase 3 adds L1 nodes and the scale-aware sampler. The checkpoint also decides whether Phase 3 is in scope, from Phase 1's per-level attribution.
- **Layer placement.** Node assignment is a compiler stage at the shipped window (after `apply_runtime_safe_envelope`, before `apply_valid_probe_compaction`); the format carries scale per probe; the renderer derives the word at load; measurement levers are prl-build flags beside `--sh-density-force-level`, never runtime settings (`experimental_spikes.md`).
- **Non-goals.** The per-light delta affinity grid: `AFFINITY_FACTOR` is locked to the compose workgroup and its coarsening is the v2 ruling's separate contract. Bake-time probe placement: closed above. World-surface lighting: lightmap and shadowmask untouched. Finer-than-1 m placement: the win is coarser open space, not finer detail. Sparse probe metadata or a sparse moments carrier: the dense carrier is this brief's decision and bounds the byte win (`research.md` §Ceiling); a separate brief. Billboard scatter ids 47/48: dense `texture_3d` by probe index. Adaptive ids 27/45: v2 ruling stands. Invariants honored: renderer owns all GPU, no `unsafe`, no receiver sums one light twice, baked over computed.

## Acceptance

Phase 1 rows are honesty gates (pass/fail) or measured findings (measure-and-report) per `experimental_spikes.md`.

### Automated
Phase 1
- [ ] With the projection enabled, the emitted `.prl` of a gate fixture is byte-identical to a bake without `--sh-analyze` (regression guard on the byte-preserving analyzer).
- [ ] On a constructed brick field: eight aligned same-level bricks whose node reconstruction passes the gate merge; a group with one failing member, one member holding a delta entry, one partial brick, a protected brick, or a misaligned origin does not; after merging, no face-adjacent participating bricks differ by more than one level; with the maximum scale set to 0 the projection equals the shipped classification histogram.
Phase 2
- [ ] id 34 v11 and id 35 v4 round-trip; each rejects with its own named error: scale above the maximum, members of one node disagreeing on level or scale, a node origin not aligned to its scale, a node reaching outside the grid or over a partial brick, L0 with nonzero scale, an L1 node with no valid corner, and a brick with a delta entry at nonzero scale (loader and compiler share the validator).
- [ ] A v10 id 34 or v3 id 35 aborts the load with the named recompile error; no degrade.
- [ ] A bake with maximum scale 0 emits probe records and atlas blobs byte-identical to the prior version's on a gate fixture apart from the version words; two `--no-cache` runs of a scale-≥1 bake are byte-identical.
- [ ] One builder yields the word with scale; the moments B/A halves and every compose carrier decode to the same word per probe; the compose slot election writes each node slot exactly once per dispatch (source-shape tests extended).
- [ ] No sampler pipeline gains a binding; the forward fragment texture inventory is unchanged; every compose BGL stays ≤ 8 storage buffers (regression guard on the budget tests).
- [ ] Every brick with an id-41 entry is stamped scale 0 (the crossfade guard), asserted on a fixture with selected static lights.
- [ ] The bake summary reports the node histogram by scale and level and the bricks pinned to scale 0 by delta entries and by protection.
Phase 3
- [ ] For constructed level/scale/validity fields, the sampler's node-local corner slots and weights at every scale equal the shared reconstruction definition; a cell with all eight corners in one L1 node takes the whole-cell path with ≤ 8 taps; no cell touches more than 8 distinct tiles.
- [ ] The SDF shadow moments decode reads the same E[d] bits before and after (regression guard).
- [ ] The emitted-reconstruction analysis of a classified hierarchy bake reports zero bricks over the gate at node granularity.

### Manual
Phase 1 (recorded in `research.md`; fixture, spacing, machine class, cache mode, and cleanup pinned per `testing_guide.md` §Resource bounds)
- [ ] On `content/dev/maps/stress-warren-mini.map`, `campaign-test.map`, and `kinematic-platform.map` at 1.0 m: stored tiles and id 34/35 bytes for shipped classification, the hierarchy projection, and the all-L2 structural floor, beside the dense per-probe record bytes; node histogram by scale and level; per-level attribution of the saving; node-level composed error (rel p95/max); seam residuals across faces separating nodes of different scale or level. Full `stress-warren.map` at 1.0 m is attempted and recorded as not-yet-evaluable if the box cannot bake it.
- [ ] The owner's go/no-go and Phase 3 scope decision are recorded in the plan of record.
Phase 2
- [ ] Scale-≥1 L2-node bakes of the Phase 1 fixtures boot with no wgpu validation errors and render indirect on world, movers, skinned meshes, billboards, and fog.
Phase 3
- [ ] Manual-visual hunt at node faces, open volumes crossed by movers, and lit pools on the Phase 1 fixtures at default fidelity plus a forced-scale worst case, from a content root; recorded as a read, never as parity.
- [ ] Per-pass GPU time (`POSTRETRO_GPU_TIMING=1`) before and after on the named adapter; not-yet-evaluable without `TIMESTAMP_QUERY`, never inferred from CPU time.

## Path

Non-binding.
- Seams, by symbol, in `research.md` §Layer map: node stored set and prefix sum beside `stored_tile_set`; bottom-up merge over `BrickClass` shaped stats from `sh_analyze::{build_brick_tiles, level_errors, tile_magnitude}`; ceilings in `storage_level_ceilings`; validation in `validate_probe_metadata`; word in `sh_indirection.rs` and `sh_indirection.wgsl`; compose writer election in `stored_slot_for_invocation` (indirect, direct, and animated-direct compose); sampler arms `sh_l1_local`, `sh_l1_corner_weight`, `sh_whole_cell_resolution`; diagnostics decoder in `sh_diagnostics.rs`.
- Shape chosen: aligned power-of-two brick cubes addressed by the existing per-probe word. Strongest rival: tetrahedral probe placement — real adaptivity in open space, but it discards the octahedral atlas, Chebyshev suppression, and the brick-major compose, and needs a lookup structure the budgets cannot fit.
- First slice: Phase 1's projection on `stress-warren-mini` — it falsifies "there are bytes beyond the L2 floor worth a format door" before any door opens; compute the analytic ceiling from the shipped histogram first (`research.md` §Ceiling).
- Files past ~800 lines this extends: `sh_analyze.rs`, `sh_density.rs`, `sh_bake.rs`, `pipeline.rs`, format `sh_volume.rs`, renderer `sh_volume.rs`, `sh_coarsen.rs` — add the merge and node helpers as new modules; split only if a seam is genuinely tangled.

## Open questions

- Phase 1 go: the analytic ceiling (`research.md` §Ceiling) bounds the tile win at roughly a tenth to a sixth of stored tiles on the representative fixtures and leaves the dense per-probe record untouched; the owner decides whether the merge projection is worth building or the ceiling alone is the checkpoint — owner: project owner — **blocks build**
- Maximum node scale actually adopted (≤ 3 on the wire) and whether a 2:1 scale-balance rule is needed — **delegated**: from Phase 1's node histogram and seam metric, reported in the plan of record
- Whether full `stress-warren.map` at 1.0 m is bakeable on the measurement box — **delegated**: attempted, recorded either way

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP | CLI |
|---|---|---|---|---|
| Node scale | per-probe scale beside `OctahedralShProbe.density_level` (node-uniform, 0 = brick) | id 34 v11 probe record reserved byte | n/a | `--sh-density-force-scale <0..3>` (measurement only, beside `--sh-density-force-level`) |
| Hierarchy projection | `sh_analyze` report section | n/a (JSON sidecar) | n/a | `--sh-analyze` (existing; projection always on) |
| Fidelity, opt-out, protection | `MapData.{sh_density_fidelity, uniform_grid_optout, sh_protect_aabbs}` — **landed**; now also govern merging | n/a | `_sh_density_fidelity`, `_sh_coarsen "0"`, `sh_protect_volume` — **landed** | `--sh-density-fidelity`, `--sh-protect-aabb` — **landed** |

## Wire format

Constraint-level; encodings are implementer-pinned at landing. Little-endian, section-internal version first, named reject on mismatch.

- **id 34 (v11).** Header fields unchanged in meaning. Probe records keep the 8 B stride; `density_level` semantics unchanged; the node scale occupies reserved bytes and is identical across every probe of a node. Constraints: scale ≤ 3; scale 0 on partial edge bricks and on any brick with a delta entry; L0 only at scale 0; L1 at any scale needs ≥ 1 valid node corner; a node's origin brick is aligned to its scale and the node lies fully inside the grid. Payload: bricks x-fastest; a node's stored set (L1 eight node corners in `corner_locals` order with zero tiles for invalid corners; L2 one mean over the node's valid probes) is charged to its origin brick and other member bricks store nothing. Slots derive by prefix sum; no node table on disk.
- **id 35 (v4).** Same stored set, geometry, order, and tag discipline as id 34's for the same map; no metadata.
- **ids 27 / 41 / 45.** Unchanged. Cross-section constraint at load: a brick with ≥ 1 CSR entry has scale 0 and `density_level ≤ cell_levels`.
- **Indirection word (runtime only).** 32 bits on the moments carrier: level, validity, scale, slot; the all-zero word is invalid; one builder feeds the compose buffers and the moments B/A halves.
