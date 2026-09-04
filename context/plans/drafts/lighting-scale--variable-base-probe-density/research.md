# Variable Base-Probe Density — retained material and derivations

Research notes for `index.md`. Retained by reference from the retired plans `lighting-scale--adaptive-sh-probe-density` (drafts, removed at `765e186`) and `lighting-scale--adaptive-base-probe-density` (archived, removed at `765e186`); recoverable via `git show 765e186~1:<path>`. Code references below are re-anchored to the current tree; the retired plans' line anchors are stale.

## Three-level rescope arithmetic — why 64 / 8 / 1, why not two, why not a tree

Recorded stress sweep (`stress-warren-maze-crates.map`, 61 selected lights): id-41 bytes 401 MB @2.0 m → 1,306 MB @1.33 m → 2,579 MB @1.0 m; pairwise scaling exponents 2.85 / 2.95 / 2.39 in probe density. Binding at the 128 MiB WebGPU storage-binding floor needed 19.2× at the 1.0 m shipping default.

- *Two levels (64/8):* coarsening's structural ceiling is 8× — the all-coarse limit was still 307 MiB at 1.0 m, over the floor with zero dense cells; reaching the floor would have needed ≥ 72–93% of entries dropped outright.
- *Three levels (64/8/1):* an L2 brick stores one 288 B tile (64×). The all-L2 limit at the shipping default was 38.4 MiB — under budget by construction before any drops. Realistic mixes (3% L0 + 10% L1 + 60% L2 + 27% dropped) reach 19.3×. L2 exists for the far field of broad lights — smooth but not negligible, the population drops cannot touch. Its reconstruction arm is cheaper than L1's (one read per probe).
- *Deeper tree / coarser-than-brick merging:* beyond 64× per brick, depth helps only where a region is smoother than one tile per 4³ probes — a drop candidate anyway; merging bricks breaks the workgroup-coupled affinity geometry (`AFFINITY_FACTOR = 4`); Godot ships no deep tree on its baked path. Rejected on arithmetic.

The same arithmetic applies to the composed atlases this spec compacts: an L2 brick's 64 tiles collapse to 1, an L1 brick's to 8.

## Hardware target and the Pascal f16 contract

Owner target: very performant on NVIDIA GTX 10-series, at least runnable — and, for this spec, smooth — on laptops including shared-memory iGPUs. This undercuts `context/lib/rendering_pipeline.md` §10's documented GTX 16-series perf floor; the divergence is deliberate and owner-stated, to be captured in `context/lib/` at promotion.

- **Bandwidth.** A GTX 1060 has ~192 GB/s shared by every pass; a laptop iGPU shares system DRAM, so the same traffic is a larger fraction of a smaller bus. Sampler traffic is per-fragment: the forward pass taps eight tiles per fragment (entities: sixteen). Locality, not tile count, is the bandwidth lever — coarse bricks make the eight corners hit one or few tiles.
- **VRAM.** 10-series spans 3 GB (1050, 1060 3GB) to 8 GB; the 1060 6GB is the volume target, 3 GB parts and iGPUs the floor. Baked-lighting residency should stay under ~1 GB on the floor; the two dense composed SH atlases alone are ~112 MB on `campaign-test` at 1.0 m.
- **Pascal fp16.** Consumer Pascal (compute capability 6.1) executes native fp16 arithmetic at 1/64 the fp32 rate (NVIDIA Pascal tuning guidance; community measurements). The engine is on the right side: delta payloads stay f16 at rest and `unpack2x16float` yields `vec2<f32>`; atlases are `Rgba16Float` sampled into f32. Contract: **f16 storage, f32 arithmetic** — no WGSL `enable f16` in any pass this spec touches.

## Cap posture (inherited, unchanged)

Bake-side: 256 MiB aggregate post-compaction cap over ids 27/41/45 (`DEFAULT_MAX_PAYLOAD_BYTES`, `enforce_payload_cap`), fail-loud, no coarsen-to-fit retry; 64 MiB authoring warning. Loader: 128 MiB per-section binding floor checked on raw bytes before decode, all-or-nothing per section (id 41 clears id 40). Runtime: oversized SH atlases are disabled cleanly (`atlas_fits`), never a validation crash. This spec adds no cap: stored tiles ≤ dense tiles, so any atlas that fit dense fits stored.

## Industry survey — mechanisms and disposition

| Mechanism (source) | What it is | Disposition |
|---|---|---|
| 64-probe bricks with multi-level subdivision, min/max spacing (Unity APV) | 4×4×4 bricks at several subdivision levels, denser near geometry | Adopted, collapsed to three stored levels on the fixed affinity grid; APV's arbitrary-depth tree rejected on arithmetic |
| `ProbeBrickIndex` / indirection texture (Unity APV, Unreal VLM) | World position → brick pointer at its level | Adopted as the per-probe word: level + validity + stored slot, riding the depth-moments texture every sampler already binds |
| Maximum Brick Memory (Unreal VLM) | Hard budget; over-budget silently culls detail bricks | Not needed for the stored atlases (always ≤ dense); delta caps stay fail-loud |
| Detail bricks culled where lighting is "nearly equal" (Unreal VLM) | Refinement keyed on lighting variance, not only geometry | Adopted — the composed-receiver-error classifier is exactly this criterion, measured post-hoc |
| Probe classification / inactive probes (RTXGI DDGI) | Stop paying for probes that cannot contribute | Already shipped read-side (validity, sentinel words); valid-only membership is the composed-atlas analog |
| Relocation / virtual offset (RTXGI, APV); dilation (APV) | Position-side fixes for sampling artifacts | Rejected — handled read-side by validity + Chebyshev weighting |
| Streaming cells (APV), per-sublevel streaming (VLM) | Load/evict by camera | Deferred; brick-major slot order keeps the layout residency-friendly |
| Sky occlusion (APV) | Runtime sky re-lighting | Rejected — indoor portal engine, fully baked |
| Camera-centered cascades (Godot SDFGI) | Runtime distance-from-camera adaptivity, memory bounded by construction | Rejected as mechanism (bake-time static format); adopted as posture — bounded cost is non-negotiable on small-engine hardware |
| Probe-to-probe occlusion against leaks (Godot SDFGI) | Leak suppression in the probe structure | Already shipped (per-probe validity + Chebyshev depth visibility) |
| Sparse, author-densified probes for dynamic objects (Godot LightmapGI) | Never a dense uniform volume on the baked path | Closest-peer endorsement: sparse-with-local-density is table stakes; mapper protection volumes are the hand-densify analog |
| Authored bounds, low-end warning, half-res knob (Godot VoxelGI) | Expensive path must fit or be warned off | Adopted via the hardware target; fidelity is a bake-time author knob |
| Reflection-probe blend margins (Godot) | Runtime crossfade over seams | Rejected — seams are prevented at the source (classifier + seam smoothing + continuous corner walk) |

Godot findings are from documentation search extracts; Unity/Unreal from their public manuals.

## Derivations behind the design

**Why the word must ride the moments texture.** Forward's FRAGMENT stage binds 8 storage buffers (group 2: `lights`, `light_influence`, `spec_lights`, `chunk_offsets`, `chunk_indices`; group 3: `anim_descriptors`, `anim_samples`, scripted-light descriptors) against the downlevel ceiling of 8 that the renderer does not raise (`rendering_pipeline.md` §10), and 16 sampled textures with cube support (§4 "Dynamic direct"). The depth-moments texture (`upload_depth_moment_texture`, `Rgba16Float` 3D, RG used, BA zero — `pack_probe_depth_moments`) is bound by every SH reader (`BIND_SH_DEPTH_MOMENTS` = 14 on group 3 / mesh group 4) and by the SDF shadow pass (group 1 binding 2). Switching it to `Rgba16Uint` keeps the moment bits (decoded with `unpack2x16float`) and frees 32 bits per probe for the word. `textureLoad` on a `texture_3d<u32>` is valid in vertex, fragment, and compute stages, so the billboard vertex-stage SH sample is unaffected.

**Why one slot space.** The compose already dispatches one workgroup per brick with 64 invocations (`sh_compose.wgsl` `compose_main`, `@builtin(workgroup_id) brick`). If id 34, id 35, the composed total atlas, and the composed direct atlas share the stored set and its brick-major order, an invocation's stored slot is the same number in all four textures: the compose reads base at slot s and writes composed at slot s, with only the delta term reconstructed. Invariant I2 (storage never coarser than a present delta) is what makes the delta term a direct read at L1/L2 slots: every delta present in an L2 brick is itself L2 (one mean tile), and every delta present in an L1 brick is L1 (its kept corner tiles) or L2.

**Why L1 stores all eight corner slots.** The sampler resolves a corner probe with only its own word. For an interior probe of an L1 brick it needs the brick's corner tiles; if corners were kept-rank-compacted by validity (as the delta sections do) the sampler would need the brick's 64-bit validity mask to rank a corner. Storing eight fixed slots (invalid corners as zero tiles with alpha 0, exactly the "absent corner" the shared `reconstruct_l1_tile` drops and renormalizes over) makes the corner slot `brick_slot + corner_index`, and alpha carries presence.

**Sampler tap cost.** With the base-lattice eight-corner walk retained, the per-corner definition costs: L0 1 tap; L2 1 tap; L1 2^k taps where k is the number of axes on which the probe's brick-local coordinate is interior (1 or 2) — 1 (brick corner), 2 (edge), 4 (face), 8 (body). Naively, a sample cell in the body of an L1 brick therefore costs 8 × 8 = 64 taps, so the per-corner arithmetic cannot be the implementation for that case. Two structural facts bound the design:

- *Distinct tiles per sample ≤ 8, always.* A sample cell touches ≤ 8 bricks; within each touched brick its corners lie on the sub-face (body, face, edge, or corner) shared with the cell, and every one of those corners reconstructs from that brick's corner tiles on that same sub-face — 8 (body), 4 (face), 2 (edge), 1 (corner) tiles, shared by all the cell's corners in that brick. Summed over touched bricks the union is exactly 8 for L1 (one brick × 8, two × 4, four × 2, eight × 1); L2 bricks contribute 1 and L0 bricks the actual corner probes, so the union never exceeds 8. DRAM traffic per sample is therefore never worse than today's eight tiles, and coarse bricks make those tiles shared across neighboring samples.
- *Whole-cell path for interior cells.* Trilinear interpolation over the eight base-lattice corners of values that are themselves trilinear over the brick corners equals trilinear over the brick corners at the cell's brick-relative fraction — so a cell fully inside an L1 brick is 8 taps at the brick's corner slots, and inside an L2 brick 1 tap. 27 of the 64 sample cells overlapping a brick are interior (3 of 4 per axis); this path is mandatory (Task 5).

Straddling cells (37 of 64) fall to the per-corner path: a one-face straddle costs ≤ 4 taps per corner (32 total, 8 distinct tiles), an edge straddle ≤ 2 (16), a brick-corner straddle 1 (8). Re-taps of a tile within one fragment are cache-served. Task 8 records the implied distribution of distinct tiles and tap instructions from the level map (AC15); the measured per-pass time is the finding.

**Stored-tile projection (hypothesis, not a gate).** N_s = Σ_bricks {L0: valid probes; L1: 8; L2: 1}. On `campaign-test` (V = 57,128 valid probes, B = 3,306 bricks): uniform L0 → 57,128 tiles (16.5 MB per composed atlas, 3.4× vs dense); if the spike's 75% coarsenable @0.10 splits roughly into L1/L2 with the L0 remainder holding its share of valid probes, the composed atlas lands in the low single-digit MB range before the delta pin. The pin's cost is unknown until measured — it is AC14.

**Why the delta sections need no wire change.** `valid_probe_mask_for_affinity_cell` (`delta_sections.rs`) derives each cell's mask from `base.probes[i].validity`, which stays dense in v10; `cell_levels` are the sections' own; `affinity_dims = ceil(grid_dimensions / 4)` reads the unchanged grid header. The compose reads deltas exactly as today and only changes where it writes. The retired plans assumed the delta contract would be rebuilt with the base grid; that assumption predates the shipped per-cell `cell_levels` and kept-rank payloads.

**Contribution helpers not consumed.** `incident_radiance_at_point`, `light_contribution_lambert`, `falloff`, `light_reaches_point` (`sh_bake.rs`) are the bake's direct-contribution math. A forward contribution predictor is foreclosed (`context/research/base-density-forward-predictor.md`); the post-hoc classifier measures the composed field, which already embodies delivered contribution (cone + falloff + facing + occlusion), so nothing here calls them.
