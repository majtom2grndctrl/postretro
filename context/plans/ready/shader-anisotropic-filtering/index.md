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

Wire the helper into the diffuse and specular slot reads in the lit fragment entry point. All three slots share `base_sampler` (group 1 binding 1); the sampler arg to the helper is the same for all three calls.

For the normal-map slot, use a sibling helper `sample_aniso_normal(texture, sampler, uv, ddx, ddy) -> vec3<f32>` that decodes each tap to tangent-space `[-1, 1]` (`tap.rgb * 2 - 1`), averages the decoded vectors, and normalises the sum before returning. Re-encoding to `[0, 1]` happens at the call site if needed. Averaging encoded normals and decoding afterward would bias toward `(0.5, 0.5, 1.0)` and is wrong.

### Task 2: Compile-time toggle and tap-count knob

Add `const ENABLE_MANUAL_ANISO: bool = true;` and `const ANISO_TAP_COUNT: u32 = 4;` at the top of `forward.wgsl`. When disabled, the call sites bypass `sample_aniso` / `sample_aniso_normal` entirely and call `textureSample` directly — same call site as pre-aniso code. This keeps AC #2 trivially true: the disabled build is byte-identical to the unmodified shader. Document both in a one-line comment so the next person to touch this file knows the perf vs. quality knob exists.

### Task 3: Perf measurement and decision

Run the campaign-test map on a GTX 1060 (or equivalent — `POSTRETRO_GPU_TIMING=1` reports per-pass GPU time). Record lit-pass cost for: `ENABLE_MANUAL_ANISO = false`, 2-tap, and 4-tap. Ship 4-tap if its overhead is acceptable on target hardware; drop to 2-tap if not. Record the numbers in the PR description.

## Sequencing

Prerequisite: baked-texture-mips plan must be merged first. Tasks 1, 2, and 3 are one PR. Task 3 measures with `ANISO_TAP_COUNT = 4` and `= 2`, then sets the const to the winning value before merge. The const is a compile-time edit; no separate PR needed.

## Rough sketch

Major-axis direction in UV space comes from the longer of `(ddx, ddy)`. Tap span is `major_len - minor_len` — the anisotropic *extension* only, not the full footprint. The isotropic core is already covered by the per-tap mip selection; spreading taps across the full major length would double-sample it. Taps are centered on the original UV and distributed evenly across the extension: for 4 taps at offsets `(-3/8, -1/8, +1/8, +3/8) * (major_len - minor_len)` along the major direction; for 2 taps at `±(major_len - minor_len) / 4`.

Per-tap derivatives: WGSL `dpdx`/`dpdy` are screen-space basis, not major/minor-axis aligned, so each tap must construct synthetic derivatives. Let `minor_dir` be the unit vector in UV space along the minor axis (perpendicular to the major axis direction), and let `minor_len` be the shorter of `length(ddx)`, `length(ddy)`. Build:

    ddx' = minor_dir * minor_len
    ddy' = perp(minor_dir) * minor_len

Pass `ddx'`, `ddy'` to `textureSampleGrad` for the tap. This makes the hardware pick the mip sized for an isotropic minor-axis footprint — sharper than the isotropic-major mip a single sample would pick.

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

