# Octahedral Irradiance Atlas

## Goal

Replace the baked L2 SH irradiance encoding with a DDGI-style octahedral irradiance probe atlas. Cuts per-pixel cost of the indirect-lighting hot path on pre-RTX late-GTX hardware (Pascal/Turing) without losing directional quality. Hardware-filtered fetches and elimination of per-pixel SH reconstruction land together, while per-probe Chebyshev visibility (the "directional light data") stays on by default.

## Background

The current forward/billboard/fog sampler does a manual 8-corner trilinear blend over nine `Rgba16Float` 3D band textures with `textureLoad` (~72–80 fetches/fragment), reconstructing L2 irradiance per corner (8×). On the SH encoding, hardware filtering and per-probe Chebyshev visibility are mutually exclusive: per-probe weighting needs per-probe access, which blocks a single hardware-filtered tap.

Octahedral encoding breaks that bind. It keeps an 8-probe loop (per-probe weights survive) but uses hardware **bilinear** filtering *within* each probe's octahedral tile for directional reconstruction, and removes SH reconstruction ALU entirely.

Builds on shipped Milestone 9 (probe-weight correctness, depth/visibility atlas, Chebyshev interpolant, directional fog — all in `context/plans/done/M9--*`). This is a perf re-encoding on top of M9, not part of it.

## Scope

### In scope

- **Irradiance re-encoding.** Bake per-probe octahedral irradiance tiles (interior + 1-texel border) into a 2D atlas, replacing the nine SH band textures (base + total).
- **Animated delta re-encoding.** Convert the per-light delta SH volumes to octahedral delta tiles so the compose pass can blend them per-texel.
- **Compose-pass rework.** Per-texel octahedral `total = base + Σ(delta × color × brightness)`, replacing the per-band SH compose.
- **Runtime sampler rework.** New 8-probe loop in the shared SH helper: per-probe octahedral bilinear fetch + analytic trilinear / backface / Chebyshev weights. Forward, billboard, and fog (dual-direction) paths preserved.
- **Aggressive option.** A runtime branch that skips the per-probe Chebyshev visibility term. Default **off** (Chebyshev on). Exposed as a debug-panel toggle and a startup CLI flag.
- **Measurement.** A GPU timing pair covering the SH compose and SH sampling cost.

### Out of scope (non-goals)

- **Directional octahedral depth/visibility.** Visibility stays the shipped **isotropic** per-probe `(E[d], E[d²])` Chebyshev model, reused unchanged. Per-direction octahedral depth is a future upgrade.
- **Probe streaming / bricking.** Deferred per the M9 memory-budget decision; full volume stays resident.
- **Changing probe placement, grid resolution, or ray count.** Bake sampling (`RAYS_PER_PROBE`) is unchanged; only the output encoding changes.
- **Keeping the SH path as a runtime alternative.** This is a one-way content-format migration; the engine loads one encoding. No SH↔octahedral runtime switch.
- **Specular/glossy probe reuse.** Diffuse irradiance only.

## Acceptance criteria

- [ ] A map compiled by `prl-build` stores octahedral irradiance tiles (with borders) instead of SH band coefficients; the engine renders indirect light from them.
- [ ] Side-by-side a reference scene at the default (Chebyshev-on) setting is visually equivalent to the pre-migration SH build — no new light leaks through walls, no loss of directional bounce. Capture before/after in a `measurements/` subfolder.
- [ ] Per-fragment SH texture-fetch count drops materially versus the SH path (record the before/after fetch count and GPU pass time).
- [ ] A new GPU timing pair reports SH compose and SH sampling time under `POSTRETRO_GPU_TIMING=1`.
- [ ] Animated lights still modulate indirect light: a pulsing/animated light visibly changes nearby indirect lighting through the compose pass.
- [ ] Fog volumes retain directional scatter response (the dual-direction read driven by `scatter_bias`/Henyey-Greenstein), unchanged by the aggressive toggle.
- [ ] The aggressive option, toggled at runtime in the debug panel, measurably reduces forward-pass SH cost and (only) softens near-wall occlusion; the CLI flag selects it at startup for headless/benchmark runs.

## Tasks

### Task 1: Octahedral atlas format + base bake
Define the on-disk octahedral atlas layout (see Wire format) and emit it from `sh_bake.rs`. For each probe, resample the baked per-direction irradiance into an octahedral tile, write border/gutter texels by copying across the octahedral wrap, and pack tiles into the 2D atlas in a fixed probe order. Keep the existing `ShProbe` validity bit and the isotropic depth moments — only the SH coefficient payload is replaced. Update the `SectionId` registry and `build_pipeline.md`.

### Task 2: Octahedral delta bake
Re-encode the per-light delta volumes (currently `DeltaShVolumes`, SH deltas, CSR-sparse) as octahedral delta tiles in the same tile geometry as Task 1, so a delta tile adds per-texel to a base tile. Preserve the CSR affinity-cell structure (offsets, light indices, 4×4×4 affinity cells); only the per-light payload encoding changes. The delta is **indirect-only** (bounce) per the post-`sdf-per-light-shadows` contract — the animated light's direct term lives in `lm_anim`, so the octahedral delta must encode bounce only (bake via the indirect path, not direct+indirect).

### Task 3: Runtime resources + loader
Replace the nine base/total band 3D textures in the SH GPU resource struct with octahedral atlas textures (base sampled, total storage-writeable). Add a filtering sampler and update the bind-group layout to a filterable float sample type. Leave the depth-moment texture, grid-info uniform, animation descriptors, and scripted-light buffers unchanged. Wire the loader to upload the new atlas section.

### Task 4: Compose-pass rework
Rework the `compose_main` compute pass to operate on octahedral tiles: read base atlas + the in-range delta tiles for each affinity cell, evaluate the existing animation curves (brightness/color via Catmull-Rom), accumulate per-texel, and write the total atlas. Preserve validity. Reconcile workgroup dispatch with the atlas (per-tile-texel) rather than per-probe-band. Deltas are added at **full weight** — the `delta_scale` dev knob was retired in `sdf-per-light-shadows` (the `GridDims` uniform's old `delta_scale` field is now padding), so there is no per-frame delta reweight to carry over.

### Task 5: Runtime sampler rework
Rewrite the shared sampler in `sh_sample.wgsl` as an 8-probe loop: per probe, map (grid index, octahedral direction) to an atlas tile UV and do one hardware-bilinear fetch; weight by trilinear × validity × backface × (Chebyshev unless aggressive); accumulate and renormalize. Keep the three entry shapes — forward (depth-aware, backface-reject), billboard (depth-aware), fog (dual-direction, no depth, no backface). The fog dual read fetches two octahedral directions per probe from one iteration.

### Task 6: Aggressive option (toggle + CLI)
Add a uniform-gated branch that skips the Chebyshev fetch and term in Task 5. Default off (conservative). Plumb a debug-panel control to flip it at runtime, and a startup CLI flag (e.g. `--sh-aggressive`) that sets the initial state for headless/benchmark runs. The flag seeds the same uniform the panel writes.

### Task 7: SH GPU timing
Add a timing pair (e.g. `TIMING_PAIR_SH_COMPOSE`) and bump `TIMING_PAIR_COUNT`; bracket the SH compose dispatch and the forward SH sampling so `POSTRETRO_GPU_TIMING=1` reports them.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the atlas format/contract every other task consumes.
**Phase 2 (concurrent):** Task 2, Task 3 — delta bake and runtime resources both build on the Task 1 format, independent of each other.
**Phase 3 (concurrent):** Task 4, Task 5 — compose and sampler both consume the format (Task 1) and resources (Task 3); they touch different shaders.
**Phase 4 (concurrent):** Task 6, Task 7 — toggle and timing layer onto the working sampler (Task 5).

## Rough sketch

- **Atlas geometry.** Per-probe tile of N×N texels including a 1-texel border (DDGI reference: 6×6 incl. border → 4×4 interior). Tiles packed into a 2D atlas; probe `(x,y,z)` maps to a tile by a fixed flattening of the grid. Hardware bilinear filters *within* a tile (directional); trilinear *across* probes is the manual 8-probe loop.
- **Channel format.** Low-frequency HDR irradiance — `Rgba16Float` (filterable by default in wgpu, no `float32-filterable` feature) is the safe choice; `Rg11b10Float`/`Rgb9e5Ufloat` are a memory follow-up only if filterability checks out per adapter.
- **Reference.** NVIDIA RTXGI-DDGI `Irradiance.hlsl` — octahedral encode/decode, border copy, the trilinear/backface/Chebyshev weight chain, 8-probe blend. Port the weight math near line-for-line to WGSL.
- **Grounded touch-points (verify at implementation):** baker `crates/level-compiler/src/sh_bake.rs` (`sh_basis_l2`, `apply_cosine_lobe_rgb`, `bake_probe_rgb_with_moments`); format `crates/level-format/src/{lib.rs (SectionId), sh_volume.rs (ShProbe, ShVolumeSection), delta_sh_volumes.rs}`; runtime `crates/postretro/src/render/{sh_volume.rs (SH_BAND_COUNT, bind group 3, ShVolumeResources), sh_compose.rs}`; shaders `crates/postretro/src/shaders/{sh_sample.wgsl, sh_compose.wgsl, forward.wgsl, fog_volume.wgsl}`; pass order + timing `crates/postretro/src/render/mod.rs`.
- **Compose linearity.** Octahedral irradiance adds per-texel, and a delta tile scaled by a runtime color/brightness scalar stays valid per-texel — so the existing `base + Σ(delta × color × brightness)` structure maps directly onto tiles.

## Wire format

New octahedral irradiance section (and re-encoded delta section). Mirror the existing `ShVolume` (id 20) and `DeltaShVolumes` (id 27) section conventions: same endianness, integer signedness, length-prefix width, entry-count placement, and empty-list/sentinel encoding as the current SH sections. Decide whether to replace the `ShVolume` payload in place or add sibling `SectionId`s during Task 1; if new IDs are added, register them in `crates/level-format/src/lib.rs` and `build_pipeline.md`.

Pin for the new layout:
- Tile dimension N and border width (border = 1).
- Per-tile channel format and texel order within a tile (octahedral convention must match the runtime decode exactly, or seams appear).
- Atlas dimensions and the probe `(x,y,z) → tile origin` flattening order.
- Grid origin, cell size, grid dimensions, and validity bit retain their current meaning and section placement.
- Isotropic depth moments `(E[d], E[d²])` are unchanged — same data, same section.

## Open questions

- **New section vs. in-place replacement of `ShVolume`.** Sibling `SectionId`s keep loaders simple to branch; in-place replacement avoids a dead enum value. Decide in Task 1.
- **Tile resolution.** 4×4 interior (≈L2 directionality) vs 6×6/8×8 (sharper, more memory). Bake a representative map and pick by quality/VRAM; default to the DDGI 6×6-incl-border reference unless measurement argues otherwise.
- **Octahedral seam convention.** The bake encode and the WGSL decode must share one octahedral mapping; pick it once in Task 1 and reference it from Task 5.
- **Aggressive fallback path.** If the debug toggle can't share one pipeline (e.g. a bind-group difference forces variants), fall back to the CLI flag selecting a pipeline at startup. Expected unnecessary — skipping Chebyshev is a uniform branch over identical bindings.
- **Memory delta.** Confirm octahedral atlas (base + total + delta) footprint against the nine-band SH baseline on a large map; feed the M9 VRAM-budget readout.
