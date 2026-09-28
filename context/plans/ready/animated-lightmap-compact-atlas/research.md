# animated-lightmap-compact-atlas — research

Read-only research. PRLs were parsed with scratchpad Python scripts; nothing was built.

## Why the atlas is full-layer-sized

- `AnimatedLightmapResources::new` sizes the atlas as `usable_atlas_dimensions` × slot count (`animated_atlas_extent`).
- Compose (`animated_lightmap_compose.wgsl` `compose_main`) writes at `rect.atlas_x/y + local` into `tile.target_slot`, in static coordinates.
- Forward samples `sample_lightmap_animated(in.lightmap_uv, animated_slot)` with the static UV. The slot comes from `animated_slot_for_static_layer(in.lightmap_layer)`, the binding-7 uniform (256 entries, 1 KiB).
- Compose is valid only for visible cells. Only the world forward pass binds group 4 as the lightmap group. Movers, meshes and the viewmodel bind `sh_mesh_bind_group` there. No other consumer binds these textures.

## Measurements

Pages are Rgba16Float irradiance plus Rgba8Unorm direction, 12 B per texel: a 1024² page is 12 MiB.

| Map | Today | Block area | Page size | Pages: area bound / shelf by height / shelf in face order | Notes |
|---|---|---|---|---|---|
| campaign-test | 144 MiB (3 × 2048²) | ≈31.5 MiB | 1024² | 3 (36 MiB) / 4 (48 MiB) / 6 (72 MiB) | 154 blocks, largest side 645; chunks cover 20.3% of slot texels, lit texels 11.1% |
| occlusion-test | 84 MiB (7 × 1024²) | ≈10 MiB | 1024² | 1 (12 MiB) / 2 (24 MiB) / 3 (36 MiB) | 97 blocks, largest side 568 |
| stress-warren-mini | 5.06 MiB (27 × 128²) | ≈3.1 MiB | 128², capped by the static layer | 17 (3.2 MiB) / 20 (3.8 MiB) / 37 (6.9 MiB) | 977 blocks, largest side 78 |

Blocks are approximated as each face's surviving-chunk bounding box plus the 2-texel padding. Neither shelf packer is the real one; they bracket packer quality. The "near a quarter" figure for `campaign-test` assumes a MaxRects packer reaches the area bound's page count; a height-sorted shelf reaches a third. Face order stands in for cell order. An in-order shelf exceeds today's bytes on `stress-warren-mini`, so packer quality is load-bearing on small static layers.

The sizing scripts are in the session scratchpad (`compact.py`, `pages.py`), which is not durable.

## Data invariants observed in all PRLs with section 25

These were observed in the data. The compiler does not enforce them, which is why the brief does.
- One face per BVH leaf.
- Vertices are emitted per face and never shared (`extract_geometry`).
- No animated leaf spans two layers.
- Chunk rect ⊆ leaf vertex-UV box + 1 texel. Worst excess: 0.873 texel against the UV box, 0.015 on campaign-test, and 0.001 on stress-warren-mini against the placement.
- Block count reaches 977 on stress-warren-mini and no content exceeds it. Chunk count is much higher: stress-warren-hallway-inspection has 40187 chunks over 47 faces. The cap binds blocks, not chunks. Cap figures: P16.
- `CHART_PADDING_TEXELS = 2`. A 2-texel gutter holds while overrun stays below 1.5 texels.

## Formats

- Section 25 today: `ANIMATED_LIGHT_WEIGHT_MAPS_VERSION = 3`, 20 B header, `ChunkAtlasRect` 24 B, trailing `slot_to_static_layer`. `from_bytes` also accepts v2, the one lenient reader.
- Section 17 vertex: 36 B, with `lightmap_layer: u16` and then `_padding: u16`. There is no version field and parsing requires the exact size. The runtime `WorldVertex` widens the layer to u32 at offset 32. Only the forward textured pipeline reads `@location(5)`.
- Stage: `animated_light_weight_maps::STAGE_VERSION = 6`, cache id `animated_lm_weight_maps`. The cache stores pre-cull bytes. `build_pipeline.md` calls this stage uncached, which is doc drift to fix at promotion.
- Rejected smaller formats: in wgpu 29, core storage-writable formats are Rgba8Unorm, Rgba16Float and the 32-bit families. Rg8Unorm and Rg11b10Ufloat need adapter-specific features, and Rgb9e5 is never writable. Once compact, the gain is about 15 MiB on campaign-test.
- Rejected sub-layer page tiles: 64² pages come to about 45 MiB. They need a page table and seam handling, and the table grows with static atlas area.
- Rejected second UV attribute: it changes the stride for every world pipeline and for section 43.

## Stale content

These PRLs carry section 25 and must be rebuilt after the version bump: campaign-test (and its -id41-coarsened and -bakeonly variants), closet-reveal, occlusion-test, spawner-test, stress-warren-mini, stress-warren-hallway-inspection, and a11y-strobe-test (from the E23 branch). The golden `test_animated_weight_maps_mixed.pre-script-light-membership.prl` is already v2 and needs a new baseline.

## Tests pinning today's shape

- level-format: v3/v2 round-trip and layout tests, slot and chunk-layer consistency checks, `rejects_bad_version`, `byte_estimate_scales_with_slots…`, geometry `vertex_is_36_bytes…` / `lightmap_layer_round_trips`.
- render-cpu: `validate_cross_section_rejects_*slot*`, `…rect_outside_static_atlas_bounds`.
- renderer: slot-table preflight, compose/forward shader string pins, `dispatch_tile_expansion_*`, atlas extent and dimension tests, `static_layer_slot_uniform_layout_matches_forward_wgsl`, `bgl_entries_pin_sampler_split`, `lightmap_layer_serialized_at_byte_offset_32`, over-budget fallback.
- compiler: `real_multi_layer_*`, `cull_drops_*`, `animated_atlas_over_budget…`, `overlap_assert_rejects_cross_face_rects_on_one_layer`, `stage_version_bump_*`, `animated_layer_spill_fixture…`, the golden-PRL test.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Edit only an animated light so faces gain or lose a block; rebuild warm | SDF-atlas input hash is taken before block ids are stamped on vertices | SDF-atlas cache key unchanged; SDF stage hits |
| P2 | Cold build, then warm rebuild with no edits (weight-map stage hits) | Cache holds the pre-cull, pre-repack bake; cull, repack and stamping run after both hit and miss | Section 25 and section 17 bytes identical between the two builds |
| P3 | A face whose every chunk the unlit cull drops | Repack runs after the cull | No block for that face; its vertices carry id 0; block count equals faces with a surviving chunk |
| P4 | A face with several chunks, some culled | Repack after cull; block = whole placement | One block; surviving chunks keep their offset inside the block; culled chunk texels stay zero |
| P5 | Every candidate chunk culled, or no animated light | Emptiness decided after the cull | No section 24 or 25; every vertex id 0; animated pair at placeholder bytes |
| P6 | Chunks exist but compose has nothing to write (all SDF-typed animated lights), or atlas construction fails | Block table is built from the installed resource state, after the active/dummy decision | Every vertex resolves to no block; no compact offset is applied against the dummy |
| P7 | Atlas construction fails after its size is computed (tile buffer over the storage limit) | Meter records at the allocation that survives the fallback decision | Meter reports placeholder bytes, not the rejected atlas's |
| P8 | One block whose extent equals the page size | Packer places blocks in cell order | It sits alone at the page origin; the next block starts the next page; no page is empty |
| P9 | Blocks overflow one page | Compose tile targets and forward lookup read one block table | Two pages; compose and forward agree on page and offset for every block, including page 1 |
| P10 | Block count at the cap, and cap + 1 | Cap derived once, shared by compiler and forward shader | At cap passes; cap + 1 fails naming both; the cap fits the requested uniform size and the 16-bit vertex id |
| P11 | Vertex names a block past the table, or a block on another static layer | Checked after both sections decode | Debug or `dev-tools` build: load fails with a recompile error. Player release build: level loads with no animated light and one logged error |
| P12 | Load level A, unload, load level B | Meter rebuilt per install; block table rewritten at install | After unload each count is placeholder; after B, counts are B's alone |
| P13 | "Before" resource reading | Meter must exist while the atlas is still full-layer | Before numbers come from the meter, not hand-parsed PRLs (F15) |
| P14 | Section 25 whose page size is not a power of two, is below the largest block or the lower bound, or exceeds the static layer size (section 22's layer width) | Page-size preflight after both sections decode | Level rejected; when section 22 is the placeholder, the upper bound is skipped and the level keeps today's no-animated-light path |

Additional research pins (subtract lens), literal text:

- **P15** — supersedes the Data invariants bullet "No animated leaf spans two layers.": No animated leaf spans two layers. Enforced by type: one face has one `ChartPlacement` with a single `layer`, and one leaf is one face.
- **P16** — supersedes the Data invariants cap figure ("2040 blocks"): block count ≤ 977 across content (chunk count reaches 40187). The cap derives from the requested `max_uniform_buffer_binding_size` (64 KiB under `Limits::default()` in wgpu 29). Blocks pack into 16-byte uniform array elements, so the cap is (limit − header) / per-block bytes after packing: 8190 blocks at 8 B per block behind a 16 B header, 4095 at 16 B. Current content (≤ 977 blocks) uses about 12% or 24% of it. Both caps sit under the 16-bit vertex id's 65535.

- **P17** — The identity layout is one page per static layer holding a chunk after the cull, ascending, compact layer = today's slot index. One page per static layer would exceed today (campaign-test 192 vs 144 MiB; occlusion-test 144 vs 84 MiB; stress-warren-hallway-inspection ≈3.4 GiB vs 144 MiB, over the 1 GiB animated-atlas budget).
