# Shader-Side Anisotropic Filtering

## Goal

Suppress grazing-angle shimmer on world surfaces without giving up the chunky nearest-filter look in-plane. Hardware anisotropic filtering forces `min/mag = Linear`, which blurs the pixel grid at all distances — the aesthetic we're trying to keep. This plan adds per-pixel anisotropic sampling in the fragment shader, gated by screen-space derivative aspect ratio: near-isotropic footprints take a single tap (current behaviour), grazing footprints take N taps along the major axis. Result: Quake 1 silhouette and texel density up close, no LOD-ring shimmer at distance.

Depends on the baked-texture-mips plan landing first. The aniso shader reads from the mip pyramid; quality of the taps is bounded by the quality of the mips.

## Scope

### In scope

- Manual anisotropic sampling in the forward lit pass shader (`crates/postretro/src/shaders/forward.wgsl`).
- Per-pixel branch on `aniso_ratio = max_axis_len / min_axis_len` from `dpdx`/`dpdy` on the texture UVs. Below threshold (e.g. 2.0): single `textureSample` (current). Above: N taps of `textureSampleGrad` distributed along the major-axis direction, each tap supplied explicit shrunken derivatives so the hardware picks a sharper mip per tap.
- Tap count fixed at compile time. Start with 4; tune downward to 2 if perf demands.
- Applies to diffuse, specular, and normal slots — they share UVs and the same footprint, and shimmer on any of them is visible.
- Compile-time toggle (WGSL `const ENABLE_MANUAL_ANISO: bool`) for A/B testing and disabling on the perf-floor path.

### Out of scope

- Hardware anisotropic sampler bucket per material. Rejected: requires linear in-plane filter, kills the aesthetic.
- Elliptical Weighted Average (EWA). 8–16 taps; overkill for GTX 10-era target.
- Temporal accumulation / TAA. Aesthetic conflict — temporal smearing is the artifact we're avoiding.
- RIP-maps (separate H/V mip chains). 2× texture memory and still need a linear in-plane sampler.
- Per-material aniso override in FGD or texture metadata. Derivative-based runtime detection is more reliable than authoring intent.
- Sprites, billboards, skybox, post-process passes. Aniso problem only applies to surfaces sampled at grazing angles.
- Shadow map sampling. Different problem; not addressed here.

## Acceptance criteria

- [ ] Long floor texture (a 1024² tiled diffuse on a ground brush) viewed at grazing angle shows no shimmer in motion under `cargo run -p postretro -- content/dev/maps/campaign-test.prl`. Compare against `ENABLE_MANUAL_ANISO = false` build of the same scene.
- [ ] Up-close view of the same texture (head-on, near-isotropic footprint) is pixel-identical between manual-aniso and disabled builds. Confirms the low-aspect branch is the no-op fast path.
- [ ] No visible banding or seams at the threshold crossover. View the floor at a continuous range of angles and confirm the transition between 1-tap and N-tap regions is invisible.
- [ ] Measure lit-pass cost on a GTX 1060-class GPU at 1080p for `ENABLE_MANUAL_ANISO = false`, 2-tap, and 4-tap. Ship 4-tap if its overhead is acceptable on the target hardware; drop to 2-tap if not. Record the numbers in the PR description.
- [ ] Normal-map shading reads correctly through the aniso path — no specular shimmer on glossy surfaces at distance beyond what a single tap produces.

## Tasks

### Task 1: Manual aniso function in `forward.wgsl`

Add a `sample_aniso(texture, sampler, uv, ddx, ddy) -> vec4<f32>` helper. Computes `len_x = length(ddx)`, `len_y = length(ddy)`, picks the major axis, computes `aniso_ratio = max / min`. If `ratio < THRESHOLD` (const, start at 2.0), return a single `textureSampleGrad(texture, sampler, uv, ddx, ddy)`. Otherwise distribute N tap positions along the major axis in UV space, each tap reading with the *minor*-axis derivative pair (so each tap picks the mip appropriate for the short footprint). Average the taps.

Wire the helper into all three slot reads (diffuse, specular, normal) in the lit fragment entry point. When the helper is called on the normal-map slot, renormalise the averaged result (`normalize(sample.rgb * 2 - 1)` followed by re-encoding) before returning. Diffuse and specular returns are not renormalised.

### Task 2: Compile-time toggle and tap-count knob

Add `const ENABLE_MANUAL_ANISO: bool = true;` and `const ANISO_TAP_COUNT: u32 = 4;` at the top of `forward.wgsl`. When disabled, the helper compiles to a passthrough `textureSample`. Document both in a one-line comment so the next person to touch this file knows the perf vs. quality knob exists. These compile-time consts are for development gates and bisection; if tuning across playtest sessions becomes important, promote them to runtime uniforms driven by CVars.

### Task 3: Perf measurement and decision

Run the campaign-test map on a GTX 1060 (or equivalent — `POSTRETRO_GPU_TIMING=1` reports per-pass GPU time). Record lit-pass cost for: `ENABLE_MANUAL_ANISO = false`, 2-tap, and 4-tap. Ship 4-tap if its overhead is acceptable on target hardware; drop to 2-tap if not. Record the numbers in the PR description.

## Sequencing

Tasks 1 and 2 are one PR. Task 3 is a measurement gate before merge; informs whether to ship 2-tap or 4-tap.

## Rough sketch

Major-axis direction in UV space comes from the longer of `(ddx, ddy)`. Tap positions are offsets along that direction, centered on the original UV, spanning ±0.5 of the major-axis derivative length. For 4 taps: offsets at -3/8, -1/8, +1/8, +3/8 of the major-axis length. For 2 taps: ±1/4.

Per-tap derivative: pass the minor-axis derivative pair to `textureSampleGrad`, with the major-axis derivative shrunk to `minor_len`. This makes each tap pick a mip sized for an isotropic minor-axis footprint — sharper than the isotropic-major mip a single sample would pick.

Threshold choice: 2.0 is a defensive default. Below it, hardware mip selection already gives acceptable results. Above 2.0, the isotropic mip starts visibly over-blurring the minor axis.

## Boundary inventory

No engine-side type or section changes. Pure shader work. Sampler config unchanged.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Aniso enable flag | n/a (WGSL const) | n/a | n/a | n/a | n/a |
| Tap count | n/a (WGSL const) | n/a | n/a | n/a | n/a |

## Open questions

- **Threshold value.** 2.0 is a starting guess. The right value depends on the texture content — high-frequency tiled patterns (grates, brick) shimmer at lower aspect ratios than low-frequency textures (concrete). May need to tune empirically against the campaign-test scene.
- **Crossover continuity.** Hard branch at the threshold could produce a visible seam where 1-tap and N-tap regions meet. If acceptance criterion #3 fails, swap the branch for a smooth lerp: 1-tap below threshold, N-tap above, blend over a small window. Costs the worst-case taps in the blend window but kills the seam.
- **Other shader paths.** Forward lit pass is the obvious target. Depth prepass uses the same UVs but doesn't read colour — skip. Fog composite reads scene colour, not surface textures — skip. Confirm no other pass samples world textures with non-trivial derivatives before merging.
- **Placeholder textures.** Placeholder textures (1×1 checkerboard, neutral normal, black specular) have effectively zero UV derivatives, so the aspect-ratio branch routes them through the 1-tap fast path. No special-casing needed in the helper.
