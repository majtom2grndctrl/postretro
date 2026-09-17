# Adaptive probe spacing — grounding, layer map, rivals, risks

Derivation for `index.md`. Read at 2441b87 (re-grounded after `sh-delta-tile-alpha-drop` landed). Symbols cited are for the executor to re-verify; a stale claim here is reported in the plan of record.

## What ships today (grounded this session)

- **Lattice.** `sh_bake::DEFAULT_PROBE_SPACING = 1.0`; `bake_sh_volume` sizes the grid to the world AABB and bakes every probe; `cell_size = [probe_spacing; 3]` (`sh_bake.rs`). The renderer uniform `ShGridInfoParams` (`crates/render-cpu/src/sh_volume.rs`) carries one `cell_size` vec3 for the whole grid.
- **Three-tier brick LOD (id 34 v10).** `sh_reconstruct::Level {L0, L1, L2}`; `stored_tile_set` — L0 valid probes, L1 all eight `corner_locals` (invalid corners as zero tiles), L2 one mean, an all-invalid brick stores nothing; `stored_brick_prefix_sum` → `StoredBrickPrefixSum` assigns brick-major slots. The v10 header carries one stored-tile atlas geometry; the payload is the stored set in brick-major order (`crates/level-format/src/sh_volume.rs`, `SH_VOLUME_VERSION = 10`). id 35 mirrors the stored set (`DIRECT_SH_VOLUME_VERSION = 3`).
- **Classifier.** `sh_density::classify_base_levels` builds one `BrickClass` per brick from composed tiles (`sh_analyze::{build_brick_tiles, level_errors, tile_magnitude}`) and calls `sh_coarsen::classify_levels_with_ceiling`: Phase A map-wide p95 magnitude and darkness floor, Phase B per-brick gate (`CoarsenParams::default()` = rel p95 0.10 / rel max 0.25 / darkness 0.02, scaled by `_sh_density_fidelity`), Phase C protection + ceilings, Phase D fixpoint seam smoothing (`smooth_pair`, participating bricks only, `demote_one` skips L1 when the brick's L1 failed its own gate). Ceilings come from `sh_density::storage_level_ceilings` (partial bricks L0; any brick with a CSR entry capped at that section's `cell_levels`). The stage runs in `pipeline.rs` after `apply_coarsen_classification` / `apply_runtime_safe_envelope` and before `apply_valid_probe_compaction` (the P21 window — `DeltaView` strides dense payloads); packing (`pack_indirect_section_with_levels`, `pack_direct_section`) runs after `--sh-analyze`.
- **Loader.** `validate_probe_metadata` rejects out-of-range levels, disagreeing bricks, nonzero levels on partial bricks, and L1 without a valid corner, then derives the prefix sum; `validate_storage_levels_against_delta` (called from `prl_loader.rs` `validate_storage_ceiling_for_delta`) enforces `density_level ≤ cell_levels` per brick with entries.
- **Renderer word.** `sh_indirection.rs`: `SH_INDIRECTION_LEVEL_MASK = 3`, `SH_INDIRECTION_VALID_BIT = 4`, `SH_INDIRECTION_SLOT_SHIFT = 3` (29-bit slot); `build_probe_indirection_words` is the single builder; `sh_indirection.wgsl` `decode_sh_probe_indirection` is the single decode. Carried in `sh_depth_moments` (`Rgba16Uint` 3D, dense per probe: RG = f16 moments, BA = word; `pack_probe_depth_moments`, `upload_depth_moment_texture`) and in storage buffers for the three compose passes (`sh_compose.wgsl` binding 26 and the direct siblings).
- **Sampler.** `sh_sample.wgsl`: `SH_AFFINITY_FACTOR = 4u`; `sh_corner_index` clamps lattice neighbors; `sh_l1_local` and `sh_l1_corner_weight` compute brick-local corner weights with the factor hardwired; `sh_whole_cell_resolution` requires all eight corners in one brick (`probe / 4`) at one level and slot; `sample_l1_probe_atlas` / `sample_l1_whole_cell_atlas` are the L1 arms; `sample_probe_atlas_slot` is the L0/L2 arm. Consumers by string concatenation (`rendering_pipeline.md` §8): `forward.wgsl` (`pipeline_layout.rs`), `fog_volume.wgsl` (`fog_pass.rs`), `billboard.wgsl` (`smoke.rs`), `skinned_mesh.wgsl` (`mesh_pass.rs`), `kinematic_brush.rs`. `sdf_shadow.wgsl` `sample_open_distance` reads only the R/G moment bits.
- **Chebyshev.** `sh_corner_depth_visibility` weights each lattice corner from that probe's own moments; the moments carrier is dense and per probe regardless of level.
- **Compose.** `sh_compose.wgsl` `compose_main`: one 8×8 workgroup per brick; `stored_slot_for_invocation` elects writers (L0 valid probes, L1 corners via `l1_shared_slot`, L2 `local_probe == 0`); `l1_corner_weight` mirrors the sampler at `AFFINITY_FACTOR`. `direct_sh_compose.wgsl` and `animated_direct_sh_compose.wgsl` carry the same arithmetic.
- **Analyzer.** `sh_analyze::run_analysis` → `AnalysisReport`: `SectionBytes` per id with `uniform_bytes`, `compacted_bytes`, `coarsen_all_l1_bytes`, `coarsen_all_l2_bytes` (the structural floor); `composed_atlas` projection; `SweepRow` histogram and `projected_bytes` per threshold; `SeamStats` from `compute_seams` (shared-face residual differencing, `cross_level_*`); `run_emitted_reconstruction_analysis` scores the emitted sections against dense truth under the exact gate. Byte-preserving: `pipeline.rs` `run_sh_analysis` reads finalized sections and mutates nothing. Its id-34 byte model counts tile bytes at 288 B raw per tile; it does not count probe records.
- **Budgets.** Forward FRAGMENT: 16 sampled textures, inventory `[0, 4, 0, 3, 5, 4]` (`pipeline_budget_tests.rs` `forward_pipeline_sampled_texture_request_matches_bgl_definitions`); storage buffers at the downlevel 8 (`rendering_pipeline.md` §10). The v-b-p-d spec's I5 records why the word had to ride the moments texture.
- **Dev tools.** `sh_diagnostics.rs` `decode_probe_irradiance_atlas` decodes the stored atlas per slot; `MarkerMode::DensityLevel` colors probes by level.

## Ceiling — where the bytes are

The analyzer's byte model and the shipped findings bound what a tile-level hierarchy can save.

- **Representative fixtures at 1.0 m** (`context/plans/done/lighting-scale--variable-base-probe-density/findings.md`, AC14): campaign-test 2,554 L0 / 390 L1 / 362 L2 bricks, 28,268 stored tiles; kinematic-platform 888 / 711 / 747, 34,954 stored tiles. Only L1 and L2 bricks can merge. Merging eight L1 bricks (64 tiles) into one L1 node (8 tiles) saves 56; eight L2 bricks (8) into one L2 node (1) saves 7. Upper bound on stored-tile reduction, every eligible group merging once: campaign (390·8 + 362)·7/8 ≈ 3,050 of 28,268 (≈ 11%); kinematic (711·8 + 747)·7/8 ≈ 5,630 of 34,954 (≈ 16%). Deeper merges add at most the same fraction again of what remains, so the practical bound is the ~15–20% the routing session cited. In bytes: composed atlases at 288 B/tile ≈ 1.8 MB on campaign; at rest BC6H ≈ 36 B/tile ≈ 0.1 MB.
- **The dense record.** id 34 stores 8 B per grid probe (`OCTAHEDRAL_PROBE_STRIDE`) and the moments texture 8 B per probe in VRAM. At 1.0 m: campaign 194,028 probes → 1.55 MB; stress-warren-mini 502,000 probes (grid 125×18×223, spike finding 4) → 4.0 MB. Its all-L2 tile floor is 7,889 bricks × 288 B = 2.27 MB raw, ≈ 0.28 MB BC6H. The retained 2 m analysis (`context/research/coarsening-gating-spike/mini-2m.sh-analysis.json`) shows the same shape: id 34 `coarsen_all_l2_bytes` 387,072 vs 70,560 probes × 8 B = 564,480 of records. **On an open map the record already exceeds the tile floor; a hierarchy that keeps the dense carrier cannot take id 34 below the record.** This is the decision's known cost, not a defect; sparse metadata is a separate brief.
- **The multi-GB claim.** `tools/gen_stress_map.py`'s docstring: full `stress-warren.map` at 1.0 m "would bake millions of probes and gigabytes". Unmeasured at 1.0 m in any retained record; the gigabytes recorded elsewhere are delta payload (`variable-base-probe-density/research.md`: id 41 2,579 MB at 1.0 m on `stress-warren-maze-crates` — a pre-alpha-drop figure; the landed RGB re-stride shrinks each delta payload by a quarter, but it remains delta, not base), out of scope here. Phase 1 attempts the full warren and records feasibility.
- **Sample cost.** Whole-cell L1 is 8 taps and L2 is 1 tap at any scale; per-fragment tap count does not fall with node size, only working-set locality does. The claim in the brief is sized accordingly.
- **Coarsenable share on the open fixture.** Spike finding 1: stress-warren-mini 41.2% of bricks coarsenable at the 0.10 composed gate (18.5% at 0.02) — the floor case, dense uniform lighting. Real open content should coarsen more (spike honesty notes); magnitude is Phase 1's job.

## Layer map — what changes, by symbol

### (a) Bake and classifier (`postretro-level-compiler`, `postretro-level-format`)
- `sh_reconstruct`: a node-aware stored set beside `stored_tile_set` (scale k: L1 → the node's eight corners at node-local 0 and 4·2^k − 1; L2 → one mean over the node's valid probes); `stored_brick_prefix_sum` charges a node's tiles to its origin brick and zero to other members; `Level` unchanged. Keep `kept_mask` (delta) untouched.
- `sh_density`: `classify_base_levels` gains a bottom-up merge after `classify_levels_with_ceiling` returns brick levels: for k = 1..K, for each aligned 2^k group whose members are all participating, same level, same scale k−1, ceiling-free (scale ceiling 0 blocks), and unprotected, build a `BrickClass`-shaped stat for the node (magnitude and L1/L2 error over all valid probes of the node, node corners for L1) and accept the merge only if the node passes the same Phase B test (darkness bypass included). Re-run the ≤1-level smoothing over bricks after each scale pass (a merge never raises a brick's level, so the bound holds; assert). `storage_level_ceilings` returns a scale ceiling too: 0 for partial bricks and for any brick with a CSR entry. `apply_forced_level_constraints` clamps a forced scale the same way. `stamp_levels` stamps scale. `pack_indirect_section_with_levels` / `pack_direct_section` pack node stored sets; the L2 mean is `l2_mean_tile` over the node.
- `pipeline.rs`: no reordering — the merge runs inside the existing classification call; the bake summary line gains the node histogram and scale-0 pin attribution. `--sh-density-force-scale` beside `--sh-density-force-level` (`main.rs`).
- `sh_analyze` (Phase 1, before any of the above): a projection section in `AnalysisReport` — node histogram by scale and level, projected stored tiles and bytes for id 34/35 and the composed atlases against the shipped classification and `coarsen_all_l2_bytes`, per-level attribution, node-level error records, and a seam pass over faces between nodes of different scale or level (reuse `compute_seams`' residual differencing with node reconstruction). It reads the same dense window `classify_base_levels` reads; `run_sh_analysis` already runs there. Emits no bytes.

### (b) Wire (`postretro-level-format`, `postretro-level-loader`)
- `sh_volume.rs`: `OctahedralShProbe` gains the scale field in the reserved bytes; `SH_VOLUME_VERSION` 10 → 11; `validate_probe_metadata` validates node uniformity, alignment, containment, L0-only-at-scale-0, L1 corner validity at node granularity, and derives the node-aware prefix sum; `validate_storage_levels_against_delta` adds the scale-0 rule for bricks with entries. `direct_sh_volume.rs`: `DIRECT_SH_VOLUME_VERSION` 3 → 4.
- `prl_loader.rs`: the id-35 geometry/length cross-check reads the node-aware prefix sum; `validate_storage_ceiling_for_delta` unchanged in shape.

### (c) Renderer (`postretro-renderer`, `postretro-render-cpu`)
- `sh_indirection.rs` / `sh_indirection.wgsl`: scale field carved from the slot field (`SH_INDIRECTION_*` constants, `decode_sh_probe_indirection`), one definition, Rust constants asserted equal to the WGSL; `build_probe_indirection_words` writes every member probe of a node with the node's level, scale, and slot.
- Compose (Phase 2): `stored_slot_for_invocation` in `sh_compose.wgsl`, `direct_sh_compose.wgsl`, `animated_direct_sh_compose.wgsl` elects the node's origin brick as the sole writer of a node slot (for L2: origin brick, `local_probe == 0`; for L1 nodes in Phase 3: the origin-brick invocation that owns each node corner slot); node slots are copy-through of the base slot because nodes hold no delta. `local_probe_is_l1_corner` / `l1_shared_slot` / `l1_corner_weight` become scale-aware in Phase 3.
- Sampler (Phase 3): `sh_l1_local` → node-local (`probe − (probe / node_size) · node_size`), `sh_l1_corner_weight` fraction over `node_size − 1`, `sh_whole_cell_resolution` compares `probe / node_size` per corner; `sample_l1_probe_atlas` and `sample_l1_whole_cell_atlas` unchanged in shape. Phase 2 needs none of this: an L2 node's probes all carry level 2 and the node slot, which the shipped L0/L2 arm reads correctly; a cell straddling a brick boundary inside a node falls to the per-corner path and reads the one slot eight times (correct, cache-served).
- `sh_diagnostics.rs`: decoder and `DensityLevel` marker read scale. `ShGridInfoParams` unchanged. Tests: `pipeline_budget_tests.rs` budget row unchanged; `sh_volume.rs` binding-inventory tests unchanged; the resolve-arithmetic CPU pin in `render-cpu` extended over scale.

## Rivals

| Rival | What it buys | Why rejected |
|---|---|---|
| **Tetrahedral / LPPV probe placement** (strongest) | Genuine adaptivity in open space; probes only where authored | Discards the octahedral atlas, `sh_corner_depth_visibility` leak suppression, the brick-major compose and prefix-sum machinery; needs a tetra lookup (a storage buffer or texture the 8/8 and 16/16 forward budgets cannot hold); wins only under a different premise — mapper-authored probe placement |
| Octree with node pointers | Unbounded depth | Pointer traversal per fragment; a node buffer needs a binding the forward fragment stage does not have; unbounded cost on the laptop iGPU floor |
| Clipmap / DDGI | Bounded runtime cost by construction | Runtime-computed irradiance; violates baked-over-computed (`index.md` §2) |
| Second per-probe word texture or storage indirection | Room for a richer descriptor | No free sampled or storage slot in forward FRAGMENT (v-b-p-d I5) |
| Arbitrary face-adjacent merging (non-cube nodes) | Slightly higher merge yield | Needs per-axis extents in the word and breaks arithmetic origin derivation; aligned cubes are the pointer-free case |
| Adaptive bake-time placement | Bake-time win | Closed: `base-density-forward-predictor.md` — the need for density is manufactured in composition, downstream of the bake |

## Risks

- **Light leak at coarse nodes.** The routing session framed this as Chebyshev weakening at large nodes. Grounded: the moments carrier stays dense and per probe, and `sh_corner_depth_visibility` weights each lattice corner from that corner's own moments, so the visibility term is unchanged at any scale. The real mechanism is value averaging: an L2 node mean or L1 node corners blend irradiance across everything inside the node. The gate measures reconstruction error only at valid probes; a node spanning a wall has valid probes on both sides and fails unless both sides are dark (darkness bypass) — the bypass is the leak path to watch, because a dark room adjacent to a lit one merges only if the node as a whole is below the floor. Phase 1 reports node-level error and the darkness-bypass share of merged nodes; Phase 3's visual hunt targets node faces near geometry.
- **Promotion crossfade continuity.** id 41 carries an entry for every selected light's reach (`build_pipeline.md` id 41: coverage preserved even for zero contributions); the scale-0 rule on bricks with entries therefore keeps every promotable region at brick granularity, and the mover's far-LOD read is identical to today's. Animated promotion (id 45) is covered by the same rule.
- **Directional (sun/moon) lights.** Verified (source at 2441b87): `light_sun` maps to `LightType::Directional` (`quake_map::translate_light`) with `falloff_range` forced to 0; a static one lands in `StaticBakedLights` (lightmap + base only), is excluded from promotion (`entity_shadow_select::is_promotable_base_light` gates on Point/Spot; test `selector_excludes_non_runtime_or_non_static_lightmap_lights`), and carries no delta entry — so an open area under a static sun coarsens freely (its low-frequency, near-uniform bounce is the ideal merge case, and its residual is a smooth vertical gradient that L1 nodes hold and a single L2 mean cannot — the Phase 3 payoff). An *animated* directional light is a hazard: `AnimatedBakedLights::from_lights` has no light-type gate, and `affinity_grid` gives a directional light the whole-world AABB (`world_aabb_for_directional`) with the portal filter bypassed (`reachable_leaves` → None), so ids 27/45 carry an entry in *every* brick and the scale-0 ceiling would disable coarsening map-wide. The pin is structurally real (compose is per-brick; ids 27/45 pinned uniform L0), not just the crossfade guard — and a directional light never wins a runtime slot (`assign_shadow_pool_slots_with_promoted_baked`), so its animation is already inconsistent (the lightmap skips it, `animated_light_chunks`; test `directional_animated_light_skipped`). Decision: the compiler normalizes an animated directional light to static and warns, closing the footgun without a compose redesign.
- **Cross-node seams.** Reconstruction is intra-node and the lattice corner walk resolves each corner through its own node, so continuity is at the lattice as today (v-b-p-d P8). A large L2 node beside L1 bricks is the same 1-level step the brick case allows; a scale step (2^k beside 2^(k−1)) has no counterpart today — Phase 1's seam pass over scale-differing faces decides whether a 2:1 balance rule is needed.
- **Darkness bypass at node scale.** Phase A's map-wide p95 is unchanged; a node's `mag_p95` is over more probes, so the bypass can admit a node whose members were individually bright-gated. Phase 1 attributes merges to bypass vs gate.
- **One-way doors.** (1) `SH_VOLUME_VERSION` / `DIRECT_SH_VOLUME_VERSION` bumps re-bake every `.prl`; pre-release, no shim, cheap in time but every fixture and golden re-bakes — the owner's promotion sign-off is the §1.6 confirmation. (2) The Phase 3 sampler generalization edits one helper consumed by every SH-reading pipeline; reversible because the compiler can always emit scale 0 (Phase 2 renderer reads it unchanged) and Phase 2 ships before Phase 3.
- **Slot-field width.** Carving scale bits from the 29-bit slot field shrinks the addressable stored-tile count; the builder asserts against `SH_INDIRECTION_MAX_SLOT`, and the largest realistic atlas (millions of tiles) stays far below 2^27.
- **Compose write races.** Without writer election, eight member workgroups would write one node slot with identical values; election is required, not optional, and the source-shape tests pin it.

## Phase 1 harness notes

- Fixtures: `stress-warren-mini.map` (open-space stress; 1.0 m bake measured feasible at 2.0 GiB peak, ~16 min SH stage), `campaign-test.map` (representative), `kinematic-platform.map` (theatrical; no id 27/45). Full `stress-warren.map` at 1.0 m as a stretch; record peak RSS and wall clock either way. Pin fixture, spacing, `--no-cache` vs warm, machine class, worker count, and cleanup (`testing_guide.md` §Resource bounds). Cold `--no-cache` is the exact ship source of truth; warm bakes are approximate-indirect — use cold for the recorded numbers where the box allows.
- Report rows: shipped classified tiles/bytes; hierarchy projection tiles/bytes; `coarsen_all_l2_bytes` floor; record bytes (probes × 8); node histogram by (scale, level); attribution of saved tiles to L1 vs L2 merges and to gate vs darkness bypass; node-level rel p95/max distribution; seam residuals split by same-scale/cross-scale and same-level/cross-level faces; bricks blocked from merging by delta entries, protection, partial bricks.
- Owner ruling (this session): a marginal disk-and-live-VRAM win is accepted because the hierarchy is bandwidth-neutral and taken only where unnoticeable, so the ~15–20%-of-stored-tiles figure is informational, not a go/no-go gate. Phase 1 confirms magnitude per fixture and sizes Phase 3 (whose win on large open / directional-lit maps is the L1-shaped bounce gradient); it does not decide whether to build.
- Resource weighting for the Phase 3 gate (owner): the reject test is per-resource, not a scalar net. On the measured fixtures composed-atlas live VRAM is ~16–20 MB against a ~1 GB residency budget — abundant — while `.prl` disk is the epic's scarce term and GPU bandwidth is protected (the hierarchy is bandwidth-neutral). So a net cost that lands only in an abundant resource does not reject Phase 3; only an unoffset net loss in a scarce resource does. Phase 1's per-level attribution reports the L1-vs-L2 delta per resource so the gate can be applied this way.

## Phase 1 local measurement — 2026-09-17

The owner requested a manageable local map and reserved the preferred full Warren hallway for Windows. `stress-warren-hallway-inspection-mini.map` proved not manageable on this Mac at the pinned 1.0 m spacing: an exact cold attempt reached 2% of SH after 75.73 s while its ETA worsened to about 46 minutes; the ordinary approximate development path showed the same order of cost. Both were stopped before emitting an artifact. The completed local control therefore uses the already-approved Phase-1 theatrical fixture `content/dev/maps/kinematic-platform.map`; the Windows runbook below retains the full hallway stress proof.

Measurement identity:

- source checkpoint: `ea4ade943` (`measure adaptive SH hierarchy projection`)
- machine class: macOS 26.6.2, x86_64, 16 logical CPUs
- command: `target/debug/prl-build content/dev/maps/kinematic-platform.map -o <temp>/kinematic.prl --release --no-tui --sh-probe-spacing 1.0 --sh-analyze --sh-analyze-out <temp>/kinematic.analysis.json -j 14`
- cache/quality: exact `--release`; cache bypassed
- workers: 14
- wall time: 33.73 s; SH stage 11.46 s
- maximum resident set: 1,757,925,376 B (1.64 GiB); macOS peak-footprint counter 1,051,430,912 B
- grid: 23×65×89 = 133,055 probes, 118,231 valid; 2,346 bricks, 2,112 non-empty
- output hashes: PRL SHA-256 `998401450a586b2c9e9a978bfcdda47c9375d2b3e3a201cf7764c3c8d9221f07`; JSON SHA-256 `38dcfbd70d576c8c759d38987cf5670a04f9ed4b6faa75efca2338bf041260ac`
- cleanup: outputs lived only under `/private/tmp/postretro-adaptive-kinematic.0jv1kv` and were removed after recording these numbers

The id-34, id-35, and composed carriers share the same stored-tile geometry and 288-byte raw tile size on this fixture:

| measure | stored tiles | bytes per carrier |
|---|---:|---:|
| shipped scale-0 classification | 43,179 | 12,435,552 |
| hierarchy projection | 42,850 | 12,340,800 |
| all-L2 structural floor | 2,112 | 608,256 |
| dense id-34 probe records | n/a | 1,064,440 |

The hierarchy saves 329 tiles (0.762% of shipped stored tiles), or 94,752 raw bytes per carrier. The two on-disk base carriers therefore save 189,504 raw bytes together; the composed live atlas saves another 94,752 bytes. L1 nodes contribute 224 saved tiles (64,512 B per carrier) and L2 nodes contribute 105 (30,240 B per carrier). Because L1 makes a positive contribution in scarce disk as well as abundant composed-atlas memory, Phase 3 is selected.

| node scale | L0 | L1 | L2 |
|---:|---:|---:|---:|
| 0 | 773 | 879 | 308 |
| 1 | 0 | 4 | 15 |
| 2 | 0 | 0 | 0 |
| 3 | 0 | 0 | 0 |

Of the 19 scale-1 nodes, four passed the relative gate directly (worst relative p95 0.08849, worst relative max 0.15506) and 15 used the specified darkness bypass. Darkness-bypass nodes reached relative p95 0.27982 / relative max 0.62629, which is expected because the relative comparison is deliberately skipped below the absolute darkness floor. No level-smoothing demotion was needed. Candidate blocks were: delta entry 57, partial edge 70, member shape/level 135, gate 3, protection 0.

The hierarchy seam pass measured 5,528 differing-node brick faces: 244 cross-scale and 1,862 cross-level. Residual max/mean were 0.03799 / 0.000943 overall and 0.03220 / 0.002313 on cross-scale faces. Since only scale 1 participates, adjacent scale differences cannot exceed one at the adopted operating point; the cross-scale maximum is also below the overall maximum. Adopt maximum emitted scale 1 and add no separate 2:1 scale-balance rule.

### Windows Warren hallway runbook (owner action)

Run the preferred full stress map from a clean checkout after Tasks 3–8 land. This is blocking external evidence, not inferred from the local control. In a Developer PowerShell at the repository root:

```powershell
cargo build -p postretro-level-compiler --release
$Run = Join-Path $env:TEMP "postretro-adaptive-warren-hallway"
New-Item -ItemType Directory -Force $Run | Out-Null
$Map = "content/dev/maps/stress-warren-hallway-inspection.map"
$Prl = Join-Path $Run "hallway.prl"
$Json = Join-Path $Run "hallway.analysis.json"
$Args = @($Map, "-o", $Prl, "--release", "--no-tui", "--sh-probe-spacing", "1.0", "--sh-analyze", "--sh-analyze-out", $Json, "-j", "14")
$Clock = [Diagnostics.Stopwatch]::StartNew()
$Process = Start-Process -FilePath ".\target\release\prl-build.exe" -ArgumentList $Args -NoNewWindow -PassThru
$Peak = 0L
while (-not $Process.HasExited) {
    $Process.Refresh()
    $Peak = [Math]::Max($Peak, $Process.PeakWorkingSet64)
    Start-Sleep -Milliseconds 500
}
$Clock.Stop()
[pscustomobject]@{
    ExitCode = $Process.ExitCode
    WallSeconds = $Clock.Elapsed.TotalSeconds
    PeakWorkingSetBytes = $Peak
    Processor = $env:PROCESSOR_IDENTIFIER
    LogicalProcessors = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    PhysicalMemoryBytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
}
Get-FileHash -Algorithm SHA256 $Prl, $Json
```

Record the JSON hierarchy table, projected/shipped/all-L2 bytes, node errors, blocker attribution, seam residuals, worker count, exact command, machine fields, wall/RSS, hashes, and whether every required receiver renders without wgpu validation errors. If the full map is infeasible, repeat the same script with `stress-warren-hallway-inspection-mini.map` and record the full-map stopping stage/limit. After copying the measurements, clean up with `Remove-Item -Recurse -Force $Run`.

## Pinned orderings

| id | scenario | ordering pinned | expected outcome |
|---|---|---|---|
| R-DIR2 | A 2×2×2 block of eight already-merged scale-1 nodes (each merged from 8 bricks), all same level, whose combined L1/L2 reconstruction over the full 8-node span passes the gate. | The k=2 merge pass runs only after the k=1 pass and its post-pass seam-smoothing sweep have both settled every scale-1 node's level and scale; a scale ceiling of 0 introduced anywhere in the block during k=1 (delta entry, protection, partial brick) must still block the k=2 merge. | The eight scale-1 nodes merge into one scale-2 node; a block with one member left at scale 0 by a k=1 ceiling or gate failure does not merge to scale 2 even though the other seven would otherwise qualify. |
| R-FORCE | A bake with `--sh-density-force-scale 3` over a fixture carrying id-41/27/45 delta entries, a partial edge brick, and a `sh_protect_volume` AABB. | The forced scale is clamped by `storage_level_ceilings` (delta + partial) and by protection before stamping — the same order `apply_forced_level_constraints` clamps a forced level (ceilings/protection/seam applied after the force), never the force overriding them. | Delta-entry, partial, and protected bricks stay scale 0 (and level ≤ cell_levels); only ceiling-free unprotected bricks take the forced scale; the emitted `.prl` loads without a validator reject. |
