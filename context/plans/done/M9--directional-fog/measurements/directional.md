# Directional Fog Measurement

Date: 2026-05-23
Verified state: `HEAD` = `e0c4a86` with uncommitted M9 directional-fog changes.

## Automated Verification

- `cargo fmt --check`: pass
- `cargo check`: pass
- `cargo test -p postretro fog_pass`: pass
  - Confirms fog WGSL parse + naga validation.
  - Confirms CPU HG reference tests: lobe peak, symmetric falloff, `g = 0` uniform, finite endpoints.
  - Confirms directional SH blend is finite, componentwise bounded, and `g = 0` returns the isotropic read.
- `cargo test -p postretro-level-format fog_volumes`: pass
  - Confirms `FogVolumeRecord` round-trips `anisotropy` and `ambient_scatter`.
- `cargo test -p postretro-level-compiler fog_resolvers`: pass
  - Confirms resolver defaults and clamp/translation behavior for `scatter_bias` and `ambient_scatter`.
- `cargo clippy -- -D warnings`: pass
- `cargo test`: fail
  - 375 passed.
  - 4 existing watcher hot-reload tests failed:
    - `scripting::watcher::tests::luau_edit_triggers_reload`
    - `scripting::watcher::tests::luau_rename_triggers_reload`
    - `scripting::watcher::tests::start_script_luau_edit_at_mod_root_triggers_mod_init_reload`
    - `scripting::watcher::tests::ts_edit_triggers_reload_via_scripts_build`

The full-suite failures are outside the directional-fog path. Re-run before landing if watcher behavior changes.

## Manual Visual A/B

Status: partially captured 2026-05-23.

Map: `content/dev/maps/campaign-test.prl`
Screenshots: `~/Pictures/Screenshots/scatter-100.png`, `~/Pictures/Screenshots/scatter-1.png`

| Shot | File | `scatter_bias` | Approx. position | Notes |
|---|---|---|---|---|
| High scatter | `scatter-100.png` | 100 (g ≈ 0.9) | (-32, 2, -10) | Warmer, amber fog; forward SH ambient pulled toward baked warm indirect |
| Near-isotropic | `scatter-1.png` | 1 (g ≈ 0.009) | (-31, 2, -11) | Cooler, flatter fog; dynamic purple light reads more clearly with reduced SH ambient |

Poses captured in separate engine sessions — not pixel-identical. The color shift (warmer/more directional at scatter=100) is clearly visible and in the correct direction relative to the baked SH in the scene.

## SH cache interpolation fix (concentric-shell banding)

The original directional implementation held both cached SH reads
(`cached_sh_iso`, `cached_sh_dir`) piecewise-constant across each
`sh_coverage_dist` stride (~4m default). The world-up iso read varies slowly,
so the hold was invisible pre-M9; the view-derived dir read varies sharply with
position, so holding it constant produced a visible staircase. Because the
refresh distances sit at constant `t` along each ray, those steps project to
screen as concentric shells / nested ellipses when the camera is inside the
fog volume — loudest at high `scatter_bias` (e.g. 90 → g ≈ 0.81, blend ~81% on
the dir read).

Fix (`crates/postretro/src/shaders/fog_volume.wgsl`, `cs_main`): replaced the
piecewise-constant hold with linear interpolation between two look-ahead
anchors per read (`sh_iso_lo/hi`, `sh_dir_lo/hi`, anchored at `sh_t_anchor`).
Per step the cached value is `mix(lo, hi, saturate((t - sh_t_anchor) /
sh_coverage_dist))`. When the march crosses `sh_t_anchor + sh_coverage_dist`,
a single `if` advances the anchor, reuses `hi` as the new `lo` (no extra read),
and samples one fresh `hi` a stride ahead. Amortized cost is unchanged (one
resample per stride; one extra read at each sub-interval init). The schedule
stays keyed on `t` alone, preserving the frame-stability guarantee.

Automated validation after the fix: `cargo fmt --check` pass, `cargo test -p
postretro fog_pass` pass (WGSL parse + naga validation + HG/blend CPU
references), `cargo clippy -- -D warnings` pass.

### g = 0 baseline change (call-out)

This is strictly a change to how the iso/dir reads are reconstructed, not to the
`mix(iso, dir, saturate(g))` blend — at `g = 0` the blend still returns the iso
term exactly. But the `g = 0` path now uses the *interpolated* iso read instead
of the piecewise-constant iso. That is a small, strictly-better change (smoother
iso along the ray) well within visual tolerance, but it IS a change to the
`g = 0` rendered output. Visual `g = 0` parity vs. the pre-change flat-haze
build remains PENDING manual A/B.

### Pending manual A/B for this fix

Visual confirmation of (a) banding/concentric-shell removal at high
`scatter_bias` and (b) `g = 0` parity is PENDING — cannot be verified from a
terminal. Suggested repro: camera inside a fog volume authored with
`scatter_bias 90` on `content/dev/maps/campaign-test.prl`; capture before/after
the fix and confirm the nested-ellipse banding is gone with no new artifacts.

### AC status

- [x] Effect scales with `scatter_bias` — scatter=100 shows notably warmer, more directional fog than scatter=1 (near-isotropic baseline).
- [x] Fog responds to baked SH direction — color shift is consistent with forward-scatter pulling in warm baked indirect light.
- [ ] `scatter_bias = 0` parity with pre-change flat haze — not captured (construction guarantee covered by CPU test; visual parity AC remains open).
- [ ] Overlapping fog volumes with different `scatter_bias` blend smoothly — not captured.
- [ ] `ambient_scatter = 0` + `min_brightness = 0` shows no SH ambient while dynamic lights still scatter — not captured.
- [ ] Unset `ambient_scatter` matches previous full-ambient look — not captured.

## Look-ahead clamp fix (dark bowl)

After the interpolation fix, the look-ahead anchor (`hi`, one `sh_coverage_dist`
ahead of `lo`) could land past the first opaque surface when the camera faced a
nearby wall. A sample in solid geometry has every SH corner in-wall, so the
helper returns 0; interpolating `lo → hi` then dragged the fog ambient toward
black over the last stride before the surface, reading as a dark ellipsoidal
"bowl" centered on the view axis.

Fix (`fog_volume.wgsl`, `cs_main`): clamp both look-ahead positions to
`ray.max_t` (`min(sh_t_anchor + sh_coverage_dist, ray.max_t)`) at the
sub-interval init and at each anchor advance. The look-ahead stays in valid
empty space in front of the surface; the interpolant no longer pulls toward a
zero read.

Visual A/B 2026-05-23 (camera inside a `scatter_bias 90` volume on
`campaign-test.prl`, facing a wall): the dark bowl is gone. Faint residual
concentric banding remained — addressed by the stride/dual-fetch fix below.

## SH cache stride + dual-fetch (residual concentric banding)

The interpolation fix replaced the piecewise-constant hold with linear
interpolation between anchors `sh_coverage_dist` (~4m at the old
`SH_COVERAGE_CELLS = 4.0`) apart. Linear interpolation is C0-continuous (no
value jumps) but not C1: the slope changes at each anchor. Because anchors sit
at constant `t` along every ray, those slope kinks project to screen as faint
concentric rings — and lateral inhibition (Mach banding) exaggerates them. The
view-derived directional read varies at the SH grid's spatial frequency (one
cell), so a 4-cell stride under-samples it; at high `scatter_bias` the dir read
dominates the blend, so the rings were still visible after the interpolation
fix.

Fix (`fog_volume.wgsl` + `sh_sample.wgsl`):

1. **Dual fetch.** The 72 `textureLoad`s in an SH read are direction-independent
   (only the final `sh_irradiance` reconstruction uses the direction). Added
   `sample_sh_indirect_corners_two_without_depth` (in `sh_sample.wgsl`): one
   8-corner fetch, two reconstructions (iso + dir). Each returned component is
   bit-identical to the prior single-direction call, at half the texture
   bandwidth. The fog wrapper `sample_sh_fog` was replaced by
   `sample_sh_fog_dual`; the four per-anchor SH calls collapsed to two dual
   calls.
2. **Stride at one cell.** Lowered `SH_COVERAGE_CELLS` from 4.0 to 1.0 so the
   cache resamples once per SH cell — Nyquist for the trilinear field. The cache
   is then no coarser than the grid's own trilinear seams (already the accepted
   SH baseline on surfaces), removing the concentric banding. The dual fetch
   pays for most of the finer sampling: net SH texture bandwidth in the fog pass
   is ~2× the pre-fix cost (4× more anchors × 0.5× per-anchor loads).

Automated validation: `cargo fmt --check` pass, `cargo clippy -p postretro`
clean, `cargo test -p postretro fog_pass` 11/11 (WGSL parse + naga validation of
the new dual helper + HG/blend CPU references).

### Pending

- Manual visual A/B: confirm the residual concentric banding is gone at
  `scatter_bias 90` with no new artifacts and the dark bowl stays gone.
- GPU timing: the stride change ~2× the fog pass's SH bandwidth. Re-check the
  `<2ms/pass` target with `POSTRETRO_GPU_TIMING=1`. If it regresses, raising
  `SH_COVERAGE_CELLS` to 2.0 is the cost-neutral fallback (2× denser rings than
  the original, half the reads of the 1.0 setting).

## Composite dither (output 8-bit quantization — the actual ring source)

Re-diagnosed from scratch after the stride/dual-fetch fix above failed to
remove the rings. Tightening the SH cache to one cell per grid cell killed the
*float-domain* sampling theory: the rings survived at 1-cell stride, so they
were never a sampling-frequency artifact.

Data-path trace of the scatter value:

1. Accumulated in the raymarch in `f32` (`accum`, `cs_main`).
2. Written to the low-res scatter target — `SCATTER_FORMAT = Rgba16Float`
   (`fog_pass.rs:21`). Half-float: no visible quantization here.
3. Nearest-upscaled and **additively composited into the swapchain surface**
   (`fog_composite.wgsl::fs_main`; blend `src One + dst One`,
   `LoadOp::Load`). The surface format is chosen as the first **sRGB** caps
   format (`render/mod.rs:827` `find(|f| f.is_srgb())`) → an **8-bit
   `*UnormSrgb`** target. There is no HDR intermediate: forward, billboard, and
   the fog composite all render straight to this 8-bit surface
   (`render/mod.rs:2715`, `view` is the swapchain texture view).

So the *only* place a smooth fog gradient is quantized is the additive write to
the 8-bit sRGB surface. Because every prior fix operated on the float scatter
value (steps 1–2), none of them touched the 8-bit quantization in step 3 — which
is exactly why the rings were immortal.

This explains the full signature:

- **Rings** — quantizing a smooth gradient to 256 levels/channel produces
  discrete iso-value contours; lateral inhibition (Mach banding) sharpens them.
- **Radial / centered on the view axis** — the directional SH read evaluates
  toward `-ray.direction`, which sweeps a cone about the view center, so its
  iso-value contours *are* rings about that center.
- **Worst at high `scatter_bias`** — at high `g` the blend
  `mix(iso, dir, saturate(g))` is dominated by that broad, smooth directional
  ramp, so the quantization steps spread across more pixels and read as wide
  rings rather than tight noise.

Fix (`fog_composite.wgsl::fs_main`): dither at the point of quantization. Add a
sub-LSB triangular-PDF (TPDF) per-pixel offset — two Interleaved Gradient Noise
samples (Jimenez) differenced — sized to one 8-bit LSB. The dither is applied
in **sRGB-encoded space** (encode → add dither → decode; the hardware
re-encodes on store) so the amplitude stays ~1 LSB across the full brightness
range rather than under/over-dithering through the nonlinear transfer. This is
the textbook cure for output-quantization banding; it breaks the hard steps into
imperceptible static-grain noise. Stable per-pixel (no temporal term) so it
reads as fixed film grain, matching the pixelated-block fog aesthetic, with no
shimmer.

Cost: a handful of ALU ops per composite fragment (two hashes, an sRGB
encode/decode pair). The composite is a single fullscreen-triangle blit at
surface resolution; the dither adds no texture fetches and no extra passes.
Effectively free against the `<2ms/pass` budget. The raymarch compute pass —
where the SH-bandwidth cost lives — is untouched by this change.

Automated validation after the fix: `cargo fmt --check` pass,
`cargo clippy -p postretro -- -D warnings` clean,
`cargo test -p postretro fog_pass` 11/11 (incl. `fog_composite_wgsl_parses`,
which naga-parses the edited composite shader).

### Pending manual A/B for this fix

Cannot be verified from a terminal. Repro: camera inside the `scatter_bias 90`
volume on `content/dev/maps/campaign-test.prl`, facing into the fog. Before
(prior build) vs. after (this change): confirm the concentric rings are gone and
replaced by, at most, very fine imperceptible grain — no new visible noise,
shimmer, or color shift, and the dark-bowl regression stays fixed. Also glance at
a `scatter_bias 1` (near-isotropic) volume to confirm no regression on the flat
haze path. GPU timing is expected unchanged; re-check with
`POSTRETRO_GPU_TIMING=1 RUST_LOG=info cargo run -p postretro -- content/dev/maps/campaign-test.prl`.
