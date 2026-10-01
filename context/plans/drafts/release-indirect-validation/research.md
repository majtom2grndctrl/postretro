# release-indirect-validation — research

Read at 832e20c8a. wgpu sources are the 29.0.1 registry crates (`wgpu-types`, `wgpu-core`, `wgpu-hal`).

## wgpu behavior

| Fact | Where |
|---|---|
| `InstanceFlags::default()` is `from_build_config()`. In debug that is `debugging()` (`DEBUG`, `VALIDATION`, `VALIDATION_INDIRECT_CALL`). In release it is `VALIDATION_INDIRECT_CALL` alone. | `wgpu-types` `instance.rs` |
| `InstanceDescriptor::new_without_display_handle()` takes `flags: Default::default()` and reads no env. Only `with_env()` or `*_from_env()` read `WGPU_*`. `InstanceFlags::with_env` reads every `WGPU_*` flag var, not just one. | `wgpu-types` `instance.rs` |
| The device enables indirect validation only if the instance has the bit and the adapter has `INDIRECT_EXECUTION`, `COMPUTE_SHADERS` and at least 2 storage buffers per stage. | `wgpu-core` `device/resource.rs` (`enable_indirect_validation`) |
| With validation on, `multi_draw_indirect` calls `DrawBatcher::add` once per draw: one hash-map entry and one `Vec` push each. Then it issues draws from a validated copy. `inject_validation_pass` runs the GPU check, which turns invalid draws into no-ops silently, with no report. | `wgpu-core` `command/render.rs`, `indirect_validation/draw.rs` |
| Two things run before that branch, so they stay on with validation off. The `IndirectBufferOverrun` check covers the indirect *buffer* range. The buffer zero-init actions (`NeedsInitializedMemory`) still run. | `wgpu-core` `command/render.rs::multi_draw_indirect` |
| Metal `draw_indexed_indirect` loops `drawIndexedPrimitives` once per draw whatever the flag. | `wgpu-hal` `metal/command.rs` |
| D3D12: without the flag's command signatures, an indirect draw takes `prepare_draw(0, 0)`, and an indirect dispatch takes `prepare_dispatch([0; 3])`. A direct `dispatch` still sets `num_workgroups`. | `wgpu-hal` `dx12/command.rs` |
| The flag's doc: behavior is undefined if `first_index + index_count` passes the bound index buffer, or if `first_instance != 0` without `INDIRECT_FIRST_INSTANCE`. The instance range must also fit instance-step vertex buffers. | `wgpu-types` `instance.rs` |

`render_submit` wraps `submit_windowed_frame`, which calls `encoder.finish()`. wgpu-core replays recorded passes there, so the batcher's cost shows under that stage.

## Writers

Indirect-args buffers. Only two renderer buffers carry `BufferUsages::INDIRECT`:
- the camera "Indirect Draw Buffer", created in `ComputeCullPipeline::new`, with one slot per leaf;
- the "Shadow Cull Indirect Buffer", created in `ShadowCullPipeline::new`, with one region per slot (spot instance) or per cube face (cube instance). Each region has one slot per leaf and is padded to a 256-byte stride. Draws never read the padding.

What writes them:

| Writer | Buffer | Values |
|---|---|---|
| wgpu zero-init at creation | both | zero |
| `CandidateCullPipeline::dispatch` `clear_buffer` over the camera range | camera | zero |
| `candidate_cull.wgsl::candidate_cull_main` | camera | the leaf record from `leaves[leaf_idx]` into slot `leaf_idx`, or nothing (the slot stays cleared) |
| `bvh_cull.wgsl::cull_main` via `ComputeCullPipeline::dispatch` (camera tree walk) | camera | the leaf record, or `index_count = 0` on a visited reject |
| `bvh_cull.wgsl::cull_main` via `ShadowCullPipeline::dispatch_occupied_slots_filtered` | both shadow instances | same as the camera tree walk |

`bvh_cull` skips a rejected subtree without touching that subtree's leaf slots, and a shadow region keeps whatever its last light wrote. In both cases the stale slot holds a value written under the same rule, so the per-slot invariant holds by induction from zero-init. The camera tree walk may therefore redraw a stale off-frustum leaf, which clipping makes harmless. That is outside this brief.

What reads them: every indirect draw goes through `draw_indirect_buckets`. Its callers are `ComputeCullPipeline::draw_indirect` (the depth prepass in `renderer_shadow_passes.rs`, forward in `renderer_render_frame.rs`) and `ShadowCullPipeline::draw_slot_indirect` (spot and cube passes in `renderer_dynamic_shadow_passes.rs`). Each binds the whole world index buffer with `index_buffer.slice(..)`. That buffer holds exactly `geometry.indices`, or a 4-byte placeholder when the map has no geometry, and in that case no draw runs.

Pipelines that issue indirect draws: `depth_prepass.wgsl`, `forward.wgsl`, and `spot_shadow.wgsl` (which serves both shadow pools). Their vertex inputs are `@location` attributes only. None binds an instance-step buffer, and only the UI pipelines do, with direct draws.

No `dispatch_workgroups_indirect`, render bundle or `*_indirect_count` call exists. glyphon and egui-wgpu share the device and issue no indirect calls.

## Load check

`validate_bvh_structure` (`prl_loader.rs`) runs inside `load_prl` for every build. It rejects:
- a leaf's `index_offset` or `index_count` that is not a multiple of 3;
- `checked_add` overflow of `index_offset + index_count`;
- an end past the Geometry index count.

The production callers of `load_prl` are the startup worker, capture, observability, and the probe and measurement harnesses. `level_world_to_geometry` hands the validated `indices` and `bvh` to `install_level_geometry` unchanged.

The tests are in `crates/level-loader/src/prl.rs`:
- `load_prl_rejects_bvh_leaf_index_range_past_geometry_indices` sets offset 5 and count 3 against 6 indices. Offset 5 is mid-triangle, so the alignment check rejects it first, and the assertion only matches `section: "Bvh"`. No test isolates the range check.
- `load_prl_rejects_bvh_leaf_index_range_overflow` covers overflow.
- No test names exact-boundary acceptance, though the last leaf of `sample_bvh_section` (offset 3, count 3, against 6 indices) loads in the passing fixtures.

## Build gating

The gate is `debug_assertions` alone (owner, after `/validate-plan`). The `dev-tools` gate in `prl_animated_atlas.rs`, `cfg!(any(debug_assertions, feature = "dev-tools"))`, is not a precedent here: that rule is about strictness, and indirect validation is a silent repair. `renderer_backends_from_env` is the existing opt-in `WGPU_*` diagnostic override, using `Backends::from_env`.

## Measurements carried from the draft session (provisional)

Taken on a Radeon Pro 5300M under Metal, not re-taken here, and **provisional, pending reconciliation** of the validation-only cost. About 1.29 ms/frame on `stress-warren-hallway-inspection` (8,437 leaves) and 0.37 ms/frame on `campaign-test` (774 leaves).

A frame issues L·(2 + uncached shadow slots) indirect draws, so the 3.5× cost ratio against the 10.9× leaf ratio reflects shadow-cache state as well as L. The draft session's pose and cache state are not recorded. The manual row pins map spawn and records cache state.
