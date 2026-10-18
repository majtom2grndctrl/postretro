# Fluid reflection-free variant by entry point — contract

## Goal

The reflection-free fluid pipeline costs about 0.30 ms more than the pre-reflection fluid shader on the Radeon Pro 5300M (Metal), on a level with no reflection probes. The variant is chosen by the `FLUID_REFLECTIONS` override constant. wgpu's Metal backend substitutes overrides without removing dead branches, and naga's MSL writer declares one entry-point argument for every global the entry point's function tree references, including those only reachable in a constant-folded branch. So the reflection-free `fs_main` declares four arguments it never reads: `anim_descriptors`, `fluid_probe_array`, `fluid_probes` and `fluid_probe_distance`. The AMD Metal driver charges per declared argument. A standalone Metal benchmark of naga's real output reproduced +0.32 ms and recovered 0.25 ms of it with the four arguments removed. This change selects the variant by entry point instead, so naga's per-entry compaction leaves those globals out.

## Decisions

- **Variant = entry point.** `fs_main` is the reflection-free fragment entry; `fs_main_reflecting` is the reflecting one. The `FLUID_REFLECTIONS` override, `FLUID_REFLECTIONS_OVERRIDE` and `fluid_variant_constants` are deleted, with no compatibility shim. Consequence: the reflection-free entry's `global_uses` excludes the four reflection globals at the naga level, before any backend runs, and that is now testable.
- **Shared shading in one function.** `fluid_shade(in: VertexOutput) -> FluidShaded` holds everything up to and including the fog composite. `fs_main` returns its `.surface`. `fs_main_reflecting` computes `dpdx/dpdy(in.world_position)` first, in uniform control flow, then calls `fluid_shade`, then runs the gate and the reflection tail unchanged. The world derivatives move out of the shared prefix, so the reflection-free entry no longer computes them.
- **One layout, unchanged.** Same pipeline layout, same group-4 bind group layout and bind groups, same vertex buffers (the probe-index buffer and flat varying stay; measured cost zero). The reflection-free pipeline still binds group 4; binding a resource the shader does not declare measured no cost. No new pipeline, no persistent GPU state, no BGL change.
- **render-cpu stays GPU-free.** `FluidShaderVariant` keeps its two variants and the per-draw selection rule. The entry-point name is shader knowledge and lives in the renderer (`fluid_surface.rs`), mapped from the variant there. `reflections()` keeps its meaning; its doc comment stops naming the override.
- **The reflecting variant's cost is out of scope.** It is unchanged by this fix (benchmark 7.077 vs 7.076 ms). The ~0.07 ms residual left after the fix is unexplained and not chased here.

## Invariants

- Pixel parity: wherever the reflecting gate is shut, `fs_main_reflecting` returns exactly what `fs_main` returns, because both return `fluid_shade(in).surface` there. The reflection tail's statements stay as they are; only their inputs come from the `FluidShaded` fields.
- The CPU references in render-cpu mirror the shader statement for statement, and `fluid_surface_tests.rs` pins both texts. Every existing pin is retargeted to where its text now lives (`fluid_shade` or `fs_main_reflecting`), never deleted or loosened. A pin that read "in `fs_main`" now reads in whichever function owns that text.
- Derivatives (`dpdx`/`dpdy`) are called only in uniform control flow: at the top of `fluid_shade`, and at the top of `fs_main_reflecting` before its first branch.
- Both entry points compile under the existing DX12 FXC test and pass naga validation.
- WGSL comments match `context/lib/context_style_guide.md`; keep the comments that the prototype dropped (the gate's explanation and the fog-band note) on the reflection tail.

## Tracks and ownership

One implementation track, then one review track.

- **Implementation:** `crates/renderer/src/shaders/fluid_surface.wgsl`, `crates/renderer/src/render/fluid_surface.rs`, `crates/renderer/src/render/fluid_surface_tests.rs` (and `fluid_surface_frame_test.rs` only if it names the override or entry point), `crates/render-cpu/src/fluid_surface/slots.rs` (doc comment only), `context/lib/rendering_pipeline.md` (the **Two pipelines** bullet).
- The orchestrator owns this contract, merges, commits and the traced capture.

## Acceptance

- `grep -rn "FLUID_REFLECTIONS\|fluid_variant_constants" crates/ context/lib/` returns nothing.
- A new test asserts that the `fs_main` entry's `global_uses` (from `naga::valid::ModuleInfo::get_entry_point`) is zero for `anim_descriptors`, `fluid_probe_array`, `fluid_probes` and `fluid_probe_distance`, and nonzero for each in `fs_main_reflecting`. Run against the old shader, this test fails.
- `cargo test -p postretro-renderer --lib fluid` passes, with the test count reported and no fewer tests than on `main`, minus the deleted override test.
- `cargo test -p postretro-render-cpu --lib fluid` passes, with the count reported.
- `cargo clippy -p postretro-renderer --all-targets -- -D warnings` is clean.
- Traced 720p capture (orchestrator): fix binary vs `b3322a2ef` on the no-probe e24 PRL at the pool pose. The fix's fluid time is within about 0.1 ms of `b3322a2ef`, where the pre-fix binary read +0.30 ms.
- Reflecting-variant parity (orchestrator): a capture of the probe-bearing e24 PRL from the fix binary matches the pre-fix binary's.

## Open questions

None.
