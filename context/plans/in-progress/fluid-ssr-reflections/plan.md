# fluid-ssr-reflections — plan of record

mode: resumable
status: approved
read at: db0f270d3

Feature branch: `claude/fluid-ssr-reflections-0b3212` (worktree `.claude/worktrees/fluid-ssr-reflections-0b3212`), cut from the claim commit `db0f270d3`.

Every source symbol that Decisions and Path cite was re-read at `db0f270d3`. No Decision premise is false. The corrections below are Path-level only.

## Corrections
- **Derivatives.** The brief says `fluid_reflection_cell_centre` builds texel axes from `dpdx`/`dpdy`. In the current source it takes precomputed `FluidTexelAxes`. The derivatives are taken at the top of `fs_main_reflecting` (`fluid_surface.wgsl:491`) and `fluid_shade` (`:415`), in uniform control flow, and `fluid_pixel_reflection_reads_the_unwarped_cell_and_no_drift_or_wave` pins exactly two of each. The reflection entry follows the same shape: derivatives at entry top, axes built before the march loop, and no implicit-derivative sample inside the loop (FXC).
- **Composite reference location.** The composite's CPU reference is `reflection_composite` / `reflection_fragment` in render-cpu `fluid_surface/reflection.rs:418/564`, not `reflection_style.rs`. Today `reflection_fragment` passes `fresnel × search.weight × band`, so the landed F already contains s. The new composite takes F (Fresnel × band) and s separately. It is written beside the existing one, and the transcription pins in `fluid_surface_tests.rs` (`fluid_reflection_wgsl_bodies_match_the_cpu_reference`) gain its rows.
- **Probe-bound tracking at install.** The brief keys the target to "a level with a probe-bound fluid face", but install keeps no such flag: `create_probe_index_buffer` discards the indices. I'll record `has_probe_bound_face` on `InstalledFluidSurfaces` during `install`, using the exact test: any index ≠ `FLUID_REFLECTION_NO_PROBE` after `fill_probe_indices` over `installable_probe_section`.
- **Reflection pass inputs.** The brief's Path lists depth, scene colour, style, scale and tuning. In source, the pass needs three more things:
  - Group 0 (camera, time, mask) for the gate and pixel motion.
  - The face material's base texture dimensions for `texels_per_pixel`. UVs are normalized by installed texture size, so it binds the world material group per material range, as the fluid pass does.
  - The volume row (reflectivity) and the probe table (box, capture point).

  It gets its own pipeline layout:
  - 0 camera.
  - 1 material.
  - 2 a reflection-pass group: scene depth, scene colour, and a uniform holding style, target scale, scene-scale `texels_per_pixel` factor and dev tuning.
  - 3 a group-4 subset BGL: params window (dynamic) plus probe table only, built over the same buffers.

  It binds no probe colour or distance array and no fluid group 5, so AC 8 holds as written.
- **Uniform upload placement.** The P4 source pin asserts that `upload_scene_inputs(queue)` sits inside the fluid pass block. The reflection pass needs the new style and scale on the same frame (P8), so the upload moves to before the reflection pass, and the pin moves with it.
- **P4 pin shape.** The current pin forbids `.rebuild_scene_inputs(` in `fluid_surface.rs` and anywhere in `renderer_resources.rs` (unload), and forbids `scene_inputs` in `install`/`clear`. The brief's widened set (full init, scene-target rebuild, a fluid install or tier change that creates or drops the target, unload) needs those three sites. AC 20 already calls for widening the pin to name exactly them, so this is the planned change, not a contract change.
- **Group 5 closed inventory.** `fluid_pass_declares_no_history_or_precomputed_intersection_binding` asserts `bindings(5) == [0,1]` and forbids `history`, `previous`, `prev_`, `last_frame`, `reproject`, `temporal`, `intersection`, `hit_buffer` and `hit_table` in names and code. Binding 2 joins the inventory. The new names avoid those tokens: the target is `reflection_target`, and confidence is `confidence`.
- **`fluid_scene_inputs_layout_is_a_depth_texture_and_a_small_uniform`** pins 16 bytes and `entries[0..1]`. The uniform stays 16 bytes, with `spare` becoming `ssr_scale: vec2<f32>`, and the entries test grows to three.
- **Timing pair.** `TIMING_PAIR_COUNT` is 16 and the fluid pair is 15. The reflection pass gets its own labelled pair (`fluid_reflection`) for the GPU-cost row.
- **Fog is not re-applied at level install** (`lifecycle.rs:685-697`). The reflection setter follows `set_surface_depth_quality` instead: it keeps the value in boot state, so it works before full init and with no level, and install and full-init consult the kept tier.
- **Shared ray code split out (task 1).** `fluid_reflection.wgsl` also references the probe arrays, so the gate, ray and style functions moved verbatim into `fluid_reflection_ray.wgsl`, which both shaders include. The march is `screen_march.wgsl`; the pass is `fluid_reflection_pass.wgsl` plus `render/fluid_surface/reflection_pass.rs`. Group 0, row and probe structs are duplicated in the pass shader, pinned member for member by a test.
- **March hit test (task 1, for owner IP read).** A step hits when the ray is behind the depth buffer, has been in front of it before (hidden-origin latch), and the nearer of this and the previous sample's view depth lies within `thickness` of the buffer. That is a segment-overlap test over two sample positions, symmetric under min(), with no sign or direction read; step length and exits are unchanged. Without it, steep reflected rays near an object's base stepped past its thickness band and left a gap. Stride is in full-resolution pixels, `max(1, scene_rows / 540)`, so reach covers the same share of the screen at any resolution and every tier marches it; confidence = edge fade (10% band) × reach fade (last 25% of a 64-step budget).
- **Target storage is premultiplied.** The target holds `(c·SSR.rgb, c)`, not raw colour. Bilinear filtering then interpolates c·SSR and c linearly, so a footprint straddling a written texel and a cleared one lands between probe-only and SSR (AC 18). Raw colour would darken twice. "Colour and confidence alone" still holds.

## Delegated answers
- **Menu control:** enum radio buttons through `optionChoices` / `radioChoice`, matching `fog_quality`'s graphics-tab row in `content/dev/scripts/frontend-menu.ts`. The pause menu reuses that screen.
- **Probe-streaming drafts:** `claude/probe-streaming-brief-364a5e` is not merged into `origin/main`. It forks at `949a79411`, and its `reflection-probe-streaming` and `reflection-probe-seams` drafts exist only on that branch. Group 4 and the gate are unchanged at `db0f270d3`, so there is nothing to re-read. If it lands mid-build, re-read the gate and the budget row before task 3.

## AC-to-proof

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Pass count: Off, no fluid and no reflecting draw record none; low/med/high record one | adapter test counting reflection-pass records (`#[cfg(test)]` counter) per scenario | achievable as stated |
| 2 | Off → reflection-free variant; others → reflecting | render-cpu `shader_variant` test extended with a tier argument | achievable as stated |
| 3 | Source order skinned → reflection → fluid; one mask snapshot, one tier | extend `fluid_pass_records_between_skinned_meshes_and_billboards_on_its_own_gate` and `fluid_pass_picks_variants_by_the_frames_group_0_mask` | achievable as stated |
| 4 | Composite CPU reference: c=0 equals landed, c=1 drops probe, s-independence of F·c·X, opacity/fog independence | render-cpu unit tests over sweeps | achievable as stated |
| 5 | March CPU reference: tier-invariant hit incl. thin occluder; off-screen/budget c=0; edge band 0<c<1; toward-camera same step; hidden origin skips start overlap | render-cpu unit tests on a synthetic depth buffer | achievable as stated |
| 6 | WGSL composite and march match CPU bodies | transcription pins in `fluid_surface_tests.rs` (`wgsl_fn` + `code_only`) | achievable as stated |
| 7 | Pixel: one ray per cell from `fluid_reflection_point`'s point and direction; smooth: one per target pixel | source pin that the reflection entry calls `fluid_reflection_point`, plus a CPU test of the cell-invariant ray | achievable as stated |
| 8 | Reflection layout binds no probe arrays and no group 5, reaches no capture-shadow fn; target in the reflection globals | pipeline-layout inventory test and naga reach walk (`REFLECTION_GLOBALS` += `reflection_target`) | achievable as stated |
| 9 | IP gates: confidence and step read no direction; target holds colour and confidence only; no probe search reaches target | source scan plus naga reach walk | achievable as stated |
| 10 | `reflection_quality` round-trip, absent → high, unrecognized → field fallback, graphics-tab count | `options/tests.rs`, bridge tests, catalog and slot tests, `main.rs` tree test | achievable as stated |
| 11 | Grep gates: no style option; no reflection material/FGD input | new xtask gate beside `volume_fgd_keys_name_no_swim_field` | achievable as stated |
| 12 | Budgets: storage 8, sampled 10→11, uniforms and samplers unchanged | `pipeline_budget_tests.rs` updated rows | achievable as stated |
| 13 | Reflection pipeline built at full init, both variants under DX12 FXC; raising the tier compiles nothing | `dx12_fxc_compiles_every_full_init_pipeline` (`--ignored`, log shows ran), plus a source pin that the setter creates no pipeline | achievable as stated |
| 14 | Quad reflected at high (smooth) within 2/255 of CPU; differs from Off; low still reads quad; refused faces byte-equal; no validation error | adapter test in `fluid_surface_frame_test.rs` | achievable as stated |
| 15 | Pixel: one value per cell at every tier, differs from Off; hidden-centre cell reads own SSR; off-screen or past-edge centre c=0 | adapter test | achievable as stated |
| 16 | Overlapping surfaces: each visible fragment reads its own SSR | adapter test with the visible surface drawn first | achievable as stated |
| 17 | Reflectivity 0→1 shows SSR next frame, no target or group 5 rebuild; →0 reflection-free | adapter test via `set_fluid_surface_draws` (the per-frame collect a staged reload feeds) plus build counters | achievable as stated |
| 18 | Unwritten texel reads c=0, equals a fresh renderer; bilinear straddle between probe-only and SSR | adapter test | achievable as stated |
| 19 | Allocation lifecycle (placeholder cases, Off↔tier, P3, P4, P5, resume with Off) | adapter test with target-build and group-5-build counters | achievable as stated |
| 20 | P4 source pin and adapter case name the widened rebuild set | update `fluid_scene_inputs_are_built_at_init_and_rebuilt_only_with_the_scene_targets` and `fluid_scene_inputs_follow_the_scene_depth_across_resizes_with_and_without_a_level` | achievable as stated |
| 21 | Per-tier extent fraction (ceil, ≥1×1); tier and resolution on one tick → one coherent target (P1, P2) | pure extent-function test plus adapter case | achievable as stated |
| 22 | Style switch: first frame equals steady state byte for byte (P8) | adapter test | achievable as stated |
| 23 | Camera 1 mm below plane equals Off byte for byte (P10) | adapter test | achievable as stated |
| 24 | Walls, lights, mover and pillar reflect; pixel blocks line up; smooth unblocked | owner, in-engine | manual |
| 25 | Panning fade to probe or nothing, no hard line | owner, in-engine | manual |
| 26 | Tier stepping coarsens smooth only; pixel keeps blocks; Off removes both; live | owner, in-engine | manual |
| 27 | Camera at plane, underwater, through glass; sludge weaker | owner, in-engine | manual |
| 28 | GPU cost per tier and style, 1660 and 5300M, vs pre-change; Off matches | 1660: executor with `POSTRETRO_GPU_TIMING=1`; 5300M: owner via Metal System Trace | manual (owner judges) |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Perf slice.** Build the march WGSL module, the reflection pass (own layout, full-resolution target plus own depth, created at install when a probe-bound face exists, cleared each frame), group 5 binding 2, the new composite in `fs_main_reflecting`, and the `fluid_reflection` timing pair, all at a hard-wired high tier with no option. One adapter smoke test (AC 14 at high). Time the reflection and fluid passes on the 1660 at 4K near under both styles, against the pre-change build, and hand the 5300M trace run to the owner. **Checkpoint: report the numbers before task 3**, since they may move the tier fractions. | integrating executor | — | built; renderer lib 968 passed, render-cpu fluid 155, clippy clean; smoke `a_standing_quad_reflects_in_a_probe_bound_pool_over_the_probe_term` GPU case ran. Timing pending |
| 2 | CPU references for the march and the composite (render-cpu, beside the probe references), GPU-free tests, transcription pins, IP comments beside the march, composite and probe lookup, and the IP source scans (AC 4, 5, 6, 7, 9) | integrating executor | 1 | |
| 3 | Tier and lifecycle: render-cpu `ReflectionQuality` with `ALL` and extent fractions; a tier argument on `shader_variant`; `Renderer::set_reflection_quality` (boot-state kept, early-out when unchanged); target create, resize and release; the widened group 5 rebuild set; scene-target rebuild order (P2); P4 pins; the pass-record gate; budget and inventory tests (AC 1, 2, 3, 8, 12, 19, 20, 21) | integrating executor | 1 | |
| 4 | Correctness adapter tests and fixes: pixel cells and hidden centres, overlapping surfaces, live reflectivity, unwritten texels and bilinear, style switch, below plane (AC 14 full, 15–18, 22, 23) | integrating executor | 2, 3 | |
| 5 | Player option end to end, following `fog_quality`'s touch list: `PlayerOptions` field and enum, persist, bridge, catalog (`["off","low","medium","high"]`), slot table, `MIRRORED_GAME_STATE_LEAVES`, regenerated SDK typedefs plus the two snapshot fixtures, frontend menu, `main.rs` graphics-tab test, `render_profile` mapping and drift guard, live apply in `options_menu.rs`, splash and lifecycle apply; the xtask grep gate (AC 10, 11) | delegated worker (owns `crates/postretro/src/options/**`, `startup/**`, `app/options_menu.rs`, `main.rs` test, `crates/entities/src/{engine_state_catalog,slot_table}.rs`, `crates/script-compiler/src/light_membership.rs`, `sdk/types/**`, typedef fixtures, `content/dev/scripts/frontend-menu.ts`, `crates/xtask/src/environment_gates.rs`); consumes task 3's `Renderer::set_reflection_quality` and the render-cpu enum | 3 | |
| 6 | Dev tuning: step budget and thickness sliders in the "SDF / Fog Quality" section of `debug_ui/mod.rs` (dev-tools only) | integrating executor | 3 | |
| 7 | DX12 FXC run (`cargo test -p postretro-renderer --lib dx12_fxc -- --ignored`) and the in-engine 1660 perf table across every tier and style (AC 13, 28 on the 1660) | integrating executor | 4, 5, 6 | |
| 8 | Preflight, then review-panel and fix-review-findings loop, then the full gate including `--features postretro/dev-tools` | integrating executor | 7 | |

**Hot-path constraints carried by every task:**
- No per-frame allocation. The target, its depth and its bind groups are created only at the listed lifecycle points.
- The pass is skipped entirely when the frame has no reflecting draw.
- The march has a fixed step budget at a full-resolution stride.
- The uniform uploads only on change.
- Cost is measured on the 1660, with the owner measuring on the 5300M.

**Context updates at landing:**
- `rendering_pipeline.md`:
  - §7.3a: the screen-space layer becomes built, not "decided".
  - §8: the march module joins the shader-module list.
  - §10: group 5 binding 2 and the 11/16 budget.
  - §12: the `fluid_reflection` timing label.
- `player_options.md` §4: the reflection quality row.
- `ui.md` options list.
