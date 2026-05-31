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
- **Probe Occlusion toggle.** The per-probe Chebyshev visibility term, exposed as a runtime toggle. Default **on** (occlusion applied). Turning it off skips the Chebyshev fetch and term for speed, softening near-wall occlusion. Exposed as a debug-panel toggle and a startup env var (`POSTRETRO_SH_FAST=1` disables it for benchmark runs).
- **Measurement.** A GPU timing pair covering the SH compose and SH sampling cost.

### Out of scope (non-goals)

- **Directional octahedral depth/visibility.** Visibility stays the shipped **isotropic** per-probe `(E[d], E[d²])` Chebyshev model, reused unchanged. Per-direction octahedral depth is a future upgrade.
- **Probe streaming / bricking.** Deferred per the M9 memory-budget decision; full volume stays resident.
- **Changing probe placement, grid resolution, or ray count.** Bake sampling (`RAYS_PER_PROBE`) is unchanged; only the output encoding changes.
- **Keeping the SH path as a runtime alternative.** This is a one-way content-format migration; the engine loads one encoding. No SH↔octahedral runtime switch.
- **Specular/glossy probe reuse.** Diffuse irradiance only.

## Acceptance criteria

- [ ] A map compiled by `prl-build` stores octahedral irradiance tiles (with borders) in place of the SH band coefficients (validity bit and isotropic depth moments retained per the committed section-layout choice); the engine renders indirect light from them.
- [ ] Side-by-side a reference scene at the default (Chebyshev-on) setting is visually equivalent to the pre-migration SH build — no new light leaks through walls, no loss of directional bounce. Capture before/after in `context/plans/drafts/octahedral-irradiance-atlas/measurements/`. The 'before' is captured from a pre-migration commit/build (no runtime SH↔octahedral toggle exists post-migration).
- [ ] Per-fragment SH texture-fetch count drops materially versus the SH path (record the before/after fetch count and GPU pass time).
- [ ] A new GPU timing pair reports SH compose time under `POSTRETRO_GPU_TIMING=1`; SH sampling cost is reported as forward-pass timing deltas (octahedral vs SH build, and Probe Occlusion on vs off), since sampling lives inside the forward fragment shader and has no isolatable timestamp boundary.
- [ ] Animated lights still modulate indirect light: a pulsing/animated light visibly changes nearby indirect lighting through the compose pass.
- [ ] Fog volumes retain directional scatter response (the dual-direction read driven by `scatter_bias`/Henyey-Greenstein), unchanged by the Probe Occlusion toggle.
- [ ] Disabling Probe Occlusion at runtime in the debug panel measurably reduces forward-pass SH cost (observed as a forward-pass timing delta with the toggle on vs. off, since SH sampling is not separately bracketable within the forward pass) and (only) softens near-wall occlusion; `POSTRETRO_SH_FAST=1` disables it at startup for headless/benchmark runs.
- [ ] Loading a pre-migration `.prl` (old section version) fails cleanly with a version-mismatch error rather than mis-rendering; a re-bake of identical inputs stays byte-identical (determinism invariant preserved).
- [ ] Runtime VRAM footprint of the octahedral atlas set (base + total + delta) is recorded against the nine-band SH baseline and fed to the M9 VRAM-budget readout; the resident volume still fits the memory budget.
- [ ] Animated-light indirect contribution magnitude matches the pre-migration indirect-only delta — the octahedral delta encodes bounce only, with no direct-term double-count against `lm_anim` — verified in the side-by-side reference at peak animated brightness.

## Tasks

### Task 1: Octahedral atlas format + base bake
Define the on-disk octahedral atlas layout (see Wire format) and emit it from `sh_bake.rs`. For each probe, resample the baked per-direction irradiance into an octahedral tile (see `crates/level-format/src/octahedral.rs` for the existing encode/decode — decide reuse vs. new tile-UV-aware mapping per the seam-convention open question), write border/gutter texels by copying across the octahedral wrap, and pack tiles into the 2D atlas in a fixed probe order. Build against a concrete default of N = 6 including border (4×4 interior, the DDGI reference); the tile-resolution measurement open question is a non-blocking follow-up that may revise it. Keep the existing `ShProbe` validity bit and the isotropic depth moments — only the SH coefficient payload is replaced. Update the `SectionId` registry and `build_pipeline.md`. Bump `sh_volume::SH_VOLUME_VERSION` (currently 5) and the `sh_bake::STAGE_VERSION` (currently 2) — the loader hard-rejects mismatched section versions and the build cache keys on the stage version, so stale `.prl` files and cached bakes would otherwise be silently mis-served. Preserve the byte-identical determinism invariant.

### Task 2: Octahedral delta bake
Re-encode the per-light delta volumes (currently `DeltaShVolumes`, SH deltas, CSR-sparse) as octahedral delta tiles in the same tile geometry as Task 1, so a delta tile adds per-texel to a base tile. Preserve the CSR affinity-cell structure (offsets, light indices, 4×4×4 affinity cells); only the per-light payload encoding changes. The delta is **indirect-only** (bounce) per the post-`sdf-per-light-shadows` contract — the animated light's direct term lives in `lm_anim`, so the octahedral delta must encode bounce only (bake via the indirect path, not direct+indirect). Bump `delta_sh_volumes::DELTA_SH_VOLUMES_VERSION` (currently 2) so the loader hard-rejects stale sections. Unlike `sh_bake`, the delta bake (`delta_sh_bake.rs`) has no `STAGE_VERSION` and is not cache-keyed (it is invoked directly in `main.rs`), so there is no build-cache key to bump; introduce a `STAGE_VERSION` + `CacheKey` if delta bakes should be cached.

### Task 3: Runtime resources + loader
Replace the nine base/total band 3D textures in the SH GPU resource struct with octahedral atlas textures (base sampled, total storage-writeable). Add a filtering sampler and a filterable float sample type. Collapsing nine band textures to one or two atlas textures reshuffles group 3, so the exact binding indices are finalized at implementation; the depth-moment texture, animation descriptors, and scripted-light buffers keep their meaning. The `ShGridInfo` sampler-side uniform (`forward.wgsl`, currently `@group(3) @binding(10)`) gains the tile-geometry fields the sampler needs — tile dimension N, border, atlas dimensions, and the probe `(x,y,z) → tile-origin` flattening — so it is no longer byte-identical to the SH-band version. Wire the loader to upload the new atlas section. Note: `total` needs both a `STORAGE_BINDING` view (compose write) and a filterable sampled view (forward read) — two views on one texture. `Rgba16Float` storage-write is already proven: the current compose pass writes nine `texture_storage_3d<rgba16float, write>` textures (`sh_compose.rs`/`sh_compose.wgsl`), and this migration only narrows that to 2D, so no fallback is required.

### Task 4: Compose-pass rework
Rework the `compose_main` compute pass to operate on octahedral tiles: read base atlas + the in-range delta tiles for each affinity cell, evaluate the existing animation curves (brightness/color via Catmull-Rom), accumulate per-texel, and write the total atlas. Preserve validity. Reconcile workgroup dispatch with the atlas (per-tile-texel) rather than per-probe-band; extend the compose-side `GridDims` uniform to carry the tile geometry (N, border, atlas dims, flattening) the per-texel mapping needs. Deltas are added at **full weight** — the `delta_scale` dev knob was retired in `sdf-per-light-shadows` (the compose-side `GridDims` uniform's 4th word, `_pad: f32` (which once held the `delta_scale` knob), is now padding), so there is no per-frame delta reweight to carry over.

### Task 5: Runtime sampler rework
Rewrite the shared sampler in `sh_sample.wgsl` as an 8-probe loop: per probe, map (grid index, octahedral direction) to an atlas tile UV and do one hardware-bilinear fetch; weight by trilinear × validity × backface × (Chebyshev when Probe Occlusion is enabled); accumulate and renormalize. Keep the three entry shapes — forward (depth-aware, backface-reject, Chebyshev), billboard (depth-aware, Chebyshev, no backface), fog (dual-direction, no depth, no backface, no Chebyshev). The fog path does two bilinear taps per probe within one loop iteration (one tap per direction; unlike the legacy SH path, the two directions cannot share a single load reconstructed twice).

### Task 6: Probe Occlusion toggle (panel + env var)
Add a uniform-gated branch (e.g. a `probe_occlusion` flag) that, when disabled, skips the Chebyshev fetch and term in Task 5. Default **on** (occlusion applied — the conservative, no-leak default). Plumb a debug-panel control to flip it at runtime, and a startup env var (`POSTRETRO_SH_FAST=1` disables Probe Occlusion, matching the existing `POSTRETRO_GPU_TIMING` convention — the engine has no `--`-flag parser; `main.rs` skips `--`-prefixed args) for headless/benchmark runs. The env var seeds the same uniform the panel writes. The env var only sets the initial value; the panel may override it at runtime (a seed, not a lock). The `probe_occlusion` branch is scoped to entry shapes that use Chebyshev (forward and billboard, both depth-aware) — the fog path never reads the uniform (no Chebyshev), so the toggle is inherently a no-op there.

### Task 7: SH GPU timing
Add a timing pair (e.g. `TIMING_PAIR_SH_COMPOSE`) and bump `TIMING_PAIR_COUNT`; bracket the SH compose dispatch so `POSTRETRO_GPU_TIMING=1` reports it. Sampling is not separately bracketed (it lives inside the forward fragment shader) — its cost is read as the forward-pass timing delta (see below). Deliverable: capture and record the before/after per-fragment fetch count, the absolute SH compose time, and the forward-pass deltas; write results to the `measurements/` subfolder (see AC bullet 3). Adding the pair also requires an entry in the parallel `pass_labels` vec (`render/mod.rs`), not just the constant + count bump. The 'SH sampling' figure is the whole forward-pass timing delta (TIMING_PAIR_FORWARD) before vs. after — a sub-region of one render pass cannot be timestamp-bracketed; only the compose dispatch gets its own pair. Also update `rendering_pipeline.md` §11's measured-passes list to include the new SH-compose pair.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the atlas format/contract every other task consumes.
**Phase 2 (concurrent):** Task 2, Task 3 — delta bake and runtime resources both build on the Task 1 format, independent of each other.
**Phase 3 (concurrent):** Task 4, Task 5 — compose and sampler both consume the format (Task 1) and resources (Task 3); they touch different shaders.
**Phase 4 (concurrent):** Task 6, Task 7 — toggle and timing layer onto the working sampler (Task 5).

## Rough sketch

- **Atlas geometry.** Per-probe tile of N×N texels including a 1-texel border (DDGI reference: 6×6 incl. border → 4×4 interior). Tiles packed into a 2D atlas; probe `(x,y,z)` maps to a tile by a fixed flattening of the grid. Hardware bilinear filters *within* a tile (directional); trilinear *across* probes is the manual 8-probe loop.
- **Channel format.** Low-frequency HDR irradiance — `Rgba16Float` (filterable by default in wgpu, no `float32-filterable` feature) is the safe choice; `Rg11b10Float`/`Rgb9e5Ufloat` are a memory follow-up only if filterability checks out per adapter.
- **Reference.** NVIDIA RTXGI-DDGI `Irradiance.hlsl` — octahedral encode/decode, border copy, the trilinear/backface/Chebyshev weight chain, 8-probe blend. Port the weight math near line-for-line to WGSL.
- **Grounded touch-points (verify at implementation):** baker `crates/level-compiler/src/{sh_bake.rs (sh_basis_l2, apply_cosine_lobe_rgb, bake_probe_rgb_with_moments), delta_sh_bake.rs}`; format `crates/level-format/src/{lib.rs (SectionId), sh_volume.rs (ShProbe, ShVolumeSection), delta_sh_volumes.rs}`; runtime `crates/postretro/src/render/{sh_volume.rs (SH_BAND_COUNT → replaced by tile-geometry constants: tile dimension / interior texel count, defined alongside the atlas format; bind group 3, ShVolumeResources), sh_compose.rs}`; shaders `crates/postretro/src/shaders/{sh_sample.wgsl, sh_compose.wgsl, forward.wgsl, fog_volume.wgsl}`; pass order + timing `crates/postretro/src/render/mod.rs`.
- **Compose linearity.** Octahedral irradiance adds per-texel, and a delta tile scaled by a runtime color/brightness scalar stays valid per-texel — so the existing `base + Σ(delta × color × brightness)` structure maps directly onto tiles.

## Wire format

New octahedral irradiance section (and re-encoded delta section). Mirror the existing `ShVolume` (id 20) and `DeltaShVolumes` (id 27) section conventions: same endianness, integer signedness, length-prefix width, entry-count placement, and empty-list/sentinel encoding as the current SH sections. Decide whether to replace the `ShVolume` payload in place or add sibling `SectionId`s during Task 1; if new IDs are added, register them in `crates/level-format/src/lib.rs` and `build_pipeline.md` (the highest assigned id is `SdfAtlas = 33`, so new sibling ids start at 34).

Pin for the new layout:
- Tile dimension N (stored in the section header so resolution can change via re-bake, not a format break) and border width (border = 1). N is the full tile dimension including border (interior = N − 2·border). Default N to the DDGI 6 (incl. border; 4×4 interior) reference; the Open Questions tuning only changes the default, not the format.
- Per-tile texel order within a tile (octahedral convention must match the runtime decode exactly, or seams appear). Channel format is fixed at `Rgba16Float` — not a header field (the `Rg11b10Float`/`Rgb9e5Ufloat` memory follow-up would be a format revision).
- Atlas dimensions and the probe `(x,y,z) → tile origin` flattening order.
- Grid origin, cell size, grid dimensions, and validity bit retain their current meaning and section placement.
- Isotropic depth moments `(E[d], E[d²])` are unchanged — same data, same section.
- Only base tiles (and delta tiles) are serialized; the `total` atlas is runtime-only (compose-pass output), never written to disk.

## Open questions

- **New section vs. in-place replacement of `ShVolume`.** Sibling `SectionId`s keep loaders simple to branch; in-place replacement avoids a dead enum value. Default: prefer new sibling `SectionId`s. Commit this choice at the start of Task 1, before Phase 2 begins, so Task 2 and Task 3 do not diverge.
- **Tile resolution.** 4×4 interior (6×6 incl. border, ≈L2 directionality) vs 6×6/8×8 interior (sharper, more memory). Bake a representative map and pick by quality/VRAM; default to the DDGI 6×6-incl-border reference unless measurement argues otherwise.
- **Octahedral seam convention.** An existing `crates/level-format/src/octahedral.rs` defines `encode`/`decode` for normals (round-trips `u16x2`; not tile-UV/border-aware). Decision: reuse `octahedral.rs` extended with border logic, or define a new tile-UV/border-aware mapping alongside the atlas format. Whichever is chosen, the Rust encode (Task 1) and WGSL decode (Task 5) must share it bit-for-bit. Commit this choice at the start of Task 1 (like the SectionId question, before Phase 2). There is no codegen guaranteeing the Rust encoder and WGSL decoder agree, so mirror the mapping constants in both `octahedral.rs` and `sh_sample.wgsl` and pin equivalence with a reference-vector test.
- **Probe Occlusion fallback path.** If the debug toggle can't share one pipeline (e.g. a bind-group difference forces variants), fall back to `POSTRETRO_SH_FAST` selecting a pipeline at startup. Expected unnecessary — skipping Chebyshev is a uniform branch over identical bindings.
- **Memory delta.** Confirm octahedral atlas (base + total + delta) footprint against the nine-band SH baseline on a large map; feed the M9 VRAM-budget readout.

**Name retention.** Post-migration, "SH" names are kept as-is to avoid a half-renamed tree: `sh_compose.rs`, `sh_sample.wgsl`, `ShVolumeResources`, and the new `TIMING_PAIR_SH_COMPOSE` all retain their "SH" prefix even though the encoding is no longer spherical harmonics.
