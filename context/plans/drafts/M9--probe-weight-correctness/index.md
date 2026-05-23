# Probe Weight Correctness (no new baked data)

## Goal

Fix indirect-light artifacts from the baked SH irradiance volume in the world shader. Replace the hardware-trilinear SH fetch with a manual 8-corner blend that drops invalid corner probes and downweights backfacing ones, then renormalizes the survivors. Plumb the already-baked per-probe validity bit through to the GPU so an in-wall probe (ignore it) is distinguishable from a genuinely dark valid probe (respect its darkness). Record a residual smear/leak baseline on a leak-prone map before the depth atlas (spec #2) is built — that before/after delta justifies the atlas.

Milestone 9 spec #1. Fixes a latent darkening bug independent of the future DDGI work and a prerequisite it needs anyway: invalid probes are currently zero-packed and blended in via the hardware sampler, dragging near-wall surfaces toward black.

## Key decisions

| # | Decision | Rationale |
|---|---|---|
| Validity signal | Carry the already-baked `validity` bit in the unused band-0 alpha channel; exclude `validity == 0` corners. **Not** an all-zero-L0 heuristic. | Distinguishes "in a wall" from "genuinely dark." The all-zero test would smear brightness into spaces meant to fade to black — the opposite leak. No new baked data (validity is already baked in PRL section 20), no new section, no new VRAM (alpha already allocated in `Rgba16Float`). The packer already reads the non-gated level-format `ShProbe.validity` — today only to zero-init invalid probes — so the new alpha write sources from that same `probe.validity`, not the `#[cfg(feature="dev-tools")]` `ShVolume::validity` runtime mirror. Lean + correct = the project-vision answer. |
| Sampler topology | Replace hardware trilinear with manual per-corner texel loads. | Per-corner weights cannot be reweighted through a linear sampler. Cost: 8 corners × 9 bands = 72 texel loads vs. today's 9 samples (see open questions). The baseline loads all 9 bands per corner unconditionally (the flat-72 figure); validity-first early-out is the optimization target named in Open questions, not the baseline. |
| Code sharing | One shared WGSL SH helper (§8 string-concat pattern), consumed by forward, billboard, and fog. | Removes three copy-pasted samplers; one place to maintain the corrected blend. |
| Backface rejection scope | Forward only. Billboard and fog get validity exclusion + renormalization but no backface rejection. | Backface rejection needs a real surface normal. Billboard uses camera-forward (`N = V`); fog uses a fixed up-normal and is isotropic — neither is a surface normal. |

## Scope

### In scope

- Manual 8-corner SH blend in `forward.wgsl`: drop invalid (`validity == 0`) corners, downweight backfacing ones by `max(dot, 0)`, renormalize survivors, ambient-floor fallback when none survive.
- Plumb baked validity to the GPU: packer writes the validity bit into band-0 alpha; the SH compose pass propagates base→total band-0 alpha.
- Extract the corrected blend into one shared WGSL helper; adopt it in `forward.wgsl`, `billboard.wgsl`, and `fog_volume.wgsl` with per-path config.
- Measurement gate: manual before/after capture of residual through-wall bleed and near-wall darkening, plus a CPU frame-time delta, recorded where spec #2 can read it.

### Out of scope

- Probe depth/visibility atlas (spec #2) — no per-probe depth moments, no BVH ray-cast bake.
- Chebyshev / DDGI visibility-weighted interpolant (spec #3).
- Directional fog (spec #4).
- Any new or extended PRL section. Validity is already baked (section 20); this only carries it through to the GPU.
- GPU per-pass timing instrumentation — not yet set up; the gate uses CPU frame time.

## Acceptance criteria

- [ ] At the known through-wall bleed structure in `occlusion-test`, indirect light bleeding through the wall is visibly reduced or gone versus a captured baseline, viewed in the StaticSHOnly isolation mode.
- [ ] Near-wall surfaces no longer darken from in-wall (invalid) probes being averaged in.
- [ ] A genuinely dark-but-valid probe still contributes its low irradiance — its darkness is respected, not replaced by brighter neighbors. Validity exclusion keys on the baked validity bit, not on an all-zero coefficient test.
- [ ] When all eight corners are invalid or backfacing, the indirect term degrades to the ambient floor (matching the `has_sh_volume == 0` path) — no division by zero, no NaN, no black flash.
- [ ] In open areas where all eight corners are valid and front-facing, output is visually unchanged from baseline — no perceptible difference (manual `textureLoad` + fp32 trilinear weighting is not bit-identical to the hardware sampler on `Rgba16Float`, but the delta is sub-perceptual). The fix is a no-op where nothing is rejected.
- [ ] Rotating a test surface's normal in the forward path changes which corners contribute; billboard and fog render without regression and apply validity exclusion without backface rejection.
- [ ] Exactly one shared WGSL SH-sampling helper exists; no duplicated SH sampler remains in `forward.wgsl`, `billboard.wgsl`, or `fog_volume.wgsl` (checked at end of Phase 2 — forward adopts the helper in Phase 1, billboard and fog in Phase 2).
- [ ] Under the `dev-tools` feature, total band-0 alpha matches baked validity at a known invalid probe (alpha `0`) and a known valid probe (alpha `1`) — verified via `ShProbeReadback` or the validity marker overlay — so a compose-pass alpha-propagation regression (Task 1.2) is caught directly, not only through downstream visuals.
- [ ] A before/after screenshot pair at the known structure and a CPU frame-time delta are recorded in `context/plans/drafts/M9--probe-weight-correctness/measurements/`, annotated with map, camera pose, isolation mode, and commit, such that spec #2 can read the delta without re-deriving it.

## Tasks

### Task 1: Plumb baked validity to the GPU and build the shared corrected helper

Three coupled changes that ship together (the helper is inert until validity reaches the GPU):

1. **Packer** — for valid probes, write the validity bit into band-0 alpha (`1.0`) instead of `0.0`; invalid probes stay all-zero (alpha `0.0`) as today. The packer already branches on `probe.validity` (zero-init then skip), so alpha is exactly two-state — `0x0000` (invalid) or `0x3c00` (valid) — and the shader's `< 0.5` test never reads an intermediate value. The packer's per-band loop writes alpha for all 9 bands; guard the alpha write with `band == 0` so the validity signal lives in band-0 alpha only and bands 1–8 keep alpha `0`.
2. **Compose pass** — read base band-0 alpha and write it into total band-0 alpha. Today the compose shader reads bands as `.rgb` (dropping alpha) and stores total bands with alpha `0.0`. Consumers sample the *total* textures, and the compose pass runs unconditionally, so validity must travel base→total or it never reaches the shader. Source alpha exclusively from base: read base band-0 `.a` and write it once into total band-0 `.a`. Delta contributions stay `.rgb`-only (as today), so accumulation never pollutes the alpha channel. This needs a new `textureLoad(sh_base_band0, p, 0).a` read — the existing `.rgb` band load stays as-is for reconstruction (it can't simply gain an `.a` without breaking the `array<vec3<f32>, 9>` typing). Change only the band-0 `textureStore` to carry that alpha; bands 1–8 stores keep `.a = 0.0`.
3. **Shared helper** — a manual 8-corner blend taking `(gi, gfrac, shading_normal, geo_normal, reject_backface)`. Per corner: `textureLoad` all 9 total bands at the corner's integer grid index (unconditionally — the flat-72 baseline; early-out is deferred, see Open questions); compute the trilinear weight from `gfrac`. Clamp each corner index (`gi + corner_offset`) to `[0, grid_dimensions - 1]` before `textureLoad`, matching the existing clamp-to-edge sampling — otherwise an out-of-range load at the grid boundary returns `0`, which the validity test would misread as an invalid corner. The corner's blend weight is `w = trilinear * validity * bf`: `validity` is `1.0` when band-0 alpha `>= 0.5` else `0.0`; `bf` is `1.0` when `reject_backface` is false, otherwise `max(dot((vec3(corner_offset) - gfrac) * sh_grid.cell_size, geo_normal), 0.0)`. The backface direction is un-normalized — its magnitude divides out under renormalization, so a near-coincident corner can't blow up, and folding `bf` into `w` (rather than a hard `dot <= 0` cutoff) fades a corner smoothly to zero as it crosses behind the shading plane, avoiding the pop a hard cutoff causes when a high-weight corner flips. Accumulate `Σ w·irradiance(shading_normal)` and `Σ w`; renormalize; fall back to the ambient floor when `Σ w` is below epsilon. The helper bundles the `sh_irradiance` reconstruction it calls per corner and declares no buffers (§8); consumers bind group 3 before it is appended. The helper references the global identifiers `sh_band0`…`sh_band8` (`texture_3d<f32>`) and `sh_grid` (`ShGridInfo`); each consumer must declare these at the group-3 bindings the helper expects before it is appended, and must delete its own local `sh_irradiance`/`sample_sh_indirect*` first (the helper now owns those symbols — appending alongside a local copy is a duplicate-definition error). For forward, that deletion happens here in Task 1, not deferred to Task 2. Adopt it in `forward.wgsl` with `reject_backface = true`, passing the geometric mesh normal `mesh_n` as `geo_normal` and `N_bump` as `shading_normal` (the backface test keys on real geometry, not the perturbed shading normal; SH reconstruction still uses `N_bump`), keeping the existing normal-offset wrapper.

Plumbing: forward already has `gi`, `gfrac`, the world-space `N_bump`, and the geometric mesh normal `mesh_n` (passed as `geo_normal`). The backface direction reduces to `(vec3(corner_offset) - gfrac) * cell_size`, so no separate fragment world position is needed; `cell_size` comes from the group-3 `sh_grid` uniform (`ShGridInfo`, alongside `grid_origin` / `grid_dimensions`). Group-3 texture visibility already includes the fragment stage. The manual path uses `textureLoad` (integer coords), not the existing linear `sh_sampler`. Note `gfrac` here is derived from the existing normal-offset sample position (the wrapper forward already applies), so the backface term is evaluated at that offset point rather than the raw fragment position — a sub-cell, sub-perceptual shift; the helper deliberately reuses this `gfrac` instead of computing a separate un-offset fraction, keeping the path lean.

### Task 2: Adopt the shared helper in billboard and fog

Replace the copy-pasted SH samplers in `billboard.wgsl` and `fog_volume.wgsl` with the shared helper, passing `reject_backface = false` for both (so `geo_normal` is unused — pass each path's shading normal for both normal arguments). Validity exclusion and renormalization apply everywhere (zero-packed corners are wrong everywhere); backface rejection does not (neither path has a real surface normal — see Key decisions). Preserve fog's no-normal-offset behavior. Delete the now-dead duplicate `sh_irradiance` / `sample_sh_indirect*` definitions in both shaders.

Plumbing: both shaders already bind group 3 and derive `gi`/`gfrac` inline. The helper owns corner indexing and edge clamping, so pass raw (unclamped) `gi`/`gfrac` and remove fog's existing `gf` clamp to avoid double-clamping; the remaining edit is swapping their local sampler for the appended helper call.

### Task 3: Measurement gate

Compile `content/dev/maps/occlusion-test.map` to `.prl`. Position the camera at the known structure where shadow/indirect bleeds through a wall (fixed pose). Use the StaticSHOnly isolation mode (pure SH indirect). Capture the before image from the base commit prior to Task 1 (checkout or `git stash` the working tree before starting Task 1) and an after image (post-fix), recording both commit hashes in `baseline.md`; observe manually whether through-wall bleed and near-wall darkening are gone. Record the CPU frame-time delta from the existing frametime stats (before vs. after) as the perf proxy — note it is wall-clock and reflects the added 72-fetch cost only when GPU-bound. Store the screenshots and a short notes file (`baseline.md`: map, camera pose, isolation mode, commit, qualitative residual, frame-time delta) in `context/plans/drafts/M9--probe-weight-correctness/measurements/` (the path moves with the plan if it is promoted out of `drafts/`), so spec #2 can read the baseline.

Plumbing: isolation modes are already wired (`LightingIsolation::StaticSHOnly = 6`); CPU frame-time stats already exist (`FrametimeStats`, 120-sample ring). No new diagnostic plumbing is required.

## Sequencing

**Phase 1 (sequential):** Task 1 — validity plumbing + shared helper; everything else depends on it.
**Phase 2 (sequential):** Task 2 — adopts the helper into billboard/fog; depends on Task 1's final helper shape.
**Phase 3 (sequential):** Task 3 — measures with the fix in place; the before/after delta needs Task 1 visible.

## Rough sketch

- Helper + forward adoption: `sample_sh_indirect_fast` in `crates/postretro/src/shaders/forward.wgsl` (~L367) becomes a call into the shared blend; `sh_irradiance` (~L344) reconstruction relocates into the shared helper with its body unchanged (it is identical across forward, billboard, and fog), so all three drop their local copies — matching the "no duplicated SH sampler remains" AC.
- Validity plumbing: packer `pack_probes_to_band_slices` in `crates/postretro/src/render/sh_volume.rs` (~L533, the `band_buf[off + 3]` write, guarded to `band == 0`); compose shader `crates/postretro/src/shaders/sh_compose.wgsl` (add a `textureLoad(sh_base_band0, p, 0).a` read near the existing `.rgb` band load at ~L216, carry it into only the band-0 `sh_total_band0` `textureStore` at ~L295). `f32_to_f16_bits(1.0) == 0x3c00` for the alpha write. Consumers read this alpha via the `sh_band0`…`sh_band8` group-3 bindings (wired to the total textures); `sh_total_band0` exists only as the compose-pass storage output.
- Shared helper file: a new WGSL helper string appended at pipeline creation for the forward, billboard, and fog pipelines (§8 pattern, alongside the existing animated-light curve helper).
- Diagnostics for the gate: `LightingIsolation` in `crates/postretro/src/render/mod.rs` (~L173); `FrametimeStats` in `crates/postretro/src/frame_timing.rs`; `MarkerMode::Validity` / `ShProbeReadback` in `sh_diagnostics.rs` (both existing; for spot-checking validity — note their CPU mirrors `ShVolume::validity` / `probe_l0` are `#[cfg(feature="dev-tools")]`, so this check runs only under `dev-tools`; the alpha upload path itself is not gated).

## Open questions

- **72-fetch cost.** The manual blend is 72 texel loads vs. today's 9 hardware samples. Confirm via the Task 3 frame-time delta that it is acceptable on target hardware; if not, the corner loop is the obvious optimization target (early-out, fewer bands for the DC-dominant case). Recorded, not blocking.
- **Camera pose stability.** Task 3 assumes the `occlusion-test` bleed structure is observable from a single fixed pose; confirm during the gate (fallback: `campaign-test`).
