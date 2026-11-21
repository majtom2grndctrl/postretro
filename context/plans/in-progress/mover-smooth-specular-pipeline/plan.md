# mover-smooth-specular-pipeline — plan of record

mode: compact
status: active
read at: 1a66523ae (no source change to c56f894e7; Decision reads reused)

## Corrections
- "A style committed before full init" → offscreen test renderers are full-ready at construction (`Renderer::new_offscreen`), so no test can commit before the first full init. `finish_full_init` rebuilds the full renderer through the same `build_full_renderer` path the first init takes; planning around it by proving the "before full init" and "before a full-renderer rebuild" halves of R2 with one commit → `finish_full_init` → install → frame case.

## Delegated answers
- Whether the mover's style word survives — **retired.** Each pipeline is its own style, so a word nothing reads would be a second style signal that can drift from the bound pipeline. Bytes 28..32 of `KinematicLightParams` become padding; `SpecularStyle::kinematic_word` and `KINEMATIC_SPECULAR_STYLE_SMOOTH` go; their tests move to pin pipeline selection.
- Albedo read before or after the loop in the smooth pipeline — pending the Task 1 screen.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 smooth shade vs frozen loop composed the old way, within the tint bound, every whole-loop case, entry's own composition, non-vacuous | `kinematic_specular_test` smooth whole-loop row (GPU readback, harness splices the smooth composition function) | achievable as stated |
| A2 pixel cases bit-exact, oracles unchanged | existing `kinematic_specular_test` pixel cases, splicing the pixel loop | achievable as stated |
| A3 forward smooth and untinted identity unchanged | existing `world_specular_test` identity tests, untouched | achievable as stated |
| A4 per style: diffuse tint-invariant, tinted specular matches the tint function | `diffuse_is_identical_with_and_without_tint`, `tint_mix_matches_the_cpu_reference` (smooth row reads the one sum) | achievable as stated |
| A5 mover pass builds exactly three render pipelines | GPU test counting `gpu_creation_counts` across `KinematicBrushPass::new` | achievable as stated |
| A6 both pipelines build with cube arrays off; both entries validate in the no-cube source | same GPU test with cube arrays off; `kinematic_shader_source_validates_without_cube_arrays` names both entries | achievable as stated |
| A7 each record reports its bound style per view; live, mirror, first-frame rows read it | test-only bound-style report on `KinematicBrushPass`; frame tests read it | achievable as stated |
| A8 entry↔style source pin; report is the selecting value; mirror report present | source pin on the style→entry map and `record_draws`; mirror row asserts a mirror report | achievable as stated |
| A9 smooth loop and its helpers use no implicit-derivative sample | source test over the smooth loop's call graph | achievable as stated |
| A10 live change binds the other pipeline next frame, both directions, no extra creation | `a_live_specular_style_change_...` reading the report | achievable as stated |
| A11 mirror binds the main view's pipeline, change frame included | planar mirror frame test with a mover in the mirror | achievable as stated |
| A12 every selection reads the frame snapshot | source pin over both `record_draws` call sites | achievable as stated |
| A13 committed before full init / rebuild / install / load in flight → first frame | `specular_style_reaches_the_first_frame_of_each_install`, `specular_style_committed_mid_build_reaches_the_published_level` (see Corrections) | achievable as stated |
| A14 smooth keeps highlights through Surface Depth Off→On; style change at Off leaves the march off | harness Off/On test asserts smooth highlights; frame test reads the material uniform's march word across a change | achievable as stated |
| M1 register screen of both pipelines holds 8 waves/SIMD wave32 | `tools/metal_register_screen` per entry, counts in its README | manual (run here: this machine is the target) |
| M2 clock-pinned re-time, both poses, smooth ≤ pre-feature, pixel ≤ 1a66523ae + 0.15 ms | `measurements/specular-mover-cost/` kit | manual |
| M3 dev map movers unchanged per style; toggle with mirror in view, no stall | owner, in-engine | manual |
| M4 §7.3, §4, §8 text | land-the-plane context update, reviewed | manual |
| M5 kit and README on the branch, no hook commit | `git log` / `git diff` on the branch | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Smooth entry (V5s shape) and pixel entry (smooth compiled out) over shared code; both beauty pipelines built in `KinematicBrushPass::new`; screen tool names its entry; screen both (and the smooth albedo-before arm). Stop if either leaves the tier. | integrating executor | — | |
| 2 | Split `kinematic_brush.rs` tests into a sibling file, own commit | integrating executor | 1 | |
| 3 | Per-frame selection from the style snapshot, main and mirror; test-only bound-style report; retire the style word | integrating executor | 2 | |
| 4 | Harness and readback tests: smooth composition row, pixel splices, tint rows, A9 source test | integrating executor | 1 | |
| 5 | Frame tests: A5–A8, A10–A14 | integrating executor | 3 | |
| 6 | Bring the kit from `spike/specular-mover-cost` without the hook commit | integrating executor | — | |
| 7 | Preflight, review panel, fixes | integrating executor | 1–6 | |
