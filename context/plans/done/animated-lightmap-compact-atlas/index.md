# animated-lightmap-compact-atlas

Brief · compact · reads: `context/lib/rendering_pipeline.md` (animated lightmap), `context/lib/build_pipeline.md` §PRL section IDs, §Build Cache, `context/lib/development_guide.md` §1.4 · read at f95d476ac

**Sequence: 1 of 4.** No dependency on any brief in the series or on `E23--preferences-comfort-floor`, whose changes in the crates the Boundary inventory names are limited to the flash limiter and screen effects. Later briefs do not depend on it.

## Problem
The owner wants PostRetro to run well on laptop GPUs. Basis: an anticipated need backed by a measured waste. The animated lightmap atlas pair is the largest lightmap-shaped VRAM consumer: 144 MiB on `campaign-test`. Cause: forward samples it with the static lightmap UV, so each animated slot is a full static-layer-sized array layer, while its chunks cover about a fifth of those texels. Nothing in the dev panel or load log reports lightmap-family bytes, so the waste was found by parsing PRLs by hand. When done: the atlas holds only the texels animated faces use, packed at compile time. `campaign-test` lands near a quarter of today's bytes and renders the same frame to within one 8-bit step. The dev panel and load log report resident bytes for each lightmap-family texture.

## Decisions
- **Compile-time repack.** prl-build packs one block per animated face into a compact atlas and writes it into section 25 v4. Placement: baked over computed (`index.md` §2). Undo cost: a section version and stage-version bump.
- **Uniform pages.** The compact atlas is an array of pages of one size, a power of two the compiler picks: at least the largest block, at most the static layer size (the lightmap section's layer width), and at least 1024² unless the static layer is smaller. When the static lightmap is the placeholder, the level keeps today's no-animated-light path and the bound is not checked. Blocks pack in cell order. The compiler never writes a packed layout larger than the identity layout (below); if packing would be larger, it writes the identity layout. Page quantization costs at most one partial page over a free extent; a page is a natural load-and-evict unit for `context/plans/large-map-spatial-residency.md` stage 5; and a free extent must reallocate to grow.
- **Block equals chart placement.** A block is the face's static chart placement rect, including the existing chart padding, which is the gutter. The gutter stays zero and is never composed, matching today's zero-initialized atlas outside chunk rects, so the frame matches today's.
- **Identity layout as reference.** A block table that leaves every block at its static position is a valid v4 layout that reproduces today's full-layer atlas exactly: one page per static layer that still holds a chunk after the cull, in ascending static-layer order, at the static size. So "never larger than identity" means never larger than today. The weight-map stage caches its payload in this layout, and the renderer's parity test renders it as the reference.
- **The compiler enforces what the repack relies on, and fails the build otherwise.** No vertex is shared across blocks. Every bilinear footprint stays inside its face's placement rect. The block count stays under one cap that the compiler and the forward shader's block table share.
- **Forward remap without a new binding.** Each vertex carries its animated block id in the geometry vertex's pad field, with 0 meaning none. The group-4 binding-7 uniform grows from a slot table into a block table, which the fragment stage reads through a flat varying. Binding 7 stays FRAGMENT-only and the storage and sampled binding counts are unchanged, respecting the forward binding wall. This follows `large-map-spatial-residency.md`, whose stage-5 notes resolve per-face animated blocks in the fragment stage from the binding-7 table.
- **Strict versions.** The loader rejects section 25 v2 and v3 with a recompile error, following the exact-match rule in `build_pipeline.md` §PRL section IDs. The animated weight-map stage version bumps, because its cached payload becomes v4 in the identity layout (§Build Cache).
- **Block-id mismatch.** A vertex whose block id disagrees with the block table, or names a block past it, fails the load with a recompile error in debug builds and in any build with `dev-tools`. A player release build without `dev-tools` loads the level with no animated light and logs an error.
- **Lightmap-family byte meter.** The dev panel, the load log and the capture report show resident GPU bytes separately for:
  - static irradiance;
  - static direction;
  - shadowmask;
  - animated irradiance;
  - animated direction.

  It generalizes the existing per-resource resident-byte ledger (`ShResidencyReport`) rather than starting a second accounting model. It is the brief's own proof, and fills part of the gap in `large-map-spatial-residency.md` §Pre-planning measurements.
- **Non-goals.**
  - Streaming or visibility-driven pooling of the atlas. That is stage 5 of `large-map-spatial-residency.md`, which builds on this layout.
  - Smaller texel formats. Storage-writable formats in core wgpu leave little to gain once the atlas is compact.
  - Filling the gutter by edge-replicate. That would remove today's darkened animated-edge fringe, which is a visual change for the owner to decide separately.
  - Any change to the static lightmap, shadowmask or direction atlases. They are not the largest class, and stage 5 owns their residency.

## Acceptance
### Automated
**Pixel parity**
- [ ] A lasting GPU test in the existing GPU test harness renders the same scene with the identity block table and with the packed one, animated lights forced on, and every pixel matches within one 8-bit step. The same test shows the forced lights change the frame; a render where no animated light contributes does not count.
- [ ] With the identity block table, every chunk composes at its static layer and position, and forward samples each face at its static UV.
- [ ] The repack changes only where chunks sit. Per-texel weights and light lists are byte-identical before and after it. Each block spans its face's full chart placement, and every chunk keeps the same offset inside its block in both spaces. (pin P4)
- [ ] A level with no animated lights writes no animated sections, every vertex carries no block id, it renders as before, and the meter shows the animated pair at placeholder size. (pin P5)

**Repack**
- [ ] A face whose every chunk the unlit cull drops gets no block, and its vertices carry no block id. A level whose chunks are all dropped writes no animated sections. (pin P3, P5)
- [ ] A face with several chunks, some of them culled, gets exactly one block. Culled chunk texels inside it stay zero. (pin P4)

**Compiler guards**
- [ ] A vertex shared across two blocks fails the build.
- [ ] A bilinear footprint past its face's placement rect fails the build. A footprint just inside it passes.
- [ ] A block count over the uniform cap fails the build, naming the cap and the count. A count at the cap passes.
- [ ] The compiler's block cap equals the block-table capacity the forward shader declares. That table fits the uniform size the renderer requests, and the cap never exceeds what a vertex's 16-bit block id can name. (pin P10)

**Format and cache**
- [ ] Section 25 v4 round-trips. v2 and v3 are rejected with a recompile error.
- [ ] The loader rejects a section whose page size is not a power of two or lies outside the page-size bounds. (pin P14)
- [ ] A level with animated lights and a placeholder static lightmap loads with no animated light, as today; the page-size upper bound is not checked. (pin P14)
- [ ] The identity layout allocates exactly today's slot count for the level: one page per static layer that holds a chunk after the cull.
- [ ] A warm build over a cache written before this change re-bakes the weight-map stage.
- [ ] Editing only an animated light's properties leaves the SDF atlas stage cached. (pin P1)
- [ ] A warm rebuild whose weight-map stage hits the cache writes section 25 and the geometry section byte-identical to a cold build. (pin P2)
- [ ] Vertices with no animated block carry no block id (0 means none) and take today's no-animated-light path.

**Block-id mismatch** (pin P11)
- [ ] In a debug build, and in any build with `dev-tools`, a level where a vertex's block id disagrees with the block table, or names a block past it, fails to load with a recompile error.
- [ ] In a player release build without `dev-tools`, the same level loads with no animated light and logs one error.

**Forward remap**
- [ ] When the animated atlas falls back to the placeholder, or has nothing to compose, every vertex resolves to no block, whatever ids the level carries. (pin P6)
- [ ] No two blocks overlap on a page, and every block and chunk lies inside its page.
- [ ] The geometry vertex stays 36 bytes on disk and in the vertex buffer. The forward pass's storage and sampled binding counts are unchanged, and the block table is visible to the fragment stage only.

**Size**
- [ ] The page size is a power of two within the Decision's bounds.
- [ ] The page count is the fewest pages the packer fills in cell order, and no page is empty.
- [ ] A level whose blocks fit on one page allocates one page, however many static layers carry animated faces.
- [ ] When blocks overflow one page, they spill into a second. Compose and forward resolve every block to the same page and offset, including blocks on the second page. (pin P9)
- [ ] A block as large as a whole page packs alone at that page's origin, and the next block starts a new page. (pin P8)
- [ ] On every level with animated faces, the animated atlas bytes never exceed the identity layout's. A level where packing would be larger ships the identity layout.
- [ ] On `campaign-test`, the capture report's animated irradiance and direction bytes each equal the level's page count times one page's bytes, and fall below the meter's reading on the full-layer build. (pin P13)

**Byte meter**
- [ ] The load log reports bytes for each lightmap-family texture the byte-meter Decision lists. The dev panel and the capture report show the same numbers.
- [ ] After a level unload, every lightmap-family count returns to its placeholder size. (pin P12)
- [ ] When the animated atlas falls back to the placeholder, the meter reports the placeholder's bytes, not those of the atlas it rejected. (pin P7)

### Manual
- [ ] Visual: animated lights on `campaign-test`, `occlusion-test` and `closet-reveal` look unchanged in play, including edges and seams.
- [ ] Resource: record the meter's lightmap-family bytes on `campaign-test`, `occlusion-test` and `stress-warren-mini` on the full-layer build and after the repack, rebuilding stale PRLs first.

## Wire format
Little-endian throughout, like v3. Unsigned 32-bit fields unless stated otherwise. Mirrors v3's layout: header, then fixed records, then pools.

| Part | Fields, in order | Notes |
|---|---|---|
| Header | version = 4, chunk_count, offset_counts_len, texel_lights_len, block_count, page_size, compact_layers | Replaces v3's slot_count. Every page is page_size × page_size, a power of two; the loader rejects a non-power-of-two or out-of-range page size. compact_layers is the page count. A level with no surviving chunk writes no animated sections. |
| Chunk rect × chunk_count | compact_x, compact_y, w, h, texel_offset, block | In compact coordinates; block indexes the block table. Replaces v3's static-space rect and layer. |
| Block × block_count | static_layer, static_x, static_y, compact_x, compact_y, compact_layer, w, h | One per animated face. compact_layer is the page. Static→compact is a translation plus a layer change. |
| Offset counts, texel lights | Unchanged from v3 | |

v3's trailing slot table is dropped. The geometry vertex (section 17) keeps its 36-byte layout. Its u16 pad becomes the animated block id, where 0 means none and n means block n − 1. Section 17 has no version field, so a stale file reads 0 everywhere, and the v4 requirement on section 25 rejects that file anyway.

## Boundary inventory
| Name | Compiler | level-format | Loader / render-cpu | Renderer | WGSL |
|---|---|---|---|---|---|
| Animated block id | stamped per face vertex after the SDF key is hashed | `Vertex` pad field | per-vertex cross-check against the block table; rejects or disables per the mismatch Decision | `Uint16x2` attribute (layer, block) | flat varying |
| Block table | repack output; identity layout in the stage cache | new block record type in section 25 | `validate_cross_section` | binding-7 uniform builder | binding-7 struct |
| Page size and count | repack output | section 25 header | page-size preflight | atlas creation, compose dispatch | unchanged compose; forward UV scale |

## Path
- Writer: `bake_animated_light_weight_maps_controlled` → `cull_unlit_chunks` → `validate_animated_atlas_budget` in the compiler pipeline. The weight bake stays in static-lightmap space, because its visibility seed keys on static coordinates (`soft_visibility_texel_seed`). The repack is a pure coordinate rewrite after the cull, on both cache hit and miss, and replaces the full-layer budget estimate with the page count. It reads `face_charts` and `face_placements` only.
- Vertex stamping must happen after the `sdf_atlas` key hashes `geo_result`. Stamping earlier would make the SDF atlas re-bake for no reason.
- Runtime: `AnimatedLightmapResources::new` / `animated_atlas_extent`, `StaticLayerToAnimatedSlot` (binding 7), `animated_slot_for_static_layer` and `sample_lightmap_animated` in forward.wgsl, `DispatchTile.target_slot`. Compose keeps its shape: chunk rects arrive in compact coordinates and dispatch tiles target pages. A likely block-table layout is the page size, then per-block texel offset and page.
- Cap: derive it once from the uniform binding size the renderer requests and share it with the forward shader's array length; figures in research P16. If a map nears it, merge a cell's faces on one layer into one block. That is not built here, and the guard wording allows it.
- Packer: MaxRects per page in cell order, as `pack_layers` does per layer. An in-order shelf packer overshoots today's bytes on `stress-warren-mini` (research §Measurements). `drafts/bvh-leaf-clustering` assumes one chunk per face; whichever lands second updates the other's assumption.
- Parity harness: `gpu_or_skip` in `shadowmask_sample_test.rs` is the self-skipping GPU test pattern, and the capture scene's `force_active` seeding forces animated lights on.
- First slice: the byte meter against today's full-layer atlas, so a before reading exists. Then the repack with the identity-versus-packed parity test, which tests the riskiest assumption: that zero gutters give pixel parity.
- Many tests pin the slot-table shape. Research lists them. Rewrite them; don't wrap them.
- Rejected rivals:
  - A load-time repack from existing data, with no format change. The owner chose compile time.
  - A vertex-stage remap. It needs VERTEX visibility on binding 7 and gains nothing over a fragment lookup.
- Measurements, invariants data and the test list: `research.md`.

## Open questions
- Where the byte meter lives in the dev panel, and its log line format — **delegated**.
- Whether the unload row needs a new unload harness or can be proven by loading a second level after the first — **delegated**.
