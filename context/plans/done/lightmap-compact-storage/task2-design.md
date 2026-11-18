# Task 2 design — separate mask layout

Read-only design at 7a5ebe35e. Consumed by Task 2; removed at landing.

## Source facts that shape the design
1. **Block table already carries both plane addresses.** Entry today is `[lm addr, planeA addr, planeB addr, extent|flags]` (`render-cpu/src/lightmap_pool/block_table.rs:64-73`). Keep word0 bit-identical; word1 = `[mask_w | mask_h<<12, pb_w | pb_h<<12, 0, 0]`. Existing address-constant tests survive.
2. **"378 MiB" was measured on the repacked layout.** Task 2 keeps today's lightmap layout, so only the mask-pool ratio is checkable now (research at 0.025 m: whole-block masks 140 MiB → sub-rect 84 MiB; expect roughly 0.5–0.65).
3. **Every site that assumes a mask plane has its block's extent** must take per-plane extents: `policy/pair.rs:222` `past_device_limit`; comment `policy.rs:279-283`; `renderer/src/lighting/lightmap/pool.rs:249` region writes; `gpu_mirror.rs:190`; `payload.rs:44`; `static_plan.rs:372`; `policy/pool_state.rs:65` item extent.
4. **Comparator needs no change.** The mask dump records pre-BC RGBA per chart (`chart_dump/capture.rs:188`); A-only charts hold zeros in channels 2/3 in both bakes. Only the capture point moves.
5. **`sh-streaming-hinted-door.prl`** has no id 42/58; regenerate only if a reader's test fails.

## 1. Compiler mask layout
New module `level-compiler/src/shadowmask_bake/mask_layout.rs`:
- `MaskBlockLayout { extent: (u32,u32), plane_b: (u32,u32), charts: Vec<(u32 face, (u32,u32) mask_origin)> }`
- `layout_mask_block(block_id, faces, owners, shared) -> MaskBlockLayout`
- `copy_owned_charts(src, src_width, src_block_origin, block, layout, shared) -> Vec<u8>`: zeroed RGBA `extent` image, one row-copy per **padded** chart rect (`width_texels/height_texels` include padding; `dilate_owned_padding`, `fill.rs:472`), so the dilated ring travels (P8).
- `face_offset(...) -> [i16;2]` = mask origin − lightmap block-local origin.
- Chart extents read through one helper `placed_extent(face)` so Task 3's transposed w/h drop in; the copy stays a pure translation.

Packed charts: faces with `owners[face] != NO_FACE_OWNERS`. Plane-B membership: `owners[face][2] != SHADOWMASK_OWNER_EMPTY` (owners ascending, empties last ⇒ owns channel 2 or 3).

Packer: reuse `cell_blocks.rs` + `MaxRects`. Add `MaxRects::occupy(x,y,w,h)` (`atlas_pack.rs:315`, via `split_free` `:419` + prune). Generalize `pack_cell_block_within` (`cell_blocks.rs:171`), `shortest_block` (`:222`), `try_pack` (`:262`) into `pack_cell_block_reserving(charts, reserved: Option<(u32,u32)>, align, edge)`; reserved rect pre-occupied at (0,0).
1. Pack plane-B charts tight, align 4 (BC) → `plane_b`.
2. Pack A-only charts around reserved `plane_b`.
3. No B charts → `plane_b = (0,0)`, one pass.
4. **Identity fallback:** if the result exceeds `LIGHTMAP_POOL_LAYER_EDGE` or its area ≥ the lightmap block's, charts stay at lightmap-local origins, extent = block, plane B = block, offsets 0. Bounds the pool at today's; P1 holds. Log a fallback count.
5. Always-on `assert!`: every B chart inside `plane_b`; chart rects disjoint.

Determinism: layout depends only on owned charts' sizes in face order and B membership; owners are `-j`-independent; MaxRects deterministic. Per-layer ready blocks laid out with ordered `par_iter().map().collect()` under `control.governor().enter()` (as `block_layout.rs:205`).

P9: no owned face → extent (0,0), plane_b (0,0), both planes absent, no copy.

Wiring into `OwnershipFill` (`fill.rs:80`): add `block_faces: Vec<Vec<u32>>` (move build from `MaskCapture::new`, `capture.rs:173`), `mask_offsets: Vec<[i16;2]>`, `mask_extents: Vec<MaskExtents>`. In `fill/layers.rs:47 close_layer`, both after `finish_face`:
- **Carried (P10):** release loop `:83-114`, runs only when `hold_until[block] <= layer` (`carry.rs:6`), after `release_owners_that_baked_nothing` (`:60`). Layout from carry (`src = raw`, width `block.width`, origin [0,0]) → copy → capture → encode.
- **Ready:** replace whole-layer `encode_blocks_bc5` at `:152-163` with per-block layout from `self.data` (width `self.width`, origin `[block.x, block.y]`) → copy → capture → encode.
- Write `mask_offsets[face]` for owned faces; unowned stay [0,0].
- `encode.rs`: `encode_mask_block(image, w, h, plane_b) -> [Vec<u8>;2]`: `encode_blocks_bc5(image, w, h, 1, &[local])` (`:27`), trim group B to units `[0,0,pb_w/4,pb_h/4]` via `copy_unit_rect` (`lightmap_bake/encode.rs:116`) — lossless since plane B is 4-aligned at origin and A-only charts are zero in channels 2/3 (`reorder_channels`) — then `stored_plane` (`:106`).
- `#[cfg(test)] raw_snapshot`/`record_block_snapshot` stay in lightmap layout before the copy; add a test-only snapshot of mask images + layouts.
- `finish` (`fill.rs:306`) passes offsets + extents to the section.
- `SHADOWMASK_ATLAS_STAGE_VERSION` 9 → 10 (`shadowmask_bake.rs:69`).
- Dump: `capture_block` → `capture_mask_block(block, image, image_width, block_origin, offsets, owners, shared)` reading each owned chart at `placement − block_origin + offset`.

## 2. Wire format SMB9
Write the **final 44-B record now** with codec = 0, raw_len = stored_len (one tag, one layout). Load rejects codec ≠ 0 by name until Task 4.

New `level-format/src/shadowmask_atlas/record.rs` (parent is 486 lines): `MaskExtents { mask: (u16,u16), plane_b: (u16,u16) }`, `SHADOWMASK_CODEC_RAW = 0`, record read/write, validation (codec; raw_len = `group_plane_len(mask|plane_b)` when present, 0 when absent; stored = raw for codec 0; extents multiples of 4 and ≤ 2048; plane_b ⊆ mask; present plane ⇒ nonzero extent; plane-B extent 0 ⇒ group B absent).

In `shadowmask_atlas.rs`: tag → `SMB9` (`:29`); `RETIRED_SHADOWMASK_FORMAT_SMB8` added to `:34-36` and `check_format_tag` (`:463`); `OWNER_ENTRY_BYTES` = 12 (`:49`); `SHADOWMASK_BLOCK_RECORD_BYTES` = 44 (`:43`; `pack_output/readback.rs:157` follows); parse `:266-276` adds dx, dy and rejects a nonzero offset on an unowned face; `planes::check_block_planes` (`planes.rs:36`) takes per-plane expected lengths.

Model: `ShadowmaskBlockIndex { owners, mask_offsets, records }`, record gains `extents`; `ShadowmaskAtlasSection { owners, mask_offsets, blocks, extents }`. Add `ShadowmaskAtlasSection::whole_block(owners, blocks, &LightmapSection)` (identity extents, zero offsets) to migrate ~30 fixture literals (`grep -rl "ShadowmaskAtlasSection {"`). Add shared `SpecOwnerVertex { owners: FaceOwners, mask_offset: [i16;2] }`.

## 3. Id 58 / pool demand
`level-format/src/lightmap_pool/demand.rs`: `planes: Option<Vec<PlanePresence>>` (`:25`) → `masks: Option<Vec<BlockMasks>>`, `BlockMasks { planes, extents: [(u32,u32);2] }`. Keep `PoolDemand::new` as whole-block constructor (dry run `lightmap_residency_dry_run/cell_blocks.rs:120`, tests). Add `with_masks`, `plane_extent(block, plane)`, `item_extent(kind, item)`. `of_indices` (`:68`) reads record extents; `layers_of` (`:161-166`) and `place_planes` (`:186`) use `plane_extent`. `first_generation_layers` and `place_all_resident` unchanged. `LIGHTMAP_POOL_BOUND_VERSION` 1 → 2. Compiler `pack/lightmap_pool_bound/demand.rs:48 pool_demand` builds `with_masks` from `section.extents`. The dry run keeps whole-block mask demand: note that its mask figures overstate SMB9.

## 4. Loader stamp
`prl_spec_owners.rs:32` → `build_vertex_spec_owners(vertex_count, indices, leaves, Option<(&[FaceOwners], &[[i16;2]])>) -> Vec<SpecOwnerVertex>`. Conflict check `:59` compares the whole entry with distinct named reasons ("owner entries differ" / "mask offsets differ"), same `[PRL]` error + `stream.fill(empty)` (AC 12). Call site `prl_loader/mod.rs:293` passes `index.mask_offsets`; field types change at `prl.rs:758`, `prl_loader/staged.rs:301`.
Renderer: `SPEC_OWNER_STRIDE` (`renderer_geometry.rs:87`) = 12; `cast_spec_owners_to_bytes` (`:59`) writes 4×u16 then `(dx as u16) | (dy as u16)<<16`; `level_geometry.rs:40`, `world_geometry.rs:57`; pipeline layout `renderer_init_pipelines.rs:116` adds `offset 8, shader_location 7, Uint32`.
Only consumer: the forward pipeline (no-cube shares its layout), bound at `renderer_render_frame.rs:578` (main + capture) and `planar_reflection/record.rs:315` (mirror). Kinematic brush, skinned, depth prepass, animated compose bind neither the owner stream nor the block table.

## 5. Block table + shader
CPU (`block_table.rs`): `BLOCK_TABLE_ENTRY_BYTES` = 32 (`:9`); `to_words -> [u32;8]` (`:125`), `from_words` (`:162`); `Resident` gains `mask_extents`; None/Placeholder/Missing write word1 = 0; `block_table_bytes(extents, mask_extents, addresses)` (`:230`). Writers: `LightmapPoolModel::table_entry`/`table_bytes` (`policy.rs:240,257`), `journal.rs:301 rebuild_table_writes`, `static_plan` `block_table_bytes()` (used `renderer_resources/lightmap_shadow.rs:118`), `placeholder_block_table`, executor `stream/execute.rs:146` (already one `[u8; ENTRY_BYTES]` + one `write_buffer` ⇒ both words in one write: P11). `bindings.rs:63` sizes from the constant. Group 6 binding 0 is a VERTEX-only storage buffer (`forward.wgsl:343-349`): stride growth adds no binding; fragment 8/8 storage, 15/16 sampled untouched.

WGSL (`forward.wgsl:349`): `struct LightmapBlockEntry { lightmap: vec4<u32>, mask: vec4<u32> }; array<LightmapBlockEntry>` (`arrayLength` still counts entries, `lightmap_sample.wgsl:62`).
Varyings: `VertexInput @location(7) mask_offset: u32`; `VertexOutput` location 10 `vec2<u32>` → `@interpolate(flat) lightmap_mask: vec4<u32>` = (planeA addr, planeB addr, mask extent, plane-B extent) from `entry.lightmap.yz, entry.mask.xy`; add `@location(11) @interpolate(flat) mask_offset: u32`. 12 user locations, ~33 components (WebGPU defaults 16 / 60). `LightmapBlockVaryings` (`lightmap_sample.wgsl:35`) gets `mask: vec4<u32>` in place of `plane_addresses`.
Fragment: `sample_shadowmask_atlas(texel, layer_flags, mask, mask_offset)` (`:202`): `mask_texel = texel + sign-extended i16s` (`bitcast<i32>(x<<16)>>16`, as `animated_block_uv:172`). `sample_shadowmask_plane` (`:185`) receives `vec4(offset(addr), plane extent)` — mask extent for A, plane-B extent for B — so `lightmap_pool_uv` (`:102`) clamps per plane (AC 14) before the 1/256 snap. Call sites: `forward.wgsl:962-995` (`shadowmask_union_subtraction`), `:1304`, `:1339`, `:433`. No-owner guards (`owners[0] == SPEC_OWNER_EMPTY`, `spec_owners.x`) stay. Harnesses splicing these functions: `shadowmask_sample_harness.rs`, `shadowmask_union_harness.rs`, `pool_sample_harness.rs`, `bound_draw_harness.rs`, `sh_volume.rs:1752`; string pins `tests/shader_tests.rs:827,889-901`.

## 6. Runtime mask placement
`policy/pool_state.rs:65` → `demand.item_extent(kind, item)`; every `alloc_extent` user (repack, compaction, reservation, growth, `PoolCopy`) follows. Explicit: `pair.rs:222` per-half `alloc_extent`; `BlockUpload` (`plan.rs:61`) gains `plane_extents`, filled at `placement.rs:239`; `pool.rs:216 block_region_writes` per-plane at `:249` (streaming `execute.rs:120`, `upload_static_pool` `pool.rs:320`); `gpu_mirror.rs:190` + `assert_matches`; `payload.rs:36 planes_mismatch` per-plane (callers `stream.rs:372` `model.planes` → `model.mask_extents`, `static_plan.rs:372`); `static_plan` `AllResidentMasks` gains extents.

## 7. Tests per AC
| AC | Extend | Add |
|---|---|---|
| 5 | `chart_dump/tests.rs:399 chart_dump_candidate_matches_baseline` on showcase + campaign-test dumps | — |
| 8 | comparator; `prl.rs:7388` stamp | `the_stamp_carries_each_faces_mask_offset_and_zero_for_unowned`; `rejects_a_mask_offset_on_an_unowned_face` |
| 9 | — | `every_plane_b_chart_lies_inside_plane_b`, `a_channel_01_chart_may_sit_outside_plane_b`; always-on assert in the bake |
| 10 | `shadowmask_sample_test.rs:168` | `a_border_sample_reads_its_own_padding_not_the_abutting_chart`; mask_layout rects disjoint and padded |
| 11 | `shadowmask_bake/tests.rs:1344` → `a_carried_blocks_layout_drops_an_owner_released_at_its_last_layer`; `demand.rs:218` | `a_block_with_no_owned_face_has_zero_extents_no_planes_no_placement`; `pool_demand(section) == PoolDemand::of_indices(parsed)`; `#[ignore]` compiler all-resident mask layers vs `static_plan` `place_masks().layer_count` |
| 12 | beside `prl_spec_owners.rs:256` | `a_vertex_shared_by_equal_owners_with_different_mask_offsets_empties_the_stream` |
| 14 | `shader_pipeline_tests.rs:186` (+ no-cube); `pool_sample_test.rs:112` | `plane_b_samples_clamp_to_the_sub_rect_not_the_mask_extent` |
| 24 | `policy/pair_tests.rs` / `gpu_mirror` | `an_evicted_and_retenanted_block_gets_one_32_byte_write_with_both_words`; `to_words`/`from_words` round-trip with extents |

Format tests: update `shadowmask_atlas/tests.rs:81, 182, 252, 322`; new: codec ≠ 0, raw_len mismatch, plane_b ⊄ mask, unaligned/oversize extent, present plane with zero extent. Expected recaptures: SMB8 byte rows in `shadowmask_bake/tests/layer_budget_regression.rs` and `pipeline/lightmap_stage/tests/budget_regression.rs`; possibly the encode-residency bound `shadowmask_bake/tests.rs:1103`; `decode_blocks_to_layers` (`encode.rs:156`) maps back through offsets.

## 8. Order, with compile checkpoints
1. level-format wire (record.rs, SMB9, 12-B owner entry, SMB8 retired, `whole_block`, `SpecOwnerVertex`) → `cargo check -p postretro-level-format --tests` + tests.
2. level-format demand (`BlockMasks`, `item_extent`, id-58 v2) → demand/sizing tests.
3. Workspace fixture migration to `whole_block`; compiler emits SMB9 with identity layout (behaviour unchanged) → `cargo check --workspace --tests`.
4. render-cpu: 32-B table, per-item extents, `past_device_limit`, upload extents, payload, static_plan, gpu_mirror → render-cpu tests.
5. Renderer + WGSL: struct table, varyings, per-plane clamp, 12-B owner stream + attribute, region writes, harnesses → naga + GPU sample tests.
6. Loader stamp + tests.
7. Compiler `mask_layout`: `MaxRects::occupy`, reserving packer, `close_layer` seams, `encode_mask_block`, dump capture, stage v10 → shadowmask_bake tests.
8. `pool_demand` from section extents + equality test.
9. Real bakes (cold, with dumps): showcase 0.0125 + campaign-test; comparator; `prl-sections`; id-58 MiB per pool vs baseline 2,404 MiB lightmap; mask-pool layer ratio; one completed-mode walk.

## Risks
- Subset packs worse than the block → identity fallback bounds it; log count.
- Pack time in `close_layer` → parallel layout; check shadowmask `work_elapsed` (`shadowmask_bake.rs:127`) on campaign-test first.
- Varying limits on Metal → forward pipeline creation in GPU tests + naga.
- Post-BC values change at chart borders (owned charts now share BC blocks) — AC is pre-BC; M2 watches visuals.
- Task 3: offsets are pure translations of stored rects; extents through `placed_extent`.
