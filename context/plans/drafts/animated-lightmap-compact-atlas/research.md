# animated-lightmap-compact-atlas — research

Read-only research. PRLs were parsed with scratchpad Python scripts; nothing was built.

## Why the atlas is full-layer-sized

- `AnimatedLightmapResources::new` sizes the atlas as `usable_atlas_dimensions` × slot count (`animated_atlas_extent`).
- Compose (`animated_lightmap_compose.wgsl` `compose_main`) writes at `rect.atlas_x/y + local` into `tile.target_slot`, in static coordinates.
- Forward samples `sample_lightmap_animated(in.lightmap_uv, animated_slot)` with the static UV. The slot comes from `animated_slot_for_static_layer(in.lightmap_layer)`, the binding-7 uniform (256 entries, 1 KiB).
- Compose is valid only for visible cells. Only the world forward pass binds group 4 as the lightmap group. Movers, meshes and the viewmodel bind `sh_mesh_bind_group` there. No other consumer binds these textures.

## Measurements

| Map | Today | Compact (blocks) | Notes |
|---|---|---|---|
| campaign-test | 144 MiB | ≈31.5 MiB, 48 MiB as one 2048² layer | 3 slots, 154 chunks; chunks cover 20.3% of slot texels, lit texels 11.1% |
| occlusion-test | 84 MiB | ≈10 MiB | |
| stress-warren-mini | 5.06 MiB | ≈3.1 MiB | Power-of-two width and height would make it worse than today |

The sizing script is in the session scratchpad (`compact.py`), which is not durable.

## Data invariants observed in all PRLs with section 25

These were observed in the data. The compiler does not enforce them, which is why the brief does.
- One face per BVH leaf.
- Vertices are emitted per face and never shared (`extract_geometry`).
- No animated leaf spans two layers.
- Chunk rect ⊆ leaf vertex-UV box + 1 texel. Worst excess: 0.873 texel against the UV box, 0.015 on campaign-test, and 0.001 on stress-warren-mini against the placement.
- Chunk count ≤ 977 across content. The block table at 8 B per block is well under 16 KiB. The cap with a 16 B header is 2040 blocks.
- `CHART_PADDING_TEXELS = 2`. A 2-texel gutter holds while overrun stays below 1.5 texels.

## Formats

- Section 25 today: `ANIMATED_LIGHT_WEIGHT_MAPS_VERSION = 3`, 20 B header, `ChunkAtlasRect` 24 B, trailing `slot_to_static_layer`. `from_bytes` also accepts v2, the one lenient reader.
- Section 17 vertex: 36 B, with `lightmap_layer: u16` and then `_padding: u16`. There is no version field and parsing requires the exact size. The runtime `WorldVertex` widens the layer to u32 at offset 32. Only the forward textured pipeline reads `@location(5)`.
- Stage: `animated_light_weight_maps::STAGE_VERSION = 6`, cache id `animated_lm_weight_maps`. The cache stores pre-cull bytes. `build_pipeline.md` calls this stage uncached, which is doc drift to fix at promotion.
- Rejected smaller formats: in wgpu 29, core storage-writable formats are Rgba8Unorm, Rgba16Float and the 32-bit families. Rg8Unorm and Rg11b10Ufloat need adapter-specific features, and Rgb9e5 is never writable. Once compact, the gain is about 15 MiB on campaign-test.
- Rejected sub-layer page tiles: 64² pages come to about 45 MiB. They need a page table and seam handling, and the table grows with static atlas area.
- Rejected second UV attribute: it changes the stride for every world pipeline and for section 43.

## Stale content

These PRLs carry section 25 and must be rebuilt after the version bump: campaign-test (and its -id41-coarsened and -bakeonly variants), closet-reveal, occlusion-test, spawner-test and stress-warren-mini. The golden `test_animated_weight_maps_mixed.pre-script-light-membership.prl` is already v2 and needs a new baseline.

## Tests pinning today's shape

- level-format: v3/v2 round-trip and layout tests, slot and chunk-layer consistency checks, `rejects_bad_version`, `byte_estimate_scales_with_slots…`, geometry `vertex_is_36_bytes…` / `lightmap_layer_round_trips`.
- render-cpu: `validate_cross_section_rejects_*slot*`, `…rect_outside_static_atlas_bounds`.
- renderer: slot-table preflight, compose/forward shader string pins, `dispatch_tile_expansion_*`, atlas extent and dimension tests, `static_layer_slot_uniform_layout_matches_forward_wgsl`, `bgl_entries_pin_sampler_split`, `lightmap_layer_serialized_at_byte_offset_32`, over-budget fallback.
- compiler: `real_multi_layer_*`, `cull_drops_*`, `animated_atlas_over_budget…`, `overlap_assert_rejects_cross_face_rects_on_one_layer`, `stage_version_bump_*`, `animated_layer_spill_fixture…`, the golden-PRL test.
