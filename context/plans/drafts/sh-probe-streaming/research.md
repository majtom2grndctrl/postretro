# SH Probe Streaming — Research

Grounding for `index.md`. Read at `c269dd9`. Facts are cited by symbol; this file
holds the investigation, the spec holds the decisions.

## Basis: the frame-rate cause (cure vs symptom)

The motivating observations — stress-warren sluggish on a 6 GB GTX 1660 at production
quality; probe spacing 8 ≈ 2× the frame rate of spacing 1 on campaign-test — are
explained by **resident footprint / memory-bandwidth / texture-cache pressure**, not
per-fragment compute.

- SH indirect sampling is a **fixed 8-corner trilinear stencil** per fragment (≤ 32 taps
  in the straddle fallback), a compile-time constant independent of probe density or total
  probe count. `sample_sh_indirect` → `sample_sh_indirect_corners_pair` /
  `sample_l1_whole_cell_atlas` (`crates/renderer/src/shaders/sh_sample.wgsl`;
  `forward.wgsl:421`). Probe indirection is a direct integer `textureLoad`
  (`sh_probe_indirection`, `sh_sample.wgsl:23`), O(1), no scan over resident data.
- The atlas is a BC6H `texture_2d_array`; texel/tile count = f(total probe count), which
  shrinks ≈ cubically as spacing grows. `irradiance_atlas_dimensions` /
  `irradiance_atlas_tiles_per_row` (`crates/level-format/src/octahedral.rs:79-112`);
  `DEFAULT_PROBE_SPACING = 1.0` (`sh_bake.rs:35`); overflow guard advises raising
  `probe_spacing` to cut layers (`sh_bake.rs:55-56`).
- Conclusion: streaming (fewer tiles resident) relieves exactly this lever and, unlike
  global coarsening, keeps full near-camera density. Its frame-rate win is via
  cache-locality/footprint and is **unmeasured** — no in-repo profiler capture isolates
  cache-miss cost; the 2× figure rests on the recorded sweep in
  `plans/done/lighting-scale--variable-base-probe-density/`. Slice 1 measures it.
  (Confidence: high on the mechanism; the empirical magnitude is what Slice 1 pins.)

## Commitment: whole-load baseline, no partial residency

- `load_prl` → `load_prl_with_section_limits` reads the whole file
  (`std::fs::read`, `crates/level-loader/src/prl_loader.rs:2113`), then `from_bytes` each
  section whole: `GeometrySection` (:2120), `BvhSection` (:2190), `LightmapSection` (:2460),
  `OctahedralShVolumeSection` (:2409), `DirectShVolumeSection` (:2855).
- `Renderer::install_level_geometry` (`renderer_resources.rs:126`) creates each buffer/
  texture whole; SH base via `upload_compact_base_atlas_texture`
  (`render/sh_volume.rs:963`); lightmap one upload all layers (`lighting/lightmap.rs:516`).
- No load/evict-by-area anywhere. Renderer "eviction" is dynamic-light shadow-slot LRU
  (`render/renderer_light_slots.rs`), unrelated. Only per-section binding *floor*:
  `MAX_DELTA_SECTION_BINDING_BYTES = 128 MiB` (`prl_loader.rs:70`), a whole-section
  accept/reject, not a partial read.
- SH-heaviest ranking is asserted by `spatial-streaming.md` §2/§8 + the 128 MiB cap + a
  1.0 m fixture retaining ~177 MiB delta payload after coarsening; **no in-code
  per-section resident-byte tally exists** — Slice 1 builds it. (Confidence: med on the
  ranking; high on whole-load and the cap.)

## Substrate: cells + visible-cell signal (still grounded)

- `cell_id == BSP leaf_index` 1:1; every baked primitive carries it. `cell_id:
  face.leaf_index` (`crates/level-compiler/src/bvh_build.rs:93`); doc
  (`geometry.rs:48-53`); corroborated `cell_draw_index_bake.rs`, `pack.rs`.
- Per-frame visible-cell set: portal traversal `determine_visible_cells` →
  `VisibleCells::Culled(Vec<usize>)` (`crates/visibility/src/visibility.rs`); renderer
  converts to a fixed 4096-word bitmask, `ComputeCull::write_bitmask_from_cells`
  (`crates/renderer/src/compute_cull.rs:264`), `MAX_VISIBLE_CELLS` = 131072, consumed by
  the GPU BVH-traversal cull. Residency rides this signal (plus a wider prefetch set).
- No cell clustering pass exists (`level-compiler/src/pipeline.rs`, zero cluster matches).
- Lightmap packer still groups by leaf: `group_charts_by_leaf`
  (`lightmap_bake.rs:1155`), `pack_layers` (:1063).

## Correction to the research premise (authoring surface)

`spatial-streaming.md` §4 calls author region entities "net-new / greenfield." **Stale.**
`sdk/TrenchBroom/postretro.fgd` already defines author-placed brush region volumes:
`lightmap_scale_region` (:581), `sh_protect_volume` (:593), `fog_volume` (:562). Slice 4
models the streaming-hint entities on this established pattern and its compile-time
validation, not a blank slate.

## adaptive-probe-spacing coupling

Ready brief `plans/ready/lighting-scale--adaptive-probe-spacing/`. Within-chunk stored
density over the same octahedral atlas; **bandwidth-neutral by construction** — no new
binding, no per-fragment locate-read, ≤ 8 taps; id 34 v10→v11, id 35 v3→v4, 8-byte probe
record keeps stride, node scale in reserved bytes, slots by prefix sum, no node table.
Streaming layers residency addressing *over* v11/v4 and must not violate the
bandwidth-neutral contract. Orthogonal by the epic docs: "coarsening reduces data inside a
resident chunk; residency selects which chunks are resident … both remain useful"
(`large-map-spatial-residency.md` §Scope boundary; `spatial-streaming.md` §10).

## Measurement readiness (Slice 1 inputs)

- Reuse: `--sh-analyze` per-SH-section byte accounting + JSON sidecar
  (`sh_analyze.rs`; `main.rs:786`); `POSTRETRO_GPU_TIMING` per-pass GPU time
  (`render/frame_timing.rs`); offscreen capture `Renderer::new_offscreen` /
  `capture_frame_indirect` (`crates/postretro/src/capture/driver.rs:75,164`) — needs a GPU
  adapter, not a display; GPU integration test `tests/capture_frame.rs`.
- Build: whole-level per-section disk dump (only aggregate + largest at
  `pack.rs:1087,1152`); resident-VRAM-by-section (only per-atlas startup estimates today,
  e.g. `render/sh_volume.rs:454`); frame-time/memory on the capture path (PNG-only today).

## Section registry

Highest current `SectionId` = 48 (`AnimatedBillboardDirectScatterDeltaVolumes`,
`crates/level-format/src/lib.rs:319`). Next free id = 49 for the cluster directory. Section
table carries per-section offset+size already (`SECTION_ENTRY_SIZE`, `lib.rs`).

## Direction questions (drafter's pass; /validate-plan re-asks adversarially)

1. **Cause or symptom?** Cause: whole-level resident SH atlas → footprint/bandwidth cost
   independent of camera position. Symptom would be "frame rate low" treated by coarsening
   everywhere. Streaming attacks the resident-set cause while keeping near-camera detail.
   Slice 1 guards against solving a symptom by falsifying the premise first.
2. **Right level?** Axes pinned in the spec's Placement note (load-time vs runtime;
   compiler vs renderer vs off-frame I/O; algorithm-default vs authored). Residency
   selection is runtime; the directory is baked; GPU stays renderer-owned. Flagged for the
   reviewer, not self-cleared.
3. **Forecloses?** Commits the PRL to a per-cluster-addressable directory (one-way-ish:
   format door, but load-rejected + re-bakeable, no external consumers per development_guide
   §1.6). Nothing else material.
4. **Prior commitments touched?** Epic 9 deferral+insurance; the seed's clustered-cell
   substrate ruling; adaptive-probe-spacing's bandwidth-neutral sampler; existing
   `*_region` authoring pattern; renderer-owns-GPU and frame-ordering invariants. All
   adopted, none diverged from.
5. **One-way door + undo cost?** The cluster-directory section is the main door; undo =
   drop the section and re-bake (cheap, no shims, pre-stable). The sampler indirection line
   is the dangerous door — adding a per-fragment locate-read would regress frame time
   silently; held closed by the bandwidth-neutral invariant + budget guard AC.
6. **Strongest alternative?** Geometry-first (seed's other candidate). Rejected in
   Direction: geometry already gets culling relief and simpler semantics, so it neither
   addresses the acute pain nor proves the hard seams. Coarsen-harder rejected as a
   footprint-only lever that cannot lift the scale ceiling.
