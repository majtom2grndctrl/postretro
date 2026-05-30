# Filterable SDF Distance-Field Atlas

## Goal

Eliminate the world-locked, voxel-grid (0.5 m) stair-stepping in per-light SDF
soft shadows by making the baked **fine** distance field hardware-trilinear
filterable. Replace nearest-voxel `textureLoad` sampling with a single
`textureSampleLevel`. This is a field-representation upgrade only — the per-light
K-slice trace, half-res pass, and DDGI-indirect split stay intact.

## Background

Item 3 of the `sdf-per-light-shadows` shadow-quality follow-on (parent
`research.md`, "Shadow-quality follow-on" section). Items 1 (empty-brick
conservative-min) and 2 (Aaltonen penumbra) shipped; the dominant remaining
artifact is the fine-field nearest-sampling stair-step.

Current baked state (confirmed against source): the fine atlas is `R16Sint`,
sampled nearest, with **no sampler bound**. Surface bricks are serialized as a
**flat, row-wrapped element stream** — a brick is *not* a contiguous 3D sub-cube
of the texture (the shader de-interleaves a linear element index `slot ·
voxels_per_brick + voxel_in_brick`). Hardware trilinear needs the 8 neighbor taps
of any sample to be true spatial neighbors, so this upgrade must **re-pack bricks
as contiguous 3D sub-cubes with a 1-voxel apron**, switch to a filterable float
format, and bind a linear sampler.

## Scope

### In scope

- Fine atlas GPU texture `R16Sint` → `R16Float` (filterable by default on every
  wgpu backend incl. Metal; same 2 bytes/voxel). A linear `Filtering` sampler
  bound to the SDF bind group.
- Bake each **surface** brick with a **1-voxel two-sided apron** → `(brick_size +
  2)³` stored voxels per brick. Apron voxels hold the true signed field at the
  neighbor positions they mirror (same nearest-triangle eval as interior voxels);
  edge-extend where no neighbor exists at the world boundary.
- Pack each surface brick as a **contiguous 3D sub-cube** at its tiled atlas
  position. The fine sampler reads it via brick-3D-position + apron offset +
  half-texel addressing, one `textureSampleLevel`.
- On-disk atlas stays **compact** (surface bricks only, each `(brick_size + 2)³`);
  the 3D tiling into the texture happens at upload.
- Bump `SDF_ATLAS_VERSION` (on-disk) and the bake `STAGE_VERSION`. Re-bake dev maps.

### Out of scope

- **Coarse/empty-brick field filtering.** Stays nearest; item 1's conservative-min
  already removed its banding. Keeps the coarse texture off the `float32-filterable`
  feature dependency (it is `R32Float`).
- **Finer voxel size** (item 4 — dropped; it caused past perf regressions).
- **Full-res tracing** and **SDF-driven indirect visibility** — both would undo the
  settled half-res + DDGI-indirect architecture.
- The Aaltonen penumbra (item 2) and the self-shadow surface bias — already shipped.

## Acceptance criteria

Automated:

- [ ] The fine SDF atlas is a filterable float texture bound with a linear
      (`Filtering`) sampler; the shadow trace samples it via `textureSampleLevel`
      (hardware trilinear), not `textureLoad`. Shader naga-validation passes.
- [ ] Surface bricks bake with a 1-voxel apron on all sides (stored
      `(brick_size + 2)³`). A bake test asserts an apron voxel equals the field
      value at the neighbor position it mirrors, within quant epsilon.
- [ ] Trilinear sampling is seamless across brick boundaries: a host test on a
      known continuous field shows a brick's edge apron column equals the adjacent
      brick's first interior column (no seam discontinuity).
- [ ] `SDF_ATLAS_VERSION` and the bake `STAGE_VERSION` are bumped; loading a
      pre-bump `.prl` is rejected with a clear version error.
- [ ] `cargo fmt` / `clippy` clean; full suite green.

Manual / visual (human, in-engine — not machine-verified):

- [ ] In `occlusion-test`, SDF Mode On: the world-locked voxel-grid (0.5 m)
      stair-step ripples on the shadow penumbra are gone/smoothed, with the cast
      shadow and base contact shadow intact.

Perf gate (human, measured — owner hardware: Windows i5 / GTX 1660 Super and/or
2020 MBP):

- [ ] Per-pass `sdf_shadow` time (`POSTRETRO_GPU_TIMING=1`) and fine-atlas memory
      (≈ `(brick_size + 2)³ / brick_size³` ≈ 1.95× at `brick_size = 8`) recorded.
      The filtered trace holds budget; if not, the parent slice's fail-floor stands.

## Tasks

### Task 1: Apron'd, brick-tiled bake + wire-format bump

Extend the baker (`sdf_bake.rs`) so each surface brick stores a 1-voxel apron:
sample the signed field over `[-1, brick_size]` on each axis (`(brick_size + 2)³`
samples, z-major), apron voxels via the same nearest-triangle eval as interior
voxels, edge-extending at the world boundary. Keep on-disk storage compact
(surface bricks only, each `(brick_size + 2)³` i16, back-to-back). Update the
level-format `SdfAtlasSection` for the new per-brick length (`atlas_len ==
(brick_size + 2)³ · surface_brick_count`) and bump `SDF_ATLAS_VERSION`; bump the
bake `STAGE_VERSION`. Tests: apron voxel equals the mirrored neighbor field value;
seam continuity (a brick's +x apron column equals the +x-neighbor brick's first
interior column).

### Task 2: R16Float upload + sampler + trilinear shader sampling

Switch the fine atlas texture to `R16Float` (`sdf_atlas.rs`), converting the
on-disk i16-quantized values (`step = voxel_size_m / 256`) to f16 at upload. Place
each surface brick as a contiguous `(brick_size + 2)³` 3D sub-cube at its tiled
atlas position (per-brick `write_texture`, or a dense staging buffer + one write);
texture dims become `atlas_bricks_per_axis · (brick_size + 2)`. Create a linear
`Filtering` sampler; update the SDF bind-group layout (fine binding →
`Float { filterable: true }`, add the sampler binding). Rewrite
`sample_fine_distance` (`sdf_shadow.wgsl`): map `slot` → 3D atlas-brick coordinate
(via `atlas_bricks_per_axis`) → base texel `· (brick_size + 2)`; add the apron
offset, intra-brick coordinate, and half-texel center; sample once via
`textureSampleLevel(sdf_atlas, sdf_sampler, uvw, 0.0)`. Drop the flat
element-index de-interleave and the no-apron half-texel clamp. Interior/empty
sentinel handling unchanged.

### Task 3: Re-bake + perf/visual gate

Recompile the dev maps (the cache invalidates on the version bumps), confirm the
SDF atlas bakes and the engine loads without validation errors. Hand the visual
check (voxel ripples gone) and the perf measurement (per-pass `sdf_shadow` time +
fine-atlas memory, owner hardware) to the owner. Record numbers; apply the parent
slice's fail-floor.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the on-disk packing + apron contract
the runtime loads.
**Phase 2 (sequential):** Task 2 — consumes Task 1's format/packing; the shader
addressing must match the baked layout exactly.
**Phase 3 (sequential):** Task 3 — measures the assembled path; gates the follow-on.

## Rough sketch

**Apron fill (bake).** Interior voxels keep their current z-major order at stored
indices `[1, brick_size]` per axis; apron occupies indices `0` and
`brick_size + 1`. Apron voxel world center lies in a neighbor brick's space — eval
the signed field there exactly as for interior voxels. At the world-AABB boundary
(no neighbor), edge-extend the nearest interior value.

**Packing + addressing.** On-disk each surface brick is a compact `(brick_size +
2)³` block. At upload, brick `slot` maps to a 3D atlas-brick coordinate
`(slot % apx, (slot / apx) % apy, slot / (apx·apy))` where `ap* =
atlas_bricks_per_axis`; the brick's texels start at that coordinate `· (brick_size
+ 2)` and fill a contiguous sub-cube. The shader inverts the same mapping:
`base = atlas_brick_coord · (brick_size + 2)`; a world point's intra-brick
fraction `frac ∈ [0, 1)` over the `brick_size` interior voxels maps to texel
coordinate `base + 1 (apron) + frac · brick_size + 0.5 (half-texel center)`;
normalize by the atlas dims and `textureSampleLevel`. The `+1` skips the apron;
the apron supplies the ±1 neighbors trilinear needs at interior edges, so sampling
is seamless across brick seams.

**Why filterable float, not manual trilinear.** `R16Sint` is non-filterable in
wgpu; manual trilinear would be 8 `textureLoad`s + lerp per step. `R16Float` is
filterable by default (no feature flag, all backends), same 2 bytes/voxel, ample
precision for small local distances — one hardware `textureSampleLevel` does the
trilinear. `R32Float` is rejected (needs the non-universal `float32-filterable`).

## Wire format

Modifies the existing `SdfAtlas` section (SectionId 33, `level-format`), not a new
section. Little-endian, unchanged. The `atlas: Vec<i16>` element type is unchanged
(i16 quantized, `step = voxel_size_m / 256`); only the **per-brick element count**
changes from `brick_size³` to `(brick_size + 2)³`, and the header `atlas_len`
field follows. The atlas stays compact (surface bricks only, back-to-back, z-major
within the apron'd brick). Bump `SDF_ATLAS_VERSION`; the deserializer rejects the
prior version (recompile-from-`.map` is the pre-release path — the build cache
already invalidates on the bake `STAGE_VERSION` bump). 3D tiling is a GPU-upload
concern, not an on-disk one.

## Open questions

- **f16 encoding at upload.** Confirm a vetted f32→f16 path (the `half` crate or an
  existing helper) is available in the render crate; add it if not. The on-disk
  data stays i16, so this is confined to the upload path.
- **Apron at the world boundary** — pinned to edge-extend (clamp nearest interior
  value); flag if a different bound reads better in-engine.
- **Coarse-field filtering** is deferred (out of scope). Revisit only if residual
  4 m banding survives item 1 in practice.
