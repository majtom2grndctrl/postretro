# SH Array Atlas

> **Status:** draft. Direct analogue of the shipped `plans/done/lightmap-array-atlas/`, carried to the
> SH irradiance volume.
> **Related:** `context/lib/build_pipeline.md` §Octahedral irradiance volume bake, §PRL section IDs
> (ids 34/35) · `context/plans/done/lightmap-array-atlas/` (the template) ·
> `context/research/spatial-streaming.md` §2/§5/§6/§8 (SH is the first per-cluster residency target).

## Problem

Large maps are refused their SH irradiance volume at load. The GPU representation of the baked SH
volume is a **2D octahedral atlas**, not a 3D texture: one Rgba16Float base tile atlas
(`OctahedralShVolume`, PRL id 34) and one BC6H direct tile atlas (`DirectShVolume`, PRL id 35), each
packing one octahedral tile per probe into a near-square 2D layout. The atlas side is
`ceil(sqrt(total_probe_count)) × tile_dimension` (`crates/level-format/src/octahedral.rs:78–111`),
and the probe count is **volumetric** — `grid_x × grid_y × grid_z` over the map's AABB
(`crates/level-compiler/src/sh_bake.rs:361–368`). On warren-scale maps the atlas side exceeds the
device `max_texture_dimension_2d` (engine-required floor 8192,
`crates/renderer/src/render/renderer_init_resources.rs`).

When the atlas overflows, the renderer refuses it: the fits-check in `ShVolume::new`
(`crates/renderer/src/render/sh_volume.rs:257–274`) and the sibling check for the direct atlas
(`sh_volume.rs:834–849`) log a `[Renderer]` error and disable the volume — `has_sh_volume = 0`, and
every fragment falls back to `ambient_floor + direct_sum`. The whole map loses baked indirect
lighting because the atlas is one texel too wide.

**The exact limit hit:** `max_texture_dimension_2d` (8192), on the 2D octahedral tile atlases (ids 34
and 35). This is the same overflow the lightmap atlas hit, and it takes the same fix: spill the
oversized single 2D atlas into an N-layer `texture_2d_array`, trading `max_texture_dimension_2d`
pressure for `max_texture_array_layers` headroom (floor 256, already requested in `required_limits`
by the lightmap work).

## Goal

Convert the SH octahedral tile atlases (base, id 34; direct, id 35; and the runtime-composed total
atlas) from a single oversized 2D texture to an N-layer `texture_2d_array`, per-layer capped at 8192²
and layers capped at 256, so maps that currently refuse SH load it instead. Derive the array layer
**in-shader** from the probe's linear index — SH is sampled by world position, not per-vertex, so
there is no free vertex channel to carry a layer (the key departure from the lightmap fix). Keep each
probe's whole octahedral tile (with its existing border padding) on a single layer so the 8-tap
trilinear probe blend never filters across a layer boundary.

## Scope

### In scope

- **Layer-aware octahedral packing math** (`crates/level-format/src/octahedral.rs`): given a per-layer
  `max_dim`, compute `layer_count`, the shared per-layer `(atlas_width, atlas_height)`, `tiles_per_layer`,
  and a tile-location function `probe_index → (layer, tile_x, tile_y)`. Add
  `MAX_SH_ATLAS_LAYERS = 256` (matches the runtime array-layer floor). Whole tiles are the atomic
  packing unit — a tile never straddles a layer.
- **Restructure `OctahedralShVolume` (id 34)** (`crates/level-format/src/sh_volume.rs`): add
  `layer_count`, per-layer `atlas_dimensions`, and a **layer-major** atlas blob (layer 0, layer 1, …).
  Bump `SH_VOLUME_VERSION` (7 → 8); stale sections rejected at parse.
- **Restructure `DirectShVolume` (id 35)** (`crates/level-format/src/direct_sh_volume.rs`): same
  `layer_count` + per-layer dims + layer-major BC6H blob. Bump `DIRECT_SH_VOLUME_VERSION` (1 → 2).
  Direction/geometry reuses id 34's probe layout and layer assignment verbatim.
- **Layer-aware bake** (`crates/level-compiler/src/sh_bake.rs`): pack base and direct tiles into
  layer-major atlas blobs via the new octahedral math; write each probe's tile into its assigned
  layer slice. Single-probe-grid maps that fit one layer emit `layer_count = 1` (bit-for-bit the same
  atlas bytes as today, only the header changes). A grid whose per-layer atlas would exceed one 8192²
  layer opens additional layers; a grid needing more than `MAX_SH_ATLAS_LAYERS` hard-fails the bake.
- **Runtime array-texture pipeline** (`crates/renderer/src/render/sh_volume.rs`): upload the base atlas
  and direct atlas as `D2` textures with `depth_or_array_layers = layer_count` and `D2Array` views; make
  the runtime-composed **total** atlas a `texture_storage_2d_array` write target with a `D2Array`
  sampled view. Change the `BIND_SH_TOTAL_ATLAS` and `BIND_SH_DIRECT_ATLAS` BGL entries from
  `D2` to `D2Array`. Replace the atlas-side of the `ShVolume::new` / direct fits-checks with a
  per-layer-dim + `layer_count ≤ max_texture_array_layers` guard.
- **Compose pass array conversion** (`crates/renderer/src/shaders/sh_compose.wgsl`): read
  `sh_base_atlas` as `texture_2d_array`, write `sh_total_atlas` as `texture_storage_2d_array` —
  `textureStore`/`textureLoad` gain a layer index derived from the composed tile's probe index.
- **Shader layer derivation** (`crates/renderer/src/shaders/sh_sample.wgsl` + the ShGridInfo uniform):
  change the shared `probe_tile_origin` to return `(layer, tile_origin)` from the probe's linear index,
  and `sample_probe_atlas_tex` to sample a `texture_2d_array` with that layer. This one shared helper
  covers all three sampling passes at once (forward fragment, fog compute, billboard vertex) plus the
  mesh superset. Add `tiles_per_layer` / `atlas_layer_count` fields to the mirrored `ShGridInfo`
  structs in `forward.wgsl`, `fog_volume.wgsl`, `billboard.wgsl`, `skinned_mesh.wgsl` (and the
  `GridDims` copy in `sh_compose.wgsl`), kept in std140 lockstep with the Rust-side upload.
- **Depth-moment 3D fits-device guard** (small, separate): the SH depth-moment texture is a genuine 3D
  texture (`sh_volume.rs:920–961`, `TextureDimension::D3`) sized by grid dims; it hits
  `max_texture_dimension_3d` (wgpu default 2048), not the 2D limit, and today has no fits-check. Add a
  guard so an oversized moment texture disables SH gracefully (like the atlas refusal) instead of
  failing at `write_texture`. This is a guard only — no tiling.
- Update `context/lib/build_pipeline.md` (ids 34/35 descriptions, §Octahedral irradiance volume bake)
  to document the multi-layer atlas layout.

### Out of scope

- **SDF atlas tiling.** The `SdfAtlas` (id 33) 3D texture also overflows `max_texture_dimension_3d`
  and panics rather than refusing; fixing it (3D tiling / virtual addressing) is a separate later plan.
  Not touched here beyond leaving it as-is.
- **Depth-moment 3D tiling.** This plan only adds a *fits-device guard* for the moment texture; very
  large grids still lose SH (gracefully). The real fix (3D moment tiling) rides with the SDF-tiling plan.
- **Streaming / per-cluster residency.** Cell-aligned octahedral layers *lay the seam* for the SH
  residency work (`spatial-streaming.md` §8 slice 4), and the layer-packing should be designed
  cluster-friendly (see Decisions), but actual load/evict of layer ranges is out of scope.
- **`DeltaShVolumes` (id 27).** Animated-light indirect deltas are sparse CSR sub-blocks uploaded as a
  storage buffer, not a 2D atlas; they don't hit the 2D limit. Unchanged.
- **Bake time.** SH bake is ~3.5 h on warren-scale maps — a separate perf thread. This plan does not
  scope bake-time reduction; if anything the layer-major write is neutral-to-marginal.
- **Probe-grid density or SH order changes.** Same probe grid, same tile geometry
  (`tile_dimension = 6`, `tile_border = 1`); only the 2D→array packing changes.

## Acceptance criteria

- [ ] `cargo build -p postretro && cargo build -p postretro-level-compiler` compiles clean with no
  warnings.
- [ ] `cargo test -p postretro-level-format` passes. Includes: layer-aware octahedral-packing unit
  tests (per-layer dims ≤ `max_dim`; `layer_count` and `tiles_per_layer` correct for a grid that
  overflows one layer; `probe_index → (layer, x, y)` round-trips and each tile lands wholly on one
  layer); a round-trip test for the multi-layer `OctahedralShVolume` v8 and `DirectShVolume` v2 wire
  formats covering single-layer (`layer_count = 1`) and multi-layer cases; a test asserting the prior
  section versions (7 / 1) are rejected with the existing version-mismatch error.
- [ ] `cargo test -p postretro-level-compiler` passes, including a test that bakes (or packs) a
  synthetic probe grid large enough to overflow one 8192² layer and asserts a deterministic
  multi-layer result: layer count > 1, every per-layer dimension ≤ 8192, and each probe's tile on
  exactly one layer.
- [ ] A large map that **currently refuses** SH (logs the `sh_volume.rs:257–274` `[Renderer]` error)
  now loads its SH volume: `has_sh_volume = 1`, no refusal log, indirect lighting visible. Verified
  manually via `cargo run -p xtask -- run <overflowing-map>.prl`.
- [ ] Existing maps are unchanged: a single-layer bake emits `layer_count = 1` and renders
  identically (same probe values, same lit result) to the pre-change build. `campaign-test.prl` loads
  and renders with no `[Renderer]` SH error.
- [ ] No added sampled-texture binding. The `BIND_SH_TOTAL_ATLAS` and `BIND_SH_DIRECT_ATLAS` entries
  change `D2 → D2Array` **in place**; the SH group-3 / group-4 BGL binding *count* is unchanged, so the
  shared layout stays within Metal's 16-sampled-texture-per-stage budget (a test asserting the SH BGL
  entry count is unchanged, or that the sampled-texture count is unchanged, guards this).
- [ ] The atlas fits-device guard is a pure, unit-tested helper: it returns "unusable" (and logs a
  `[Renderer]` error) when `layer_count > max_texture_array_layers` **or** a per-layer dimension
  exceeds `max_texture_dimension_2d`, and "usable" for a conformant section. On a spec-compliant
  adapter (per-layer ≤ 8192, layers ≤ 256) it never fires — the bake caps guarantee it.
- [ ] The depth-moment 3D texture guard disables SH (no crash, `[Renderer]` error logged) when the
  grid's moment-texture dimensions exceed `max_texture_dimension_3d`; unit-tested as a pure helper
  (no real adapter exposes a sub-2048 3D limit to exercise the full path).
- [ ] `prl_loader` (or the SH load log) emits the atlas dimensions **and** layer count at `info`,
  e.g. `[PRL] SH volume: {w}x{h} atlas, {n} layer(s), {probes} probes`.

## Tasks

### Task 1: Layer-aware octahedral packing math

In `crates/level-format/src/octahedral.rs`: add layer-aware analogues of
`irradiance_atlas_tiles_per_row` / `irradiance_atlas_dimensions` / `irradiance_tile_origin` that take a
per-layer `max_dim` and produce `layer_count`, a shared per-layer `(atlas_width, atlas_height)`,
`tiles_per_layer`, and `tile_location(probe_index) → (layer, tile_x, tile_y)`. Tiles pack in
x-fastest probe order into a near-square per-layer grid capped at `max_dim`; when the next tile would
exceed the per-layer tile budget, open a new layer. Add `const MAX_SH_ATLAS_LAYERS: u32 = 256`. Keep
the existing single-atlas functions intact until Task 3/4 repoint their callers (or provide the new
math as the general form with the old ones as the `layer_count == 1` special case). Whole tiles are
atomic — never split across layers. Add unit tests per the AC (per-layer-dim bound, layer assignment,
tile-on-one-layer, deterministic order). Include a note distinguishing the **atlas array layer** this
introduces from the compiler's per-light incremental-bake "layer" concept — do not conflate them.

### Task 2: Restructure OctahedralShVolume (id 34) and DirectShVolume (id 35) for multi-layer

In `crates/level-format/src/sh_volume.rs`: add `layer_count` and per-layer `atlas_dimensions` to the
section; change the atlas payload to a **layer-major** blob (layer 0 fully, then layer 1, …), each
layer `atlas_width × atlas_height` texels in the declared format (Rgba16Float). Bump
`SH_VOLUME_VERSION` 7 → 8; `from_bytes` rejects version ≠ 8 with the existing named mismatch error.
Single-layer bakes write `layer_count = 1`; the v8 layout is always used. Update `to_bytes`,
`from_bytes`, `placeholder`, and the section's tests.

In `crates/level-format/src/direct_sh_volume.rs`: the same restructure for the BC6H direct atlas —
`layer_count`, per-layer dims, layer-major BC6H blob (each layer's block rows concatenated in layer
order). Bump `DIRECT_SH_VOLUME_VERSION` 1 → 2; reject version ≠ 2. The direct atlas reuses id 34's
probe layout and per-probe layer assignment verbatim (same tile geometry, same
`probe_index → layer`), so it carries no independent packing decision — only the same `layer_count`.

Update the wire-format descriptions in `context/lib/build_pipeline.md` (ids 34/35).

### Task 3: Layer-aware SH bake

In `crates/level-compiler/src/sh_bake.rs`: resolve each probe's `(layer, tile_x, tile_y)` from Task 1's
`tile_location` and write its base tile into the layer-major base-atlas blob at the layer's slice
offset (`layer × atlas_width × atlas_height` texels + within-layer tile origin). Emit `layer_count`
and per-layer `atlas_dimensions` onto the id 34 section. Do the same for the direct atlas (id 35) —
reuse the identical layer assignment; the BC6H encoder is per-image, so encode each layer's per-layer
atlas independently and concatenate the encoded blocks in layer order (mirrors the lightmap plan's
per-layer BC6H loop). A grid needing more than `MAX_SH_ATLAS_LAYERS` layers is a hard bake error
(named, analogous to `LayerOverflow`); a single probe grid whose per-layer atlas can't fit one layer
cannot happen (tiles are fixed 6×6 and always fit). Add the compiler-side multi-layer test from the AC
(synthetic overflowing grid → deterministic multi-layer packing). The bake stays deterministic:
layer-major writes are order-preserving, so the byte-identity / determinism invariant
(`build_pipeline.md` §Determinism invariant) holds — same inputs, same layer-major bytes.

### Task 4: Runtime array texture pipeline

In `crates/renderer/src/render/sh_volume.rs`:
- `upload_atlas_texture` (base, ~762) and `upload_direct_atlas_texture` (~829): create the texture with
  `depth_or_array_layers = section.layer_count`, `dimension = D2`, and a `D2Array` view; upload the
  layer-major blob (one `write_texture` per layer, or one call over the array). BC6H per-layer 4-block
  alignment already handled per layer.
- `create_total_atlas_texture` (~965): create with `layer_count` array layers; the storage view becomes
  `D2Array` and the sampled view `D2Array`.
- `sh_bind_group_layout_entries` (~634): change `BIND_SH_TOTAL_ATLAS` and `BIND_SH_DIRECT_ATLAS`
  `view_dimension` from `D2` to `D2Array`. **No binding is added** — dimension changes in place.
- Replace the atlas-side of the `ShVolume::new` fits-check (257–274) and the direct fits-check
  (834–849) with a guard against per-layer `atlas_dimensions ≤ max_texture_dimension_2d` **and**
  `layer_count ≤ max_texture_array_layers`. Factor the comparison into a pure helper and unit-test it
  (no conformant adapter exercises the fail path). Keep the `has_sh_volume = 0` /
  `has_direct = 0` fallback for the (now corrupt-section-only) refusal case.
- Add the **depth-moment 3D fits-device guard**: before `upload_depth_moment_texture` (920–961),
  check the grid dims against `max_texture_dimension_3d`; on overflow, disable SH (same fallback path
  as the atlas refusal) and log a `[Renderer]` error. Pure helper, unit-tested.
- Upload the new `ShGridInfo` fields (`tiles_per_layer` / `atlas_layer_count`) alongside the existing
  atlas metadata.

In `crates/renderer/src/render/renderer_init_resources.rs`: no new limit — the lightmap work already
sets `max_texture_array_layers: REQUIRED_MAX_TEXTURE_ARRAY_LAYERS` (256) in `required_limits` and adds
the adapter pre-check. Confirm 256 layers covers the SH need at 8192²/layer (it does: 256 × 8192² tiles
≫ any shippable probe grid) and reuse it. If the SH tile atlas ever needs a distinct floor, note it —
but 256 is shared and sufficient.

In `crates/renderer/src/render/prl_loader`-equivalent SH load path: extend the `info!` log to emit
`{n} layer(s)` per the AC.

### Task 5: Shader layer derivation across the sampling passes and compose

In `crates/renderer/src/shaders/sh_sample.wgsl`:
- Change `probe_tile_origin(idx)` to compute the probe's linear index, then derive
  `layer = tile_slot / tiles_per_layer` and the within-layer `tile_origin`, returning both.
- Change `sample_probe_atlas_tex` to take/sample a `texture_2d_array<f32>` with the derived layer as
  the array index; `sh_total_atlas` and the passed `direct_atlas` become `texture_2d_array<f32>`.
- The 8-tap corner loops (`sample_sh_indirect_corners_pair`,
  `sample_sh_indirect_direct_corners`) are unchanged in structure — each corner already resolves its
  own probe `idx` and now its own layer independently, so a blend that spans probes on different layers
  simply issues 8 independent single-layer fetches. No hardware filtering crosses a layer boundary
  (each tile + its 1-texel border lives wholly on one layer), so there is **no new seam**.

This one shared helper is included by `forward.wgsl` (fragment), `fog_volume.wgsl` (compute), and
`billboard.wgsl` (vertex), plus the `skinned_mesh.wgsl` mesh superset — so the layer derivation lands
in all sampling passes at once. In each of those shaders, add the new field(s) (`tiles_per_layer`,
`atlas_layer_count`) to the mirrored `ShGridInfo` struct, matching the Rust upload order and std140
padding. (The `atlas_dimensions` in the uniform is now the **per-layer** dimension — confirm every
reader divides by per-layer dims, which `sample_probe_atlas_tex` already does via
`textureDimensions(atlas)`; the array `textureDimensions` returns per-layer extent, so that path stays
correct.)

In `crates/renderer/src/shaders/sh_compose.wgsl`:
- `sh_base_atlas`: `texture_2d<f32>` → `texture_2d_array<f32>`; `sh_total_atlas`:
  `texture_storage_2d<rgba16float, write>` → `texture_storage_2d_array<rgba16float, write>`.
- The compose kernel maps a workgroup thread to an atlas texel; derive the layer from the composed
  tile's probe index (same `tile_location` math, hand-mirrored) and pass it to `textureLoad`(base)/
  `textureStore`(total). Add `tiles_per_layer` / `atlas_layer_count` to the `GridDims` uniform.
- Dispatch dimensions: composing per-layer means the dispatch grid is per-layer `(atlas_width,
  atlas_height)` × `layer_count`; adjust the workgroup dispatch so every layer's texels are covered.

## Sequencing

**Phase 1 (sequential):** Task 1 (packing math), then Task 2 (wire formats consuming the math) —
the format/packing contracts everything downstream compiles against.

**Phase 2 (concurrent):** Task 3 (bake emits multi-layer sections) and Task 4 (runtime array upload
+ BGL + fits guards) — independent once Tasks 1–2 land.

**Phase 3:** Task 5 (shader layer derivation + compose array), consuming Task 4's `D2Array` BGL and the
`ShGridInfo` uniform fields uploaded in Task 4. Compose-pass changes and sampling-pass changes land
together since they share `tile_location` semantics.

## Wire format

### OctahedralShVolume (PRL id 34), version 8

Header adds `u32 layer_count` (≥ 1); `atlas_dimensions` is now the **per-layer** `(width, height)`
(pow2, per-layer ≤ 8192). Probe metadata records (validity, f16 E[d], f16 E[d²]) are unchanged and
still x-fastest `probe_index = x + y·grid_x + z·grid_x·grid_y`. The atlas blob is **layer-major**:
layer 0's full per-layer atlas, then layer 1, … `layer_count − 1`; each layer is
`atlas_width × atlas_height` Rgba16Float texels row-major. Tile placement within a layer:
`tile_slot = probe_index − layer × tiles_per_layer`, `tile_x = (tile_slot % tiles_per_row)·tile_dim`,
`tile_y = (tile_slot / tiles_per_row)·tile_dim`, `layer = probe_index / tiles_per_layer`. Parsers
reject `version ≠ 8`.

### DirectShVolume (PRL id 35), version 2

Same `layer_count` + per-layer `atlas_dimensions` addition; layer-major BC6H blob (each layer encoded
independently as its own per-layer image, concatenated in layer order). Reuses id 34's probe layout
and per-probe layer assignment — no independent packing. Parsers reject `version ≠ 2`.

## Decisions

- **Layer derived in-shader from probe index, not carried per-vertex.** SH is sampled by world
  position at fragment/probe granularity — there is no vertex channel to piggyback a layer on (the
  lightmap's `lightmap_layer` per-vertex trick does not transfer). The layer is a pure function of the
  probe's linear index and the `tiles_per_layer` uniform, computed in the shared `probe_tile_origin`.
  One helper change covers all three sampling passes plus compose.
- **Whole tile on one layer = the interpolation invariant.** The analogue of the lightmap's "a BVH
  leaf's charts stay on one layer." Tiles are fixed-size (6×6 incl. border) and are the atomic packing
  unit, so a probe's tile never straddles a layer. The 8-tap trilinear blend reads 8 probes that may
  live on different layers, but each tap is an independent single-layer fetch within one padded tile —
  no hardware filtering crosses layers, so array packing introduces no seam. This is strictly simpler
  than the lightmap case (variable-size charts); no MaxRects-style packer is needed — near-square
  per-layer fill suffices.
- **Reuse the 256-layer floor.** `max_texture_array_layers ≥ 256` is already in `required_limits` and
  pre-checked (shipped by lightmap-array-atlas). 256 layers × 8192²/layer dwarfs any shippable probe
  grid, so no new limit is introduced and the guard never fires on a conformant adapter.
- **Total atlas is runtime-composed — the compose pass is a fourth touch-point.** Unlike the lightmap
  (a static uploaded texture), the SH indirect atlas the passes sample (`sh_total_atlas`) is written
  every frame by the compose compute pass from `sh_base_atlas` + animated deltas. So `D2Array` must
  cover a storage-texture-array write (`texture_storage_2d_array`) and per-layer dispatch, not just a
  sampled-texture read. This has no lightmap analogue and carries the main incremental risk (below).
- **Cluster-friendly layer packing (streaming seam, not scoped here).** `spatial-streaming.md` §2/§8
  names SH the heaviest baked artifact and the first per-cluster residency target. Keep the
  `probe_index → layer` mapping a contiguous linear-index range per layer so a future pass can align
  layer boundaries to cell/cluster probe ranges and load/evict layer ranges. Design for it; do not
  build it.
- **Direct atlas rides the base atlas's layout.** id 35 reuses id 34's probe grid, tile geometry, and
  per-probe layer assignment — one packing decision, two atlases — mirroring how the shipped code
  already shares tile math between the indirect and direct octahedral atlases.

## Risks

- **Compose-pass storage-texture-array support / correctness.** `texture_storage_2d_array` writes and
  per-layer dispatch are the one genuinely new GPU surface (no lightmap precedent). Verify wgpu
  downlevel/backend support and that the per-layer dispatch covers every texel exactly once. Mitigate
  with a compose round-trip check (dev-tools reads back the composed total atlas; assert it matches a
  CPU compose for a small multi-layer grid).
- **Interpolation seams.** Argued away by the whole-tile-on-one-layer invariant + independent per-tap
  fetches (see Decisions). Residual risk is a mistaken cross-layer `textureDimensions`/UV divisor —
  guard by keeping the per-layer-dims-only normalization (`textureDimensions` on an array returns
  per-layer extent) and a visual A/B on an overflowing map vs. a forced single-layer bake of a smaller
  map.
- **Depth-moment 3D still has a hard ceiling.** The moment texture is a real 3D texture bounded by
  `max_texture_dimension_3d` (2048); this plan only adds a graceful guard, so very large grids still
  lose SH (cleanly, not crashing). The true fix (3D moment tiling) is deferred to the SDF-tiling plan.
- **std140 uniform drift.** `ShGridInfo` is hand-mirrored in four shaders plus the compose `GridDims`.
  Adding fields risks silent layout drift. Mitigate by adding the field at the end with explicit
  padding and (if one exists) extending the shader/Rust layout-parity test that already pins the SH
  uniform.

## Related work

- **`context/plans/done/lightmap-array-atlas/`** — the template this mirrors. Same overflow, same
  D2→D2Array trade, same 256-layer floor; differs in layer-carrying (in-shader vs. per-vertex), touch
  count (3 passes + compose vs. 1), and packer complexity (near-square fill vs. leaf-aware MaxRects).
- **`context/research/spatial-streaming.md` §5/§6/§8** — SH is the heaviest baked artifact (§2) and
  the first per-cluster residency target (§8 slice 4); §6 notes the array-atlas layer structure is the
  half-built substrate residency leverages. This plan lays the cell-alignable layer seam; residency is
  a follow-on epic.
- **SDF-atlas / depth-moment 3D tiling** — a separate later plan for the `max_texture_dimension_3d`
  ceiling (SdfAtlas id 33 panics today; the SH moment texture gets only a guard here).
- **SH bake-time reduction (~3.5 h warren-scale)** — a distinct perf thread; explicitly a non-goal here.
