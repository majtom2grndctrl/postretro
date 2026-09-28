# animated-lightmap-compact-atlas

Brief · compact · reads: `context/lib/rendering_pipeline.md` (animated lightmap), `context/lib/build_pipeline.md` §PRL section IDs, §Build Cache, `context/lib/development_guide.md` §1.4 · read at 0a7352039

## Problem
The owner wants PostRetro to run well on laptop GPUs. Basis: an anticipated need backed by a measured waste. The animated lightmap atlas pair is the largest lightmap-shaped VRAM consumer: 144 MiB on `campaign-test`. Cause: forward samples it with the static lightmap UV, so each animated slot is a full static-layer-sized array layer, while its chunks cover about a fifth of those texels. Nothing in the dev panel or load log reports lightmap-family bytes, so the waste was found by parsing PRLs by hand. When done: the atlas holds only the texels animated faces use, packed at compile time. `campaign-test` lands near a third of today's bytes (one layer instead of three) and renders identical pixels. The dev panel and load log report resident bytes for each lightmap-family texture.

## Decisions
- **Compile-time repack.** prl-build packs one block per animated face into a compact atlas and writes it into section 25 v4. The weight bake stays in static-lightmap space, because its visibility seed keys on static coordinates. The repack is a pure coordinate rewrite after the unlit-chunk cull. Placement: baked over computed (`index.md` §2). Undo cost: a section version and stage-version bump.
- **Compact layers keep the static layer's size.** Blocks pack into as many static-layer-sized layers as they need. A later merge of the static and animated direction arrays then stays shape-compatible (`plans/done/static-light-shadowmask-world-receipt`). The tightest free extent would save about another third on `campaign-test`. It loses because it closes that door.
- **Block equals chart placement.** A block is the face's static chart placement rect, including the existing chart padding. That padding is the gutter. It stays zero and is never composed, matching today's zero-initialized atlas outside chunk rects. Result: pixel parity with today.
- **The compiler enforces what the repack relies on, and fails the build otherwise.** Each block sits on one static layer. No vertex is shared across blocks. Every bilinear footprint stays inside its block plus gutter. The block count stays under the binding-7 uniform cap. The cap is derived from the uniform binding limit the renderer requests, which is `wgpu::Limits::default()` today (64 KiB). Current content is far below it. If a map ever reaches it, the remedy is merging a cell's faces on one layer into one block. That is not built here, and the guard wording already allows it.
- **Forward remap without a new binding.**
  - Each vertex carries its animated block id in the geometry vertex's existing pad field, with 0 meaning none. Stride and on-disk size are unchanged.
  - The group-4 binding-7 uniform grows from a slot table into a block table: compact extent, plus per-block texel offset and compact layer.
  - The fragment stage resolves the block from a flat varying. Binding 7 stays FRAGMENT-only, and storage and sampled binding counts are unchanged. This respects the forward binding wall.
  - This departs from `context/plans/large-map-spatial-residency.md`, which leans toward a vertex-stage indirection at layer or cluster granularity. Per-face blocks make the animated-slot-count worry in that plan irrelevant, and a fragment lookup costs the same as today's slot lookup. The same change updates that plan's stage-5 notes.
- **Compose is unchanged in shape.** Chunk rects arrive in compact coordinates, and dispatch tiles target compact layers.
- **Strict versions.** The loader rejects section 25 v2 and v3 with a recompile error, following the exact-match rule in `build_pipeline.md` §PRL section IDs. The animated weight-map stage version bumps, because its cached payload encoding changes (§Build Cache).
- **Lightmap-family byte meter.** The dev panel and the load log report resident GPU bytes separately for:
  - static irradiance;
  - static direction;
  - shadowmask;
  - animated irradiance;
  - animated direction.

  It generalizes the existing per-resource resident-byte ledger (`ShResidencyReport`) rather than starting a second accounting model. This is the brief's own proof. It also fills part of the gap in `context/plans/large-map-spatial-residency.md` §Pre-planning measurements.
- **Non-goals.**
  - Streaming or visibility-driven pooling of the atlas. That is stage 5 of `large-map-spatial-residency.md`, which builds on this layout.
  - Smaller texel formats. Storage-writable formats in core wgpu leave little to gain once the atlas is compact.
  - Filling the gutter by edge-replicate. That would remove today's darkened animated-edge fringe, which is a visual change for the owner to decide separately.
  - Any change to the static lightmap, shadowmask or direction atlases.

## Acceptance
### Automated
**Pixel parity**
- [ ] With identical animation state, a before/after frame capture on `campaign-test` and `stress-warren-mini` is pixel-identical. The diff is compared against the last full-layer build.
- [ ] A level with no animated lights builds and renders as before, with an empty block table and no atlas memory beyond today's placeholder.

**Compiler guards**
- [ ] A face whose block would span two static layers fails the build with a message naming the face.
- [ ] A vertex shared across two blocks fails the build.
- [ ] A bilinear footprint past block plus gutter fails the build. A footprint just inside it passes.
- [ ] A block count over the uniform cap fails the build, naming the cap and the count. A count at the cap passes.

**Format and cache**
- [ ] Section 25 v4 round-trips. v2 and v3 are rejected with a recompile error.
- [ ] A warm build after the stage-version bump re-bakes the weight-map stage and does not re-bake the SDF atlas.
- [ ] Every vertex's block id agrees with the block table's static layer, or the loader rejects the level.
- [ ] Vertices with no animated block read block 0 and take today's no-animated-light path.

**Size**
- [ ] Compact layers have the static layer's dimensions. The layer count is the fewest that hold every block, with no layer allocated per static layer.
- [ ] A level whose blocks fit in one layer allocates one layer, however many static layers carry animated faces.

**Byte meter**
- [ ] The load log reports bytes for each of the five lightmap-family textures. The dev panel shows the same numbers.
- [ ] After a level unload, every lightmap-family count returns to its placeholder size.

### Manual
- [ ] Visual: animated lights on `campaign-test`, `occlusion-test` and `closet-reveal` look unchanged in play, including edges and seams.
- [ ] Resource: record lightmap-family bytes from the meter on `campaign-test`, `occlusion-test` and `stress-warren-mini`, before and after, rebuilding stale PRLs first. Expect `campaign-test`'s animated pair to fall to about a third.

## Wire format
Little-endian throughout, like v3. Unsigned 32-bit fields unless stated otherwise. Mirrors v3's layout: header, then fixed records, then pools.

| Part | Fields, in order | Notes |
|---|---|---|
| Header | version = 4, chunk_count, offset_counts_len, texel_lights_len, block_count, compact_width, compact_height, compact_layers | Replaces v3's slot_count. Width and height equal the static layer's; the loader rejects a mismatch. An empty block table has block_count 0 and compact_layers 0. |
| Chunk rect × chunk_count | compact_x, compact_y, w, h, texel_offset, block | In compact coordinates; block indexes the block table. Replaces v3's static-space rect and layer. |
| Block × block_count | static_layer, static_x, static_y, compact_x, compact_y, compact_layer, w, h | One per animated face. Static→compact is a translation plus a layer change. |
| Offset counts, texel lights | Unchanged from v3 | |

v3's trailing slot table is dropped. The geometry vertex (section 17) keeps its 36-byte layout. Its u16 pad becomes the animated block id, where 0 means none and n means block n − 1. Section 17 has no version field, so a stale file reads 0 everywhere, and the v4 requirement on section 25 rejects that file anyway.

## Boundary inventory
| Name | Compiler | level-format | Loader / render-cpu | Renderer | WGSL |
|---|---|---|---|---|---|
| Animated block id | stamped per face vertex after the SDF key is hashed | `Vertex` pad field | per-vertex cross-check against the block table | `Uint16x2` attribute (layer, block) | flat varying |
| Block table | repack output | new block record type in section 25 | `validate_cross_section` | binding-7 uniform builder | binding-7 struct |
| Compact extent | repack output | section 25 header | extent preflight | atlas creation, compose dispatch | unchanged compose; forward UV scale |

## Path
- Writer: `bake_animated_light_weight_maps_controlled` → `cull_unlit_chunks` → `validate_animated_atlas_budget` in the compiler pipeline. The repack goes after the cull and replaces the full-layer budget estimate with the compact extent. It reads `face_charts` and `face_placements` only.
- Vertex stamping must happen after the `sdf_atlas` key hashes `geo_result`. Stamping earlier would make the SDF atlas re-bake for no reason.
- Runtime: `AnimatedLightmapResources::new` / `animated_atlas_extent`, `StaticLayerToAnimatedSlot` (binding 7), `animated_slot_for_static_layer` and `sample_lightmap_animated` in forward.wgsl, `DispatchTile.target_slot`. The `WorldVertex` layer attribute sits at offset 32.
- First slice: the compiler repack plus a capture diff, before the byte meter or any cleanup. It tests the riskiest assumption, that zero gutters give pixel parity. Measured worst UV overrun is far below 1.5 texels on current content.
- Many tests pin the slot-table shape. Research lists them. Rewrite them; don't wrap them.
- Rejected rivals:
  - A load-time repack from existing data, with no format change. The owner chose compile time.
  - A vertex-stage remap. It needs VERTEX visibility on binding 7 and gains nothing over a fragment lookup.
  - A free compact extent. See Decisions.
  - Block merging now. Deferred until a map nears the cap.
- Pack blocks in cell order, so a later cluster-ordered residency pass inherits some locality. `drafts/bvh-leaf-clustering` assumes one chunk per face; whichever lands second updates the other's assumption.
- Measurements, invariants data and the test list: `research.md`.

## Open questions
- Where the byte meter lives in the dev panel, and its log line format — **delegated**.
