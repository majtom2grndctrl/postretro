# Research notes — delta-SH probe coarsening

Grounding for the spec. Identifiers verified against source this session. Line
numbers omitted deliberately (they go stale); cite `file:symbol`.

## Shared delta read chain (identical across id 27/41/45 GPU passes + CPU ref)

1. **probe → cell + local**: `map_probe_to_affinity` → `cell_index` (x-fastest over `grid.affinity_dims`) + `local_probe` (0..63, `local = lx + ly*4 + lz*16`).
2. **valid-mask gate + within-cell rank**: `delta_compaction_meta` holds two u32 words per cell (the u64 mask, low then high) via `valid_probe_mask_word`; `within_cell_rank(cell, local)` = popcount of set bits below `local`. This rank = the probe's compact slot among **kept** probes. Payload is indexed by this rank, not by `local` — the axis coarsening extends.
3. **entry → payload base offset (CSR)**: tail of `delta_compaction_meta` from `compaction_meta_offset_base()` = `affinity_dims.x*y*z*2` holds one f16 base offset per post-drop CSR entry; `entry_delta_f16_offset(entry)`.
4. **f16 fetch**: `read_delta_texel(entry, rank, texel)` = `entry_delta_f16_offset(entry) + rank*grid.delta_probe_f16_stride + texel*4`, two u32 words `unpack2x16float`.

## Per-consumer specifics (verified in the shaders + Rust layouts)

| Consumer | file:entry | invalid signal | base atlas | read-only storage buffers | free slots |
|---|---|---|---|---|---|
| Indirect id 27 | `sh_compose.wgsl:compose_main` | base `probe_indirection[..]==INVALID_PROBE_INDIRECTION` (0xffffffff) | **compact** (`sh_base_atlas` + `probe_indirection`) → dense `sh_total_atlas` | 8 (`[20,21,24,25,26,27,22,23]`) | **0** |
| Direct id 41 | `direct_sh_compose.wgsl:compose_main` | `local_probe_is_valid` (delta mask) | **dense** `direct_base_atlas`, nearest fetch | 5 (`20,21,24,26,28`) | 3 |
| Animated direct id 45 | `animated_direct_sh_compose.wgsl:animated_compose_main` | `local_probe_is_valid` | dense `direct_intermediate_atlas` (Pass A out) | 7 (`20,21,22,23,24,25,27`) | 1 |

- All three dispatch **per composed-atlas texel** at `@workgroup_size(8,8,1)` (verified in all three `.wgsl`). Not brick-aligned — an 8×8 texel workgroup straddles 6×6 probe tiles. Bears on the per-frame read-bandwidth question (spec Direction / Task 8).
- Each pass builds its **own** bind-group layout (`compose_bgl_entries` / `promotion_compose_bgl_entries` / `animated_compose_bgl_entries`) — not shared. `compose_layout_keeps_eight_compute_storage_buffers_and_local_sampler` pins id 27 at 8.
- CPU reference: `render-cpu/src/sh_compose.rs:resolve_delta_f16_offset` (core; direct/animated wrappers delegate). Returns `None` on clear mask bit (= invalid today); coarsening needs a third "dropped-valid" result + reconstruction. `delta_entry_offsets` sizes each cell by `mask.count_ones()*stride`.

## Wire / format (level-format)

- `delta_sh_volumes.rs`: `DeltaShVolumesSection{ valid_probe_masks: Vec<u64>, affinity_offsets, affinity_lights, delta_subblocks: Vec<u16>, ... }`; `DELTA_SH_VOLUMES_VERSION=4`; `AFFINITY_FACTOR=4`; `PROBES_PER_CELL=64`. Length identity: free fn `valid_probe_mask_payload_f16_count(offsets, masks, stride)` = Σ_cell `(offsets[c+1]-offsets[c]) × masks[c].count_ones() × stride`. **Purely popcount-driven** → a coarser mask shortens the payload with no encoding change. No per-cell level field exists.
- `direct_sh_delta_volumes.rs` id 41 (`DIRECT_SH_DELTA_VOLUMES_VERSION=2`, no descriptor indices) and `animated_direct_sh_delta_volumes.rs` id 45 (`ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION=2`, has descriptor indices, strict `validate_wire_contract`) share the same mask+CSR layout and reuse the same length fn.
- `sh_volume.rs` id 34: `OctahedralShProbe.density_level: u8` — reserved, "every v9 bake writes zero; v9 parsing rejects nonzero." The base-atlas level slot (v9→v10) — **out of scope** here. Compact-atlas fields: `compact_atlas_dimensions`, `compact_atlas_tiles_per_row/layer`, `compact_atlas_layer_count`, `irradiance_format`, `compact_atlas`.

## Bake hook points (level-compiler)

- Pipeline order (`pipeline.rs`): exact-zero drop → **valid-probe compaction** → payload cap → optional `--sh-analyze` (measurement only, changes no bytes).
- `delta_sections.rs`: `PostBakeDeltaSections::apply_valid_probe_compaction(base)` → `compact_dense_valid_probe_payload` → `CompactedDeltaPayload{ valid_probe_masks, delta_subblocks }`. Mask decided in `valid_probe_mask_for_affinity_cell(base, affinity_dims, cell)` (sets bit iff `base.probes[..].validity != 0`). **Classifier hook**: refine this predicate to `valid ∧ lattice(level)`. `DEFAULT_MAX_PAYLOAD_BYTES = 256 MiB`; cap in `enforce_payload_cap` (hard `anyhow` error, no drop-to-fit).
- `affinity_grid.rs`: `AFFINITY_FACTOR=4` (= 4×4×4 brick = one affinity cell = 64 base probes); `affinity_dims = base_dims.div_ceil(4)`; `grid_dimensions(min,max,spacing)`; x-fastest cell linearization; CSR via `build_csr`.
- Reference reconstruction math (`sh_analyze.rs`, measurement-only today): `corner_locals() -> [usize;8]`, `local_xyz(local)`, `trilinear_weight(target, corner) -> f32` (per-axis `t/(AF-1)`), `reconstruct_l1_tile(tiles, target_local, texels) -> Option<Tile>`, `reconstruct_l2_tile(tiles, texels) -> Option<Tile>` (brick-mean), `enum Level{L0,L1,L2}`, `stored_delta_tiles(level, mask)`, `choose_level`. `tile_magnitude` (added this session) gives per-brick composed magnitude for the relative gate.

## Protection / fallback hooks

- Brush-entity → AABB precedent: `trigger_volumes.rs:resolve_trigger_volume(geo_map, brush_ids, props, scale, classname)` (shambler `brush_hulls`/`face_vertices` → `aabb_min`/`aabb_max` via `quake_to_engine`), `encode_trigger_volumes_section`.
- Measurement stand-in already present: `sh_analyze::ProtectAabb{min,max}`, CLI `--sh-protect-aabb` (`main.rs:parse_protect_aabb` → `Args.sh_protect_aabbs`), `intersects_any(protect_aabbs, wmin, wmax)` forces `Level::L0` in the sweep.
- No uniform-grid-fallback decision point exists today; the natural seam is around `enforce_payload_cap` in `delta_sections.rs`.

## Gating-spike evidence

`context/research/coarsening-gating-spike/` — `README.md` (data + density trend), `operating-point.md` (the precommitted gate + rationale), `mini-2m` / `arena-2m` magnitude-enabled JSONs, `relerr_opmap.py`. Operating point: coarsest of L0/L1/L2 whose relative-p95 ≤ 10% AND relative-max ≤ 25% of local composed magnitude (2% darkness floor). Cosine-weighting dropped (≈ unweighted). Cut on top of compaction: 11.7% (arena 8 m bricks) → ~65% (showcase 6 m bricks, approximate).
