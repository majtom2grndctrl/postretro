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

### AC status

- [x] Effect scales with `scatter_bias` — scatter=100 shows notably warmer, more directional fog than scatter=1 (near-isotropic baseline).
- [x] Fog responds to baked SH direction — color shift is consistent with forward-scatter pulling in warm baked indirect light.
- [ ] `scatter_bias = 0` parity with pre-change flat haze — not captured (construction guarantee covered by CPU test; visual parity AC remains open).
- [ ] Overlapping fog volumes with different `scatter_bias` blend smoothly — not captured.
- [ ] `ambient_scatter = 0` + `min_brightness = 0` shows no SH ambient while dynamic lights still scatter — not captured.
- [ ] Unset `ambient_scatter` matches previous full-ambient look — not captured.
