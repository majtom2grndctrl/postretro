# fluid-depth-fog-waves — research

Read at 6c31e7d36. Findings that inform the brief without deciding it.

## Prototype
`poc-fog-wave.patch` in this folder is the reverted fluid-surface-motion experiment the owner approved on 2026-10-13. A reference, not a base to apply.
- Group 5 holds one `texture_depth_2d`; the pass sets `depth_ops: None`. The bind group is rebuilt every frame; the brief rebuilds it with the scene targets instead.
- `fluid_underwater_path` solves the view ray's clip-space depth for the sampled scene depth. The ray is linear in clip space, so no inverse matrix is needed.
- Fog: `exp(-3·path)` with constant linear albedo `(0.04, 0.055, 0.048)` times the surface `lighting`, gated on `dot(world_normal, V) > 0`. Composite: `total = 1 − (1 − α)·T`; `rgb = (rgb·α + fog·(1 − α)·(1 − T)) / total`.
- Wave: Quake turb at a tenth of its speed — amplitude 0.125 tile, frequency 8 rad per tile, speed 0.1 rad/s, `u` driven by the cell's `v` and vice versa. Evaluated once per texel cell and rounded to whole texels, but through `sin`, which the no-trig test rejects.

## Wave defaults
Dev water on e24's east wing is `fluids/water_64` (64×64) at texture scale 1. The compiler maps `u = axis·p / scale + offset` in texels, so scale 1 is one texel per unit regardless of texture size. Fragment UV is in tile units and `uv × dims` is texels, so a texel offset becomes a UV offset by dividing by `dims`, with no per-face data.

| Prototype term | Tile units | Texels (64 px) |
|---|---|---|
| Amplitude (peak offset) | 0.125 | 8 → `waveAmplitude` 8 |
| Wavelength 2π/8 | 0.785 | 50.3 (6.3 × A, below the 8 × A floor) |
| Period 2π/0.1 | — | 62.8 s → dev `wavePeriod` 63 |

At amplitude 8 the default range is `[64, 80]`, so dev water's crests run longer than the prototype's 50. The floor 48 governs only below amplitude 6; at amplitude 16 the range collapses to `[128, 128]`.

`water_128` keeps texel size and doubles the tile, so the prototype's tile-relative wave would double in world size there; the texel form does not. The scale-0.25 fine pool has four texels per unit, so the same texel wave is a quarter the world size there — §7.3a's grain rule.

## Coherence and travel
- The warp is cross-coupled: cell (i, j) samples (i + F(j), j + G(i)). In continuous form its Jacobian is 1 − F′G′, so it folds wherever the two slopes' product reaches 1. At L = 6A a sine-like shape peaks at 2π/6 ≈ 1.05 texels per cell, a product of about 1.10: the continuous warp folds.
- The shortest crest sets the peak slope, so coherence uses the range minimum. At min = 8A a sine-like arc peaks at 2π/8 ≈ 0.79, a product of about 0.62 and a Jacobian of at least 0.38. The margin depends on the arc: a triangle's peak slope is 4A/L = 0.5; a parabola arc's is 8A/L = 1.0, exactly the fold boundary with no margin. The twin's arc keeps its peak slope below 1 texel per cell at L = 8A.
- Rounding to whole texels adds at most one: `|round(a) − round(b)| ≤ floor(|a − b|) + 1`. A peak slope below 1 therefore gives rounded neighbours at most 1 texel apart; the CPU twin derives its bound from its own arc.
- Rounding alone lets diagonal cells share a texel at any nonzero slope, as in the approved prototype: where both rounded offsets step down by one between neighbours, cells (i, j) and (i + 1, j + 1) sample one texel and one texel goes unsampled. The 8× minimum rules out the continuous fold, not this.
- Crests travel at mean length ÷ period texels per second: 72 / 63 ≈ 1.1 for dev water's `[64, 80]`, 128 / 6 ≈ 21.3 at the limits.

## GPU layout
- `fluid_depth_stencil_state` tests `Less` and never writes. The "Fluid Surface Pass" attaches depth Load/Store. The "Textured Pass" is the `depth_ops: None` precedent while sampling depth. Later passes are unaffected: billboards load depth with write off, the fog composite has no depth attachment, the viewmodel clears depth.
- Bind groups: 0 camera, 1 material, 2 lighting, 3 SH, 4 per-volume params by dynamic offset. Group 5 is free; `max_bind_groups` is 8. `FLUID_BIND_GROUP_COUNT` and the debug assert in `renderer_init_resources.rs` count them.
- `create_depth_texture` always adds `TEXTURE_BINDING`; `Renderer::rebuild_scene_targets` rebuilds it. Rebind precedents: `FogPass::resize`/`build_group6`, `SdfShadowPass::resize`, `SpotShadowPool::rebuild_bind_group`.
- Budgets: `fluid_pipeline_fragment_storage_request_matches_bgl_definitions` pins storage `[0,0,5,3,0]` (8/8). `fluid_pipeline_vertex_storage_and_sampled_textures_stay_within_limits` pins sampled `[0,4,0,3,0]` (7 of `FORWARD_SAMPLED_TEXTURE_BUDGET` 16). Both arrays have one entry per group, so group 5 extends them.

## Params row
- WGSL `FluidVolumeParams { slots: array<vec4<f32>, 4096> }` is a uniform with a dynamic offset; x is opacity, yzw reserved. Rust `FluidSurfaceSlots.params: Vec<[f32;4]>` writes unwritten rows as `[-1,0,0,0]`.
- `FLUID_PARAMS_STRIDE` 16 × `FLUID_SLOTS_PER_WINDOW` 4096 = 64 KiB, the WebGPU default uniform binding size and D3D's constant-buffer ceiling. Stride 32 would mean 2048 slots per window.
- Floats needed: opacity 1, fog distance 1, fog color 3, wave period 1, amplitude 1, length min and max 2 = 9. SSR's reflectivity makes 10, so three `vec4` leave two spare (brief §Decisions). The eye's frame weight is per frame, and per-face proximity comes from the camera and the fragment, so neither takes a row float.
- At stride 48 a 64 KiB window holds 1365 slots (65 520 B), not a multiple of the 256-byte dynamic-offset alignment; pin P1 fixes the expected addressing.

## Motion
- `uniforms.time` is a plain f32. `fluid_high_half` and `fluid_exact_fract_product` split the product so `fluid_drift_scroll` stays exact at 24 h (fluid-surface-motion R9). A raw `sin(time·k)` loses that.
- `fluid_warp_uv` composes `fluid_cell_step`, `fluid_shimmer_offset`, `fluid_ring_offset`, `fluid_faded_flat` and `fluid_displayed_offset`. Shimmer and rings fade per cell between 0.75 and 1.5 texels per pixel; drift does not fade. Shimmer fades without a seam because it is ±1 texel; a multi-texel wave has no such guarantee.
- `fluid_surface_wgsl_warp_has_no_trig_and_keys_cells_on_the_fixed_grid` enforces the ban textually, not for FXC: `billboard.wgsl` and `light_eval.wgsl` call `sin`/`cos`. A per-cell rounded wave keeps texel edges straight even through `sin`, as the prototype did, so the binding reasons are the test and f32 precision over a long run, not the look.
- `displayed_offset` (shimmer + rings) is what the 3 Hz and tenth-of-a-grid guards in `motion_tests.rs` measure; `warped_texel_uv` adds drift beside it.
- Parity is textual: `fluid_motion_shader_constants_match_the_cpu_reference`, `fluid_motion_wgsl_bodies_match_the_cpu_reference` and `fluid_drift_scroll_wgsl_matches_the_cpu_mirror` pin WGSL against `render-cpu` `fluid_surface/motion.rs`; behaviour tests live in `motion_tests.rs`.

## Underwater and fog volumes
- `apply_fluid_tint` (`screen_effects.wgsl`) is a flat `mix` toward `viewTint` rgb by its strength, clamped by the photosensitivity limiter. Not distance-based.
- `viewTint` rgb is an unlit target the resolve mixes toward after tonemap. The prototype's murk was a pre-tonemap albedo `(0.04, 0.055, 0.048)` times surface lighting. Dev water's `viewTint` rgb `(0.1, 0.3, 0.4)` is 2.5–8× that albedo, so a `fogColor` in `viewTint`'s space needs a scale to reach it.
- The tint's blend weight is `FluidEye`'s ramp (`fluid_presentation/eye.rs`): 0 dry, 1 fully presented, moving at 1 per `FLUID_PRESENTATION_RAMP_SECONDS` (0.25 s) toward the trigger's state.
  - The trigger turns on once the eye is `EYE_FLUID_MARGIN` (0.08 m) below the surface and off once it is that far out. A same-fluid split is one fluid.
  - The ramp stays 0 while the trigger is off. A dry eye less than the band below the plane sees no outside face below it and no tint: the view under the plane is clear and untinted until the trigger turns on.
  - Containment is separate: `fluid_at_eye` reports any eye inside a fluid brush, from the plane down, whatever the trigger or ramp.
- `build_pipeline.md` §Custom FGD (`fluid_volume`): two `fluid_volume` entities draw a surface between them, each side's outward normal pointing into the other. Draws sort far to near per volume, so two fogged faces on one pixel stack their composites.
- Fog volumes raymarch at the map's `fog_pixel_scale` (default 4, quarter resolution), and `fog_composite.wgsl` blends the result over the scene after the fluid pass (§7.5). Each volume is a `fog_volume` brush a mapper places.

## `defineFluid` plumbing
- `FluidDescriptor` (foundation `data_descriptors/types/fluid.rs`) has no `Default`. Literal sites span foundation, combat-model, the netcode predict-reconcile harnesses, postretro (presentation, sound events, digest, startup tests) and scripting-core. `FluidDescriptor::validate` holds range checks.
- `FLUID_KEYS` and `fluid_from_json` (scripting-core `data_descriptors/fluids.rs`); only `EntryPath::optional_string` exists.
- Typedefs register through `register_type("FluidDescriptor")` in sim `scripting/primitives/manifest.rs`, with a `name?` suffix for optional fields. `cargo run -p postretro-sim --bin gen-script-types` regenerates `sdk/types/postretro.d.ts` and `.d.luau`; typedef snapshot fixtures follow.
- SDK prose: `defineFluid` in `sdk/lib/data_script.{ts,luau}`. Docs: `docs/scripting-reference.md` §Fluids and swimming, `context/lib/scripting.md` §2. Parser tests: `scripting-core/src/data_descriptors/tests/fluids.rs`.
- Dev content: `content/dev/start-script.ts` and `content/dev/scripts/fluids-swim.{ts,luau}` both declare `water` and `sludge`; `fluids_swim_surface_tests.rs` pins them, and its `mutate` helper needs each mutated string to occur once.
- Invalid fields: `FluidRegistry::from_declarations` drops that one fluid with a warning naming the field.

## Co-op
`mod_compatibility_digest_from_registry` skips `registry.fluids` by name. `FluidMovementTuning` carries name and viscosity only; presentation resolves through `FluidMovementTable::local_presentation`. New presentation fields stay out of the digest unless one feeds `resolve_fluid_surface_drift`.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | A level whose surface volumes fill more than one params window draws the last slot of the first window and the first slot of the second in one frame | One dirty range crossing the window boundary uploads, then each window binds at its dynamic offset | Every window's dynamic offset is a multiple of 256; each slot's row is read from the byte it was uploaded at; neither slot reads another volume's row or fails dynamic-offset validation |
| P2 | Hot reload mid-level changes water's `fogDistance` 1.0 → 3.0, `surfaceOpacity` unchanged | Reload commits, then the next collect → `set_draws` | That frame's dirty range covers water's slots with the new extinction; sludge's slots stay clean; no frame draws one slot with a mix of old and new values |
| P3 | `waveAmplitude` 8 declared, `wavePeriod` omitted | Resolve → pack row → shader evaluates the wave per cell | The row's amplitude is 0, so every cell's wave offset is 0 at every time; never a frozen displacement pattern |
| P4 | Window resized in the frontend with no level loaded, then a pool level installs; or a resize commits in the frame a level installs; or a level with no fluid surfaces runs | `commit_extents` → `rebuild_scene_targets` → `install_fluid_surfaces`, either order within the frame | Group 5 exists from full init whether or not a level has surfaces, tracks the current depth view after every rebuild, and level clear or install never drops or rebuilds it; the first fluid frame after any of these binds the current depth |
| P5 | `waveAmplitude` out of range, `waveLength` omitted, `wavePeriod` omitted or set | Parser resolves the default length, then validation checks fields | The amplitude range is checked before the length range and the cross-field rule; wave ranges apply whether or not a period is set; a defaulted length never appears in a warning |
| P6 | Hot reload mid-level changes `wavePeriod` 63 → 20, or amplitude or length | Reload commits; next frame's row upload; shader evaluates phase from script time | The pattern may jump once on the reload frame, accepted as drift's re-resolve is; phase continuity is not kept across a parameter change |
| P7 | Eye bobbing ±0.05 m about dev water's surface after a dive, looking down | Per presented frame: eye update (containment, trigger band, ramp), then the fluid pass reads the frame weight | Below the plane the eye is contained, so the weight is 0; above it the trigger stays on inside its band, so the ramp holds at 1 and the weight at 0: no fluid face carries fog and the tint holds steady; climbing out, the weight rises as the ramp falls |
| P8 | Swimmer inside entity A faces the shared face with entity B (same fluid, or water over sludge) | Eye contained in A; B's outward normal faces the camera, so the normal gate is open and B's proximity is positive | The eye is contained, so the frame weight is 0 and B's face draws no fog whatever the ramp; the swimmer sees the flat tint only |
| P9 | Camera in air sees two outside-facing fluid faces on one pixel (split pool seam, or a near pool over a far pool) | Far volume's draw composites fog to scene depth, then the near volume's draw composites fog to the same scene depth | The overlap equals one composite at T², a denser seam that is accepted: brighter or darker as the murk is brighter or darker than the scene behind it, never past the fog colour |
| P10 | Script time crosses a whole number of wave periods (t = k·period, k up to the 24 h count) on a fluid whose `waveLength` range has min < max | Phase splits into a whole count and a fraction from one split, wrapped over a whole number of hash-repeat spans; next frame evaluates | The pattern only translates: every crest keeps its hashed length and its position plus travel across the boundary; the whole count and the fraction never disagree, so no frame jumps the field |
| P11 | Eye arrives 1 m under a fluid's surface in one frame (respawn, teleport, or level install after unload reset the ramp to 0); or the eye moves into an adjacent fluid this machine does not present | Eye update: containment from `fluid_at_eye`; trigger turns on (or moves); ramp rises from 0 (or falls to 0); the fluid pass reads that frame's weight | The eye is contained, so the weight is 0 from the first frame whatever the ramp; a neighbouring volume's outward face never shows fog while the ramp moves |
| P12 | `waveLength` explicit, `waveAmplitude` omitted | Parser resolves the default amplitude, then validation applies the 8× rule to the resolved amplitude | The rule binds a defaulted amplitude exactly as a written one; the warning names `waveLength` |
| P13 | Eye 0.04 m above an upper pool's surface with a lower pool in view; or eye 0.04 m, then a band-width, below a floating fluid volume's bottom face | Frame weight is 1 (eye not contained, ramp 0); each fragment computes its face's proximity from the camera and that face's plane | Each face's proximity is its own: the upper face's is 0.5 and the lower pool's is 1. The bottom face's outward normal points down, so the eye below it is on its outside: proximity 0.5 at 0.04 m, full fog at a band-width or more |
