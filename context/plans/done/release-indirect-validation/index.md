# release-indirect-validation

Brief · compact · reads: `context/lib/rendering_pipeline.md` §5, §7.1, §12 · `context/lib/development_guide.md` §3.5, §6.4 · `context/lib/build_pipeline.md` §PRL section IDs · read at 832e20c8a

> `/validate-plan`: Direction sound, with amendments the owner decided (2026-10-01). Owner signed off; promoted to `ready/` (2026-10-01).

## Problem
Developer-raised, from CPU profiling on this Mac (Radeon Pro 5300M, Metal). wgpu-core 29.0.1 validates every indirect draw while encoding, and its default instance flags keep that validation on in release builds. The cause is the per-draw `DrawBatcher::add` plus the injected validation pass, both inside `CommandEncoder::finish` under the `render_submit` stage. The depth prepass, the forward pass and each uncached shadow slot each draw one indirect record per BVH leaf. The cost therefore follows shadow-cache state as well as the leaf count; the provisional numbers and the draw-count formula are in `research.md` §Measurements. When this is done, release builds skip that work, and an engine-owned range check plus a writer invariant take over the safety it gave. Debug builds keep wgpu's validation.

## Decisions
- **Release builds clear `InstanceFlags::VALIDATION_INDIRECT_CALL`, and no other bit.** The gate is `debug_assertions` alone. A release build creates its instance without the bit, with or without `dev-tools`, so a `dev-tools` release build matches a player build. A debug build keeps wgpu's build-config default unchanged. wgpu's indirect validation never reports. It silently turns a bad call into a no-op (`indirect_validation/draw.rs`; the `VALIDATION_INDIRECT_CALL` doc), so it is a safety net, not a strictness check. Debug builds keep it so that a cull-shader edit on a developer GPU yields missing geometry, not undefined behavior. The windowed and offscreen-capture instances share one policy, and capture and observability builds follow it. The renderer logs the effective state once, when it creates the instance.
- **`WGPU_VALIDATION_INDIRECT_CALL` overrides the bit in every build (owner decision).** It uses `InstanceFlags::with_env` semantics but is read for this one bit only. Unset, the build policy applies. Set to `0`, it clears the bit, including in debug. Set to any other value, it sets the bit, including in release. This follows the `WGPU_BACKEND` precedent in `renderer_backends_from_env`: a wgpu-named opt-in diagnostic that is absent by default. `development_guide.md` §6.4's `POSTRETRO_*` rule governs instrumentation, and this variable is a wgpu safety toggle, so the rule does not apply.
- **Two range checks, at load and at install.** `validate_bvh_structure` already rejects, in every build, a leaf whose `index_offset + index_count` overflows or passes the Geometry (id 17) index count, as a hard `PrlLoadError::SectionValidation { section: "Bvh" }`. Today every install goes through `load_prl`, but `LevelGeometry` and `install_level_geometry` are public, so nothing structurally forces that. The install check is an O(leaves) range check, in every build, against the `indices` about to be uploaded. It runs up front, before the first install step, so a rejected level installs nothing and no unwind is needed. The caller handles the error as a failed level load. `install_level_geometry` keeps the same check as a debug assertion for direct callers. Both checks reject, per `research.md` §Load check.
- **Invariant the cleared bit relies on.** It has four parts:
  1. Each indirect slot holds zero, or its own leaf's baked record (`index_count`, 1, `index_offset`, 0, 0) with any field zeroed by a reject. No shader change is needed. No writer computes these values, and no PRL field reaches `instance_count`, `first_instance` or `base_vertex`: the writers hard-code 1, 0 and 0, and the device never requests `INDIRECT_FIRST_INSTANCE` (`request_renderer_device`). A skipped slot keeps an earlier value written under the same rule. The writers are in `research.md` §Writers.
  2. Every leaf range passed both checks above.
  3. The index buffer each indirect draw binds is exactly the checked Geometry index array: `install_level_geometry` uploads `geometry.indices` as "World Index Buffer", and the passes bind `index_buffer.slice(..)`.
  4. No indirect-drawn shader reads `vertex_index` or `instance_index`, and the engine issues no indirect dispatch. The ban stays as defense in depth: on D3D12 only `num_workgroups` is affected by the cleared bit (`research.md` §wgpu behavior).

  **Revisit this flag before landing any of these:**
  - an indirect draw or dispatch that reads those built-ins;
  - GPU compaction of indirect records;
  - computed (non-baked) indirect arguments;
  - any change to the leaf-to-index-buffer mapping, such as geometry/BVH residency (`plans/large-map-spatial-residency.md` stage 5's generalization) or `bvh-leaf-clustering`.
- **§3.5 consultation.** Clearing the bit uses a safe API (the `flags` field of `InstanceDescriptor`), and no `unsafe` block appears. It does opt into wgpu's documented undefined behavior when the invariant breaks. The owner's sign-off on this brief counts as the `development_guide.md` §3.5 consultation, and the invariant above is the agreed mitigation.
- **Landing order (owner).** Upload batching (`context/plans/ready/per-frame-upload-batching/`) lands first, then this brief, then `visible-span-draws` (`context/plans/drafts/visible-span-draws/`), then `bvh-leaf-clustering`. This brief's manual proof measures with upload batching already landed.
- **Non-goals.** Draw count belongs to `visible-span-draws`. The two stack, because wgpu-hal Metal's `draw_indexed_indirect` still calls `drawIndexedPrimitives` once per draw. No other wgpu validation flag changes, so `InstanceFlags::with_env` is not adopted: it would also honor `WGPU_VALIDATION`, `WGPU_DEBUG` and `WGPU_GPU_BASED_VALIDATION`. The wgpu version stays as is, because no released wgpu changes the per-draw Metal loop.

## Acceptance
### Automated
#### Instance flags
- [x] With the variable unset, a release build has the indirect-call bit cleared, both with and without `dev-tools`. A source scan finds no cargo feature in the gate that selects the build input.
- [x] With the variable unset, a debug build has the bit set, both with and without `dev-tools`.
- [x] A release build with the variable set to `1` has the bit set.
- [x] A debug build with the variable set to `0` has the bit cleared.
- [x] Set to an empty value or to false, the variable sets the bit. Only 0 clears it.
- [x] Whatever the variable's value, every other bit equals wgpu's build-config default. Setting `WGPU_VALIDATION`, `WGPU_DEBUG` or `WGPU_GPU_BASED_VALIDATION` changes nothing. The env half is a source scan, since a test cannot set environment variables without unsafe: the instance-flag path reads no environment variable except `WGPU_VALIDATION_INDIRECT_CALL`.
- [x] The windowed and offscreen renderers both build their flags through the policy. A source scan finds no non-test renderer code creating an instance any other way. No other code reads `WGPU_VALIDATION_INDIRECT_CALL` (P3).
#### Range checks
- [x] At load, a map whose one leaf is triangle-aligned and ends one triangle past the Geometry index count fails with the Bvh range error, not the triangle-alignment error. Regression: the existing past-end test uses offset 5, so the alignment check rejects it first and the range check is never isolated.
- [x] At load, a map whose last leaf ends exactly at the Geometry index count loads.
- [x] Regression guard (passes today): at load, a leaf whose `index_offset + index_count` overflows `u32` fails.
- [x] A level whose geometry fails the install range check ends as a failed load: the frontend on a runtime load, an error exit on the boot load. The check runs before the first install step, so nothing of the rejected level is installed: no texture, geometry, navigation or network level state (P1).
- [x] At install, geometry with a leaf ending one triangle past `indices` is rejected before any upload. The renderer keeps no partially installed level. The rejection is a test of the pure check. "Before any upload" and "no partially installed level" are an on-demand GPU test through the offscreen renderer: after a rejected install, the renderer still holds the state it held before the install began.
- [x] At install, geometry whose last leaf ends exactly at `indices.len()` is accepted. A leaf whose range overflows `u32` is rejected.
- [x] At install, geometry with zero leaves is accepted (P5).
- [x] At load and at install, a leaf with no indices whose offset equals the index count is accepted, and one whose offset passes it is rejected (P6).
#### Writers
- [x] A source scan fails in each of three cases. Case one: a renderer buffer gains `INDIRECT` usage outside the two cull owners. Case two: a shader other than `bvh_cull.wgsl` and `candidate_cull.wgsl` binds the indirect-args array. Case three: a store to that array writes anything other than the leaf's `index_count`, the leaf's `index_offset`, 0 or 1, or indexes the array by something other than the same leaf index the leaf record was read from (P4).
- [x] A source scan shows that the indirect-drawn vertex shaders read neither `vertex_index` nor `instance_index`, and that no renderer code issues an indirect dispatch. The scan reads the full source each indirect-drawing pipeline compiles, including every composed module, not only its entry file.
- [x] A source scan shows that the indirect-args buffers are created only in the level install that also creates the world index buffer, so no slot outlives the index buffer it was written for (P2).
- [x] A source scan shows that every indirect world draw binds the whole world index buffer, and that only level install creates that buffer (P7).

These scans match statements, not meaning. A shadowed `leaf` binding would pass them. The writer comment carries that part as a review rule.
### Manual
- [x] On this Mac, measured with upload batching already landed. Conditions: `--release` without `dev-tools`, `POSTRETRO_CPU_TIMING=1`, map spawn, the window in front, idle VRAM recorded per `rendering_pipeline.md` §12, and shadow-cache state recorded (warm or cold, and the uncached slot count). On each map, the median of the per-window `render_submit` averages over at least 5 windows drops by at least half the reconciled validation-only cost. The thresholds are set from the reconciliation numbers; provisionally they are ≥0.65 ms on `stress-warren-hallway-inspection` and ≥0.18 ms on `campaign-test`. The baseline is the same binary with `WGPU_VALIDATION_INDIRECT_CALL=1`, run back to back under the same recorded machine and shadow-cache state.
- [x] On both maps, a `sample` profile of that release build shows no `DrawBatcher::add` and no `inject_validation_pass` frames. A debug build of the same commit still shows both. The release build keeps its symbols (`CARGO_PROFILE_RELEASE_STRIP=none`), and the same binary run with `WGPU_VALIDATION_INDIRECT_CALL=1` shows both frames.
- [x] The startup log reports indirect-call validation off in a release build and a `dev-tools` release build, and on in a debug build. With the variable set, the line says the override set the state, and the line appears once per run.
- [x] On both maps, at spawn and over a short walk, the release build looks the same as it did before the change.

## Path
- Seams: `renderer_backends` and `renderer_backends_from_env` in `renderer_init.rs` set the shape. Add a pure policy function beside them that takes `debug_assertions` and the override value. Read only `WGPU_VALIDATION_INDIRECT_CALL`, never `InstanceFlags::with_env`, and add a thin `cfg!` wrapper. The startup log line says whether the override set the state. `Renderer::new` and `Renderer::new_offscreen` both set `flags` from it. GPU test harnesses keep wgpu defaults.
- Install check: make it a pure function over `(bvh leaves, index count)`, so it is testable without a GPU (`testing_guide.md` §No GPU context in tests). Call it from the callers in `startup/lifecycle.rs` and `capture/prepared.rs` before their first install step, and repeat it as a `debug_assert!` in `install_level_geometry`. The empty-geometry reset in `renderer_resources.rs` cannot fail. Both doc comments name the release flag as what depends on them. The exact error type is delegated.
- Pin the load check with tests beside `load_prl_rejects_bvh_leaf_index_range_past_geometry_indices` in `crates/level-loader/src/prl.rs`. The last leaf of `sample_bvh_section` already ends at the 6-index boundary. Assert on the message.
- Put the invariant in a comment at each shader's `indirect_draws` binding and at the index-buffer upload.
- Writer scans extend the shader-text tests in `compute_cull.rs` (`is_aabb_outside_frustum_is_identical_across_shaders`). Precedent for scanning source text: `cache_source_does_not_request_copy_source_usage` in `promoted_depth_cache.rs`.
- Chosen: clear the bit in release only, with load and install range checks. Rivals considered:
  - **Clear the bit in every build.** Rejected narrowly: the silent no-op repair protects developer GPUs while the cull shaders are being edited, and debug builds don't need the frame time.
  - **Install-time check.** Adopted (Decisions).
- First slice: clear the bit in a local release build and take the hallway `render_submit` numbers, recording shadow-cache state. That confirms the cost premise before any tests are written.
- No file this touches is past ~800 non-test lines. `compute_cull.rs` reaches line 793 before its test module.
- Doc update at promotion: add the build-gated default and the variable to `rendering_pipeline.md` §12 Diagnostics, beside `POSTRETRO_GPU_TIMING` and `POSTRETRO_CPU_TIMING`. Put the invariant in §7.1. `context/lib` has no central env-var list, and `WGPU_BACKEND` is documented only in code. Player-facing `docs/` does not change.

## Open questions
None.
