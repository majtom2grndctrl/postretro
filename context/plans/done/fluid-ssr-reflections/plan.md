# fluid-ssr-reflections — plan of record

mode: resumable
status: landed
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
- **March hit test (task 1, for owner IP read; stride and budget superseded by rows 10–11).** A step hits when the ray is behind the depth buffer, has been in front of it before (hidden-origin latch), and the nearer of this and the previous sample's view depth lies within `thickness` of the buffer. That is a segment-overlap test over two sample positions, symmetric under min(), with no sign or direction read; step length and exits are unchanged. Without it, steep reflected rays near an object's base stepped past its thickness band and left a gap. Stride is in full-resolution pixels, `max(1, scene_rows / 540)`, so reach covers the same share of the screen at any resolution and every tier marches it; confidence = edge fade (10% band) × reach fade (last 25% of a 64-step budget).
- **Confidence's hit-confidence factor is the reach fade (review round 1, clarification).** The Decision says "c = hit confidence × screen-edge fade, and nothing else". As built, hit confidence is the reach fade over the last quarter of the step budget: it reads the step index alone and is 1 elsewhere. Owner accepted 2026-10-20 (see the IP decision below); the docs will say it.
- **Hit-test band, restated for the owner IP read (review round 1).** The band test asks whether the step's view-depth interval since the last sample reaches within `thickness` behind the buffer: `min(ray_w, last_w) ≤ scene_w + thickness`. It is one rule for every ray, with no direction read. Because view depth is monotonic along a ray, the interval's near end is the previous sample for a receding ray and the current sample for an approaching one. It decides acceptance only: no technique choice, exit, step or weight changes. The IP gate now allowlists the march's exact exits, returns and step chain.
- **Owner IP decision (2026-10-20): both open calls accepted as built.** Engineering triage, not legal advice; counsel's pre-ship claims check is unchanged.
  - **Hit test.** The interval test `min(ray_w, last_w) ≤ scene_w + thickness` stays. It is one order-free rule for every ray: it decides hit acceptance only, and makes no toward-camera determination and no technique choice, exit, skip or re-weight, which every granted Microsoft claim needs. Prior-art note: McGuire & Mara, "Efficient GPU Screen-Space Ray Tracing" (JCGT 3(4), 2014), appears to use the same test. Their listing swaps the previous and current ray depth estimates into a min/max pair and hits on overlap with the depth buffer's thickness band. That predates Microsoft's 2018-02-22 priority. This comes from recollection of the published listing; `ip-check.md` §6 read only the abstract, so confirm it against the listing before relying on it.
  - **Hit confidence.** Hit confidence is the reach fade: c ramps to 0 over the last quarter of the step budget, reading the step index alone. Since the reach cap it is also the deliberate hand-over from SSR to the probe term at about half a screen height. The `context/lib` docs will define hit confidence this way.
- **Target storage is premultiplied.** The target holds `(c·SSR.rgb, c)`, not raw colour. Bilinear filtering then interpolates c·SSR and c linearly, so a footprint straddling a written texel and a cleared one lands between probe-only and SSR (AC 18). Raw colour would darken twice. "Colour and confidence alone" still holds.

## Delegated answers
- **Menu control:** enum radio buttons through `optionChoices` / `radioChoice`, matching `fog_quality`'s graphics-tab row in `content/dev/scripts/frontend-menu.ts`. The pause menu reuses that screen.
- **Probe-streaming drafts:** `claude/probe-streaming-brief-364a5e` is not merged into `origin/main`. It forks at `949a79411`, and its `reflection-probe-streaming` and `reflection-probe-seams` drafts exist only on that branch. Group 4 and the gate are unchanged at `db0f270d3`, so there is nothing to re-read. If it lands mid-build, re-read the gate and the budget row before task 3.

- **Task 1 checkpoint (owner, 2026-10-19):** the tier fractions stay at quarter, half and full per axis. Tasks 2 and 3 proceed now. The 5300M timing waits for the owner's Mac run.

- **Owner decision (2026-10-20): a probes-only tier.** The ladder becomes `off` / `probes` / `low` / `medium` / `high`, default `high`. This supersedes the brief's four-tier ladder; "Off turns off both layers" still holds for `off`.
  - At `probes`, every probe-bound draw takes the reflecting variant and the probe term draws unchanged.
  - No screen-space target is allocated (the placeholder reads c = 0), and no reflection pass is recorded. That is the landed probe-only picture and cost.
  - The menu shows OFF, PROBES, LOW, MEDIUM, HIGH.
  - AC rows read accordingly:
    - **AC 1:** `probes` records no pass.
    - **AC 2:** `probes` takes the reflecting variant.
    - **AC 10:** round-trips five values.
    - **AC 19:** `probes` allocates only the placeholder.
    - **AC 26:** stepping to `probes` removes SSR and keeps the probe term.
- **Owner decision (2026-10-20, revised twice): tier presets, VERY HIGH and ULTRA.** This supersedes the probes-only entry above. The ladder is `off` / `low` / `medium` / `high` / `very_high` / `ultra`, default `high`. `very_high` must run smoothly on the GTX 1660 Super; `ultra` may slow it down, and is aimed at RTX 30x0-class GPUs. Preset values are set from new perf data (Mac agent plus 1660 sweeps); until then the table holds placeholders.
  - `low` is the probe term only: no SSR target and no reflection pass.
  - `medium`, `high` and `ultra` run SSR. Each tier is a preset of resolution fraction, step budget, stride divisor and thickness. This supersedes the brief's "every tier marches the same stride and step budget".
  - Starting presets:
    - medium: ½, 48 steps, divisor 180, 1.0 m
    - high: full, 96 steps, divisor 270, 1.0 m
    - very_high and ultra: placeholders, tuned from the perf sweep
  - Owner intent for `ultra`: deliberately beyond the 5300M, as future-proofing.
  - The dev sliders are live overrides on the active preset.
  - The pillar investigation found the reach default too short. The owner's Mac playtest at ½ render resolution held vsync at 202–256 steps with divisor 135–350, and a lower divisor read sharper because SSR covered more of the water.
  - AC rows read accordingly:
    - **AC 5:** tiers differ in reach; a lower tier never reaches further.
    - **AC 10:** five values.
    - **AC 19 and AC 26:** `low` allocates only the placeholder and records no pass.
    - **AC 28:** `ultra` is judged on the 1660, not the 5300M.
- **Pillar reflection (owner report, 2026-10-20): in scope.** Manual row 24 and the Decision "SSR owns … the camera-facing sides of objects standing in the water" both cover it. Under investigation as a defect or reach limit, not a follow-up brief.
- **Owner decision (2026-10-20): reach capped at half a screen height.** This supersedes the saturated presets of row 10. High, very high and ultra march steps = divisor ÷ 2 (about 0.5 screen heights): high 1/90/180/1.0, very_high 1/180/360/1.5, ultra 1/270/540/2.5. Medium is unchanged at ½/48/180/1.0 (about 0.27).
  - Why: the owner saw probe reflections of objects set back from the water disappear at long reach. In the composite, wherever SSR reports a hit it replaces the probe term, so a longer march trades the probe's content for whatever surface the ray finds on screen.
  - A capture slider check on the 1660 (reflection-probes-test, poses `low` and `west`) isolated the lever. Thickness 4.0 m and 0.25 m gave identical frames at 256 steps / divisor 135. A reach sweep at divisor 180 showed the probe's content clearly at 36–72 steps, faint at 90–108, and gone at 150 and above.
  - Trade-off accepted: SSR no longer reflects anything more than about half a screen height from the reflecting pixel. Past that, the probe term (or nothing, for things the probe never baked) shows instead.
  - The quality step between SSR tiers is now stride alone, at equal reach. Cost falls from the saturated presets because steps roughly halve.

## AC-to-proof

| AC | Row (short) | Proof | Status | Result |
|---|---|---|---|---|
| 1 | Pass count: Off, no fluid and no reflecting draw record none; low/med/high record one | adapter test counting reflection-pass records (`#[cfg(test)]` counter) per scenario | achievable as stated | pass: `the_reflection_pass_records_once_per_reflecting_frame_and_never_at_off` (GPU ran) |
| 2 | Off → reflection-free variant; others → reflecting | render-cpu `shader_variant` test extended with a tier argument | achievable as stated | pass: `shader_variant_reflects_only_a_bound_face_above_zero_reflectivity_under_every_term` |
| 3 | Source order skinned → reflection → fluid; one mask snapshot, one tier | extend `fluid_pass_records_between_skinned_meshes_and_billboards_on_its_own_gate` and `fluid_pass_picks_variants_by_the_frames_group_0_mask` | achievable as stated | pass: source-order and one-snapshot pins |
| 4 | Composite CPU reference: c=0 equals landed, c=1 drops probe, s-independence of F·c·X, opacity/fog independence | render-cpu unit tests over sweeps | achievable as stated | pass: render-cpu `reflection_tests.rs` sweeps (c = 0 equal to float tolerance, operation order) |
| 5 | March CPU reference: tier-invariant hit incl. thin occluder; off-screen/budget c=0; edge band 0<c<1; toward-camera same step; hidden origin skips start overlap | render-cpu unit tests on a synthetic depth buffer | achievable as stated | pass: `screen_march_tests.rs` (4 cases + 2 proptests) |
| 6 | WGSL composite and march match CPU bodies | transcription pins in `fluid_surface_tests.rs` (`wgsl_fn` + `code_only`) | achievable as stated | pass: march, composite and `fs_reflection` whole-body pins |
| 7 | Pixel: one ray per cell from `fluid_reflection_point`'s point and direction; smooth: one per target pixel | source pin that the reflection entry calls `fluid_reflection_point`, plus a CPU test of the cell-invariant ray | achievable as stated | pass: ray-provenance source pin + `the_reflection_pass_marches_one_ray_per_pixel_cell_and_one_per_smooth_fragment` |
| 8 | Reflection layout binds no probe arrays and no group 5, reaches no capture-shadow fn; target in the reflection globals | pipeline-layout inventory test and naga reach walk (`REFLECTION_GLOBALS` += `reflection_target`) | achievable as stated | pass: `fluid_reflection_pass_binds_no_probe_array_and_no_fluid_group_5`, naga reach walks |
| 9 | IP gates: confidence and step read no direction; target holds colour and confidence only; no probe search reaches target | source scan plus naga reach walk | achievable as stated | pass: direction scan, exit/return/step allowlist and whole-body pins under the IP message (3 mutations fail it); target/probe-lookup walk |
| 10 | `reflection_quality` round-trip, absent → high, unrecognized → field fallback, graphics-tab count | `options/tests.rs`, bridge tests, catalog and slot tests, `main.rs` tree test | achievable as stated | pass: options, bridge, catalog, slot table, main.rs tree test (ran) |
| 11 | Grep gates: no style option; no reflection material/FGD input | new xtask gate beside `volume_fgd_keys_name_no_swim_field` | achievable as stated | pass: `player_options_name_no_reflection_style`, `no_world_material_fgd_key_or_texture_rule_takes_a_reflection_input` (exact prefix, suffix and `format!` sets) |
| 12 | Budgets: storage 8, sampled 10→11, uniforms and samplers unchanged | `pipeline_budget_tests.rs` updated rows | achievable as stated | pass: sampled 11 of 16, storage 8, uniforms 7, samplers 2 |
| 13 | Reflection pipeline built at full init, both variants under DX12 FXC; raising the tier compiles nothing | `dx12_fxc_compiles_every_full_init_pipeline` (`--ignored`, log shows ran), plus a source pin that the setter creates no pipeline | achievable as stated | pass: FXC test ran, every full-init pipeline compiled; P14 source pin |
| 14 | Quad reflected at high (smooth) within 2/255 of CPU; differs from Off; low still reads quad; refused faces byte-equal; no validation error | adapter test in `fluid_surface_frame_test.rs` | achievable as stated | pass: GPU ran; quad hits match CPU worst 0/255 high, 1/255 low; refused faces byte-equal; no validation error |
| 15 | Pixel: one value per cell at every tier, differs from Off; hidden-centre cell reads own SSR; off-screen or past-edge centre c=0 | adapter test | achievable as stated | pass: GPU ran; per-cell spread 0 at every tier |
| 16 | Overlapping surfaces: each visible fragment reads its own SSR | adapter test with the visible surface drawn first | achievable as stated | pass: GPU ran; both draw orders byte-equal |
| 17 | Reflectivity 0→1 shows SSR next frame, no target or group 5 rebuild; →0 reflection-free | adapter test via `set_fluid_surface_draws` (the per-frame collect a staged reload feeds) plus build counters | achievable as stated | pass: GPU ran; no target or group 5 rebuild |
| 18 | Unwritten texel reads c=0, equals a fresh renderer; bilinear straddle between probe-only and SSR | adapter test | achievable as stated | pass: GPU ran; unwritten texels read 0; straddle matches CPU mix within 1/255 |
| 19 | Allocation lifecycle (placeholder cases, Off↔tier, P3, P4, P5, resume with Off) | adapter test with target-build and group-5-build counters | achievable as stated | pass: GPU ran (lifecycle cases) |
| 20 | P4 source pin and adapter case name the widened rebuild set | update `fluid_scene_inputs_are_built_at_init_and_rebuilt_only_with_the_scene_targets` and `fluid_scene_inputs_follow_the_scene_depth_across_resizes_with_and_without_a_level` | achievable as stated | pass: P4 source pin + adapter twin |
| 21 | Per-tier extent fraction (ceil, ≥1×1); tier and resolution on one tick → one coherent target (P1, P2) | pure extent-function test plus adapter case | achievable as stated | pass: extent rule tests + one-tick adapter case |
| 22 | Style switch: first frame equals steady state byte for byte (P8) | adapter test | achievable as stated | pass: GPU ran; first frame equals steady state |
| 23 | Camera 1 mm below plane equals Off byte for byte (P10) | adapter test | achievable as stated | pass: GPU ran; equals Off byte for byte |
| 24 | Walls, lights, mover and pillar reflect; pixel blocks line up; smooth unblocked | owner, in-engine | manual | outstanding (owner). No dev map has a mover in view of a probe-bound pool; `reflection-probes-test.map` has the pillar. Not recorded per row: the owner playtested on the Mac (sliders, tiers, the probe-visibility report that led to the reach cap) and called landing on 2026-10-20 |
| 25 | Panning fade to probe or nothing, no hard line | owner, in-engine | manual | not recorded per row; owner called landing 2026-10-20 |
| 26 | Tier stepping coarsens smooth only; pixel keeps blocks; Off removes both; live | owner, in-engine | manual | not recorded per row; owner called landing 2026-10-20 |
| 27 | Camera at plane, underwater, through glass; sludge weaker | owner, in-engine | manual | not recorded per row; owner called landing 2026-10-20 |
| 28 | GPU cost per tier and style, 1660 and 5300M, vs pre-change; Off matches | 1660: executor with `POSTRETRO_GPU_TIMING=1`; 5300M: owner via Metal System Trace | manual (owner judges) | 1660 measured (Task 7 table: High +8.6 ms, Medium +2.5, Low +0.9 at 4K; Off matches pre-change); 5300M measured by the Mac session (5300M ladder sweep below). Both sweeps predate the reach cap, which roughly halves steps on high and above, so shipped tiers cost at most what was measured |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Perf slice.** Build the march WGSL module, the reflection pass (own layout, full-resolution target plus own depth, created at install when a probe-bound face exists, cleared each frame), group 5 binding 2, the new composite in `fs_main_reflecting`, and the `fluid_reflection` timing pair, all at a hard-wired high tier with no option. One adapter smoke test (AC 14 at high). Time the reflection and fluid passes on the 1660 at 4K near under both styles, against the pre-change build, and hand the 5300M trace run to the owner. **Checkpoint: report the numbers before task 3**, since they may move the tier fractions. | integrating executor | — | built; renderer lib 968 passed, render-cpu fluid 155, clippy clean; smoke test GPU case ran (later replaced by task 4's AC 14 test). done (owner checkpoint passed; 5300M timing deferred to AC 28) |
| 2 | CPU references for the march and the composite (render-cpu, beside the probe references), GPU-free tests, transcription pins, IP comments beside the march, composite and probe lookup, and the IP source scans (AC 4, 5, 6, 7, 9) | integrating executor | 1 | done: render-cpu `screen_march.rs` + tests, layered composite reference, pins and IP gates; render-cpu fluid 166, renderer fluid 82, clippy clean. Composite at c = 0 equals the landed one to float tolerance (operation order), not bit for bit |
| 3 | Tier and lifecycle: render-cpu `ReflectionQuality` with `ALL` and extent fractions; a tier argument on `shader_variant`; `Renderer::set_reflection_quality` (boot-state kept, early-out when unchanged); target create, resize and release; the widened group 5 rebuild set; scene-target rebuild order (P2); P4 pins; the pass-record gate; budget and inventory tests (AC 1, 2, 3, 8, 12, 19, 20, 21) | integrating executor | 1 | done: render-cpu `FluidReflectionQuality` (sole extent rule), tier on `shader_variant`/`frame_draws`, `Renderer::set_reflection_quality` (boot-state kept), widened P4 pins, P14 source pin; renderer lib 980, render-cpu fluid 170, 20 adapter cases ran. Weaker than written: the no-full-renderer setter branch is source-pinned only (offscreen renderers build full); P1 frame start simulated by `commit_extents()` |
| 4 | Correctness adapter tests and fixes: pixel cells and hidden centres, overlapping surfaces, live reflectivity, unwritten texels and bilinear, style switch, below plane (AC 14 full, 15–18, 22, 23) | integrating executor | 2, 3 | done: 9 GPU cases in `fluid_reflection_frame_test.rs` (all ran; CPU march on read-back depth; P12 via validation error scope); no renderer bug found; four mutation runs each fail their test. Weaker than written: AC 14 quad hits are all c = 1 (fade band checked on a black wall); AC 15 low/medium check per-cell uniformity, CPU match at high; AC 17/22 drive renderer setters, not an app staged reload; AC 23 uses a 2° grazing view |
| 5 | Player option end to end, following `fog_quality`'s touch list: `PlayerOptions` field and enum, persist, bridge, catalog (`["off","low","medium","high"]`), slot table, `MIRRORED_GAME_STATE_LEAVES`, regenerated SDK typedefs plus the two snapshot fixtures, frontend menu, `main.rs` graphics-tab test, `render_profile` mapping and drift guard, live apply in `options_menu.rs`, splash and lifecycle apply; the xtask grep gate (AC 10, 11) | delegated worker (owns `crates/postretro/src/options/**`, `startup/**`, `app/options_menu.rs`, `main.rs` test, `crates/entities/src/{engine_state_catalog,slot_table}.rs`, `crates/script-compiler/src/light_membership.rs`, `sdk/types/**`, typedef fixtures, `content/dev/scripts/frontend-menu.ts`, `crates/xtask/src/environment_gates.rs`); consumes task 3's `Renderer::set_reflection_quality` and the render-cpu enum | 3 | done: option, persist, bridge, catalog, slot table, mirror, SDK typedefs (regen byte-identical), menu radios, startup/live/install apply (capture untouched, renders High); xtask gates `player_options_name_no_reflection_style` and `no_world_material_fgd_key_or_texture_rule_takes_a_reflection_input` (planted violations fail them). postretro 101, entities 34, sim typedef 44, script-compiler 51, xtask gates 9, clippy clean |
| 6 | Dev tuning: step budget and thickness sliders in the "SDF / Fog Quality" section of `debug_ui/mod.rs` (dev-tools only) | integrating executor | 3 | done: steps, thickness and rows-per-stride-pixel sliders; uniform-only, upload on change; resets on a full-init rebuild like the SDF tuning |
| 7 | DX12 FXC run (`cargo test -p postretro-renderer --lib dx12_fxc -- --ignored`) and the in-engine 1660 perf table across every tier and style (AC 13, 28 on the 1660) | integrating executor | 4, 5, 6 | done: DX12 FXC `dx12_fxc_compiles_every_full_init_pipeline` ran (every full-init pipeline compiled); 1660 table below |
| 8 | Preflight, then review-panel and fix-review-findings loop, then the full gate including `--features postretro/dev-tools` | integrating executor | 7 | preflight green: fmt, clippy, release check, crate graph; workspace test 11 784 passed after updating `ui_slot_snapshot_clones_present_values_and_skips_valueless_slots` for the new slot (62 → 63, default pinned). Review round 1 (9 agents, no 🔴): fixed NaN-on-miss write, near-plane ε guard, resize drop order, exact material/suffix/FGD-class gate sets, stronger march IP allowlist, fluid sampling-rule pin, comment drift. Round 2 (2 agents): clamp hit colour to f16 max, whole-body march pin under the IP message, pin the sibling-map `format!` suffixes, thickness/mask doc fixes, plan-qualified row labels. Full gate green: fmt, clippy (plain and dev-tools), workspace test 11 787 / dev-tools 11 890 passed, release check, crate graph. Round 3 (1 agent): whole-body `fs_reflection` pin under the IP message, no-skip suffix parsing, colour clamped to [0, f16 max]; each of three attacks proven to fail its gate; renderer lib 989, xtask 34, dev-tools adapter 9 (all ran). Loop closed |
| 9 | Owner ladder revision: six tiers (`off`/`low`/`medium`/`high`/`very_high`/`ultra`), `low` probe-only, SSR tiers as presets in render-cpu `FLUID_REFLECTION_PRESETS`, dev sliders as live overrides, capture scene fields `reflection_quality` / `reflection_style` / `reflection_march` for sweeps | integrating executor (delegated build) | 8 | built: placeholder presets medium ½/48/180/1.0, high 1/96/270/1.0, very_high 1/192/180/1.5, ultra 1/256/135/2.5; render-cpu fluid 173, renderer lib 992, app (dev-tools, capture) 161/148, entities 34, sim typedef 44, xtask 34, workspace clippy clean. High's default march changed (was 64/540/0.5), so earlier High timings are not comparable. Presets await the 5300M + 1660 sweep |
| 10 | Owner-approved presets: medium ½/48/180/1.0, high 1/193/180/1.0, very_high 1/386/360/1.5, ultra 1/578/540/2.5; stride made fractional (`max(1, rows ÷ divisor)`) so saturated tiers reach the full screen at every height | integrating executor (delegated) | 9 | built: render-cpu fluid 173, renderer lib 992, dev-tools adapter 11 (all ran), app capture/options 227, workspace clippy clean; full workspace gate green on 91f16b47c (fmt, clippy, workspace test 11 793 / dev-tools 11 896 passed, release check, crate graph). Superseded by row 11 |
| 11 | Owner reach cap: high 1/90/180/1.0, very_high 1/180/360/1.5, ultra 1/270/540/2.5 (steps = divisor ÷ 2, about half a screen height); medium unchanged | integrating executor | 10 | built: preset table, sizing and stride comments, and the two preset pins updated; render-cpu fluid 173, renderer lib 992, renderer dev-tools reflection 36 (adapter cases ran), app capture 131, fmt and clippy clean |

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

## Task 1 measurement — GTX 1660 Super (Vulkan, driver 617.14)

Here, BEFORE is `db0f270d3` and AFTER is `a89aca24b`, each a release `capture,dev-tools` binary. Runs used `POSTRETRO_GPU_TIMING=1`, `POSTRETRO_SH_STREAMING=sync-proof`, 120 warmup + 360 samples, and ABBA over 2 rounds, with 26/26 runs valid. Poses are the fluid-base-pass-cost precedent's: e24 `near` and the probes-map pool, both straight down from 1.6 m. Smooth used a third binary with a local, reverted default-style override, because capture cannot select a style. The march ran at a 64-step budget, with stride `max(1, rows/540)` and the target at full resolution. Values are ms, the mean of 2 runs. All spreads are ≤ 0.1 ms.

| view | mask / style | fluid_surface before → after | fluid_reflection | GPU frame before → after | SSR added |
|---|---|---|---|---|---|
| near 4K | reflection-free | 0.827 → 0.825 | — | 8.966 → 8.958 | 0 |
| near 4K | reflecting pixel | 3.324 → 3.612 | 8.309 | 12.528 → 21.193 | +8.60 |
| near 4K | reflecting smooth | 3.324 → 3.031 | 7.852 | 12.528 → 20.123 | +7.56 |
| probes 4K | reflection-free | 0.787 → 0.787 | — | 8.418 → 8.436 | 0 |
| probes 4K | reflecting pixel | 4.614 → 4.886 | 8.277 | 13.227 → 21.758 | +8.55 |
| probes 4K | reflecting smooth | 4.614 → 4.222 | 7.807 | 13.227 → 20.623 | +7.42 |
| near 1080p | reflecting pixel | 0.893 → 0.982 | 2.096 | 3.352 → 5.538 | +2.19 |
| near 1080p | reflecting smooth | 0.893 → 0.818 | 1.976 | 3.352 → 5.243 | +1.90 |

- **Off matches pre-change.** The reflection-free pass was unchanged within spread, and no reflection pass was recorded.
- **The reflection pass dominates the added cost.** Its cost scales with shaded target pixels × the step budget, not with what the water reflects. At these straight-down poses every reflected ray heads toward the camera and runs to the screen edge or the budget, with no hit. The IP rule (no exit by direction) makes this the worst case by design.
- **1080p at high is a proxy for 4K at medium**, because both make the same number of full-resolution loads. That puts 4K medium at about 2.1 ms. 4K low (¼ per axis) projects to about 0.5 ms, though that is not measured.
- **Reach is short.** A 64-step budget reaches about 12% of screen height. Longer reach costs proportionally more.

## Task 7 measurement — GTX 1660 Super, every tier

All configurations run at HEAD `5be267241`, one release `capture,dev-tools` binary each, with a local, reverted `#[default]` override for tier and style, because capture loads no options. The method is the same as task 1: 48/48 runs valid, ABBA over 2 rounds, and all spreads ≤ ±0.11 ms. "Before" is `db0f270d3`.

**SSR added** is fluid_reflection + fluid_surface after the change, minus the before build's reflecting fluid_surface (ms).

| config | near 4K | probes 4K | near 1080p | fluid_reflection near 4K |
|---|---|---|---|---|
| Low pixel | +0.93 | +0.91 | +0.25 | 0.66 |
| Medium pixel | +2.53 | +2.52 | +0.65 | 2.27 |
| High pixel | +8.57 | +8.43 | +2.14 | 8.30 |
| Low smooth | +1.09 | +1.05 | — | 1.49 |
| Medium smooth | +1.84 | +1.80 | — | 2.25 |
| High smooth | +7.56 | +7.33 | — | 7.86 |
| Off | −2.50 | −3.75 | — | — |

- **Off.** fluid_surface is 0.830 near and 0.784 probes, against the before build's reflection-free 0.823 and 0.786. No reflection pass is recorded at Off. Like for like, the before build's reflecting frame with its fluid cost swapped for Off's is within 0.14 ms of Off's frame. The `0x13F` mask also drops forward light terms (≈1 ms), so a raw frame comparison against it is not like for like.
- **The fluid pass alone** costs +0.27 ms over before in pixel style, from the target read and the cell projection. In smooth style it costs 0.3–0.4 ms less than before.
- **Low smooth's pass costs 2.3× Low pixel's.** At ¼ resolution, neighbouring smooth target pixels march rays 4 scene pixels apart, so depth reads lose cache coherence. Pixel rays are shared per water cell. Medium and High show no such gap. This is not a defect.
- **The 5300M is outstanding** (AC 28, owner run).

## Ladder sweep: GTX 1660 Super, af8bfef61 (placeholder presets)

One release `capture,dev-tools` binary, tiers and overrides set through the capture scene fields. Method as task 7 (sync-proof SH, 120 + 360 frames, ABBA over 2 rounds). 276 runs, all valid; every run's logged settings matched the request. Spreads ≤ ±0.06 ms, except one at ±0.24 ms.

Poses, engine metres, fov 100:

| pose | map | position | yaw | pitch |
|---|---|---|---|---|
| near | e24 | (−4.8768, 1.6, −44.2976) | — | −89 |
| probes | probes map | (−6.5024, 1.6, −6.5024) | — | −89 |
| A, pillar basin | reflection-probes-test | (−2.6416, 1.6, −25.4) | 40 | −20 |
| B, largest water | vault-test (348 m²) | (−35.052, 4.394, −75.1332) | −125 | −20 |
| C, e24 oblique | e24 | (−1.016, 1.6, −44.2976) | 90 | −20 |

GPU frame in ms, with `fluid_reflection` in brackets, pixel style:

| pose | res | off | low | medium | high | very_high | ultra |
|---|---|---|---|---|---|---|---|
| near | 1080p | 2.68 | 3.36 | 3.79 [0.40] | 5.74 [2.34] | 5.52 [2.12] | 5.07 [1.67] |
| A | 1080p | 2.68 | 3.16 | 3.43 [0.26] | 4.70 [1.52] | 5.13 [1.95] | 4.70 [1.53] |
| B | 1080p | 2.40 | 2.74 | 3.04 [0.29] | 4.37 [1.62] | 4.82 [2.06] | 4.37 [1.61] |
| near | 4K | 9.92 | 12.57 | 14.20 [1.59] | 21.88 [9.18] | 20.80 [8.16] | 19.09 [6.49] |
| A | 4K | 9.42 | 11.21 | 12.22 [0.94] | 17.22 [5.95] | 18.83 [7.58] | 17.17 [5.90] |
| B | 4K | 8.40 | 9.66 | 10.73 [1.03] | 15.99 [6.27] | 18.03 [8.30] | 16.18 [6.45] |

- **Reach saturates.** Reach is about steps ÷ divisor screen heights. Past about 1.0 a missed ray has already left the screen, so extra steps cost nothing. 128 to 384 steps at divisor 135 measure identical.
- **The divisor sets the cost.** Once saturated, the pass is linear in the divisor: about 0.8 + 0.038 × divisor ms at 4K pose A. Size steps at about 1.07 × the divisor.
- **Thickness costs nothing.** It is a quality choice only.
- **The placeholders are not monotone.** `ultra` (256/135) costs less than `very_high` (192/180), and its steps past saturation are dead weight.

Candidate presets (thickness 1.5 m for VH, 2.5 m for ultra):

| candidate | steps / divisor | 1080p GPU frame | 4K GPU frame | note |
|---|---|---|---|---|
| VH | 192 / 180 | 4.8–5.6 ms | 18.0–21.3 ms | |
| VH fine | 288 / 270 | 5.7–6.5 ms | 21.8–24.7 ms | |
| | 384 / 360 | 6.6–7.3 ms | 25.4–28.0 ms | |
| ultra | 576 / 540 | 8.4–9.0 ms | 31.5–34.1 ms | ≈ 2× the 16.7 ms budget, the owner's RTX 30x0 aim |

At 1080p every config measured stays at or under 9.1 ms at every pose. At 4K only `off`, `low` and `medium` hold 16.7 ms everywhere; the 4K base frame alone is 8.4–9.9 ms. Smooth style costs 0.1–0.2 ms less than pixel.

Raw data: session scratchpad `results3/`. Final presets wait on the 5300M sweep (Mac session, branch `claude/fluid-ssr-reflections-mac-perf`).

## Ladder sweep: RX 5300M (Metal), af8bfef61, Mac session

Kit and raw results: `measurements/fluid-ssr-reflections/` (`5300m-notes.md`, `results-quick.*`, `results-pilot.*`, `metal/`).

**Method.** Metal System Trace at 1536×960, the owner's binding Mac play extent (half the laptop panel). One round, 120 + 360 frames, pixel style, all terms, sync-proof SH. Every run's effective settings were confirmed in the log.

Whole-frame GPU in ms, `fluid_reflection` in brackets:

| config | near | B vault | A pillar |
|---|---|---|---|
| off | 6.03 | 6.51* | 6.66* |
| medium 0.5/48/180 | 8.69 [0.61] | 7.61* [0.50] | — |
| high 96/270 | 11.48 [3.42] | 8.72 [2.32] | 10.34 [2.25] |
| very_high 192/180 | 11.29 [3.04] | — | — |

Saturated divisor walk at near, steps = ⌈1.07·d⌉, thickness 1.5:

| divisor | whole frame | `fluid_reflection` |
|---|---|---|
| 135 | 10.38 | [2.29] |
| 270 | 12.55 | [4.49] |
| 540 | 21.48 | [12.03] |

Medium at near, 64 steps: fraction 0.375 gives 8.70 [0.45]; fraction 0.75 gives 9.84 [1.49].

\* About 20% of frames were incomplete in these runs, so treat them as directional. The near-off figure is the pilot's valid run.

Findings:
- Everything holds 16.7 ms except d = 540 (about 46 fps). d = 360 is interpolated at about 14–15 ms (not measured).
- The reflecting variant alone adds about 2 ms over off at near (`fluid_surface` 0.46 → 2.4 ms), and `low` pays this too.
- Cost tracks screen fill, not pool area.
- The busy-median clock varied 814–1134 MHz because capture does not saturate this extent, so these figures read high (conservative).

**Not run:** the Samsung extent, which the owner plays at roughly ¼ to ⅓ render, no more pixels than 1536×960. Also not run: 4K, smooth, and `ultra`.
