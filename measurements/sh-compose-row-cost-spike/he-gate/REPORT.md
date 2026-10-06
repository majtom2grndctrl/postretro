# Gate H-e (static MSL check count), naga 30.0.1, wgpu-hal 30.0.1

Verdict: **checks inside per-entry/per-texel loops: yes.** Both shaders.

## Options used (src/main.rs)
Mirrors wgpu-hal-30.0.1 `metal/device.rs::load_shader`:
- `bounds_check_policies`: `index = buffer = image_load = Restrict` (checked) / `Unchecked` (unchecked); `binding_array = Unchecked` (as hal).
- `force_loop_bounding`: `true` / `false`. `emit_int_div_checks`, `mesh_shader_primitive_indices_clamp`, `ray_query_initialization_tracking`: `true` / `false`, i.e. `ShaderRuntimeChecks::checked()` (the `Default`, used by `create_shader_module`) vs `unchecked()` (all five fields false).
- `zero_initialize_workgroup_memory: true` (wgpu default, same in both), `spirv_cross_compatibility=false`, `fake_missing_bindings=false`, `task_dispatch_limits=None`.
- `PipelineOptions`: entry point Compute, `vertex_pulling_transform: true`, empty vertex mappings.
- Lang version (4,0). hal picks 4.0 on macOS 26, 3.2 on macOS 15, 3.1 on macOS 14. Check insertion does not depend on it.
- Validator capabilities: default plus `SHADER_FLOAT16_IN_FLOAT32` (needed by `unpack2x16float`; wgpu enables it on Metal).
- Source: shader + "\n" + curve_eval.wgsl + "\n" + sh_indirection.wgsl (`WGSL_DECODE_HELPER = include_str!("../shaders/sh_indirection.wgsl")`, sh_indirection.rs:28).

Approximated: the binding map. Slots are assigned sequentially per class (buffer/texture/sampler) from the module's globals, and `sizes_buffer = Some(next buffer slot)`. hal derives them from the pipeline layout. This changes slot numbers only, not check insertion. `_buffer_sizes` is present, as in hal.
One behavioural caveat: the `unchecked()` flags set `int_div_checks=false` too, so the diff includes `naga_div`/`naga_mod` select-guards (not bounds checks).

Files: `{sh_compose,animated_direct}_{checked,unchecked}.metal`, `*.diff`, `analyze.py` and `analysis.txt` (per-function/loop breakdown).
Unchecked MSL has 0 `loop_bound`, 0 `metal::min(unsigned(`, 0 `naga_div`/`naga_mod`. No `image_load` guards exist: the only image ops are a sampled read and a storage `write`, and Restrict-on-load has nothing to guard.

## Per-shader table (checked count; unchecked = 0 for every row)
The two shaders have identical structure and per-loop counts unless noted. "Pass B" = animated_direct (accum is `float4[36]`); "indirect" = sh_compose (accum is `float3[36]`).

| Loop (source) | check kind | count (checked) |
|---|---|---|
| kernel prologue (not a loop) | const clamps `row_ids`/`packed_rows`; buffer clamps `affinity_offsets` x2, `probe_indirection` x2; 7 int-div guards | 4 + 4 + 7 |
| loop_1: lane-0 64-candidate indirection scan | loop-bound counter; buffer clamp `probe_indirection` | 1 + 1 |
| loop_2: per-texel init (36) | loop counter; **const clamp `accum[min(i,35)]`** x2; int-div/mod guard x2; sampled base read | 1 + 2 + 2 |
| loop_3: L0 per-CSR-entry | loop counter; buffer clamp `affinity_lights`; callee `animated_light_scale` (see below) | 1 + 1 |
| loop_4: L0 inner per-texel (36) | loop counter; **const clamp `accum` x2 (read + write)**; div/mod guard x2; callee `read_delta_texel`: **buffer clamp `delta_subblocks` x2**, `delta_compaction_meta` x1 (via `entry_delta_f16_offset`), div guard x2 | 1 + 2 + 2 + (3 + 2) |
| loop_5: L1/L2 per-entry | loop counter; const clamp `shared_kept_present` x2; buffer clamp `affinity_lights`; callee `animated_light_scale` | 1 + 2 + 1 |
| loop_6: L1 per-texel load into `shared_kept_tiles` | loop counter; const clamp `shared_kept_tiles[min(..,287)]`; div guard x2; callee `read_delta_texel` (3 clamps + 2 div) | 1 + 1 + 2 + (3 + 2) |
| loop_7: L2 per-texel load | same as loop_6 | 1 + 1 + 2 + (3 + 2) |
| loop_8: L1/L2 per-texel accumulate | loop counter; const clamp `accum` x2; `shared_kept_tiles` x1 (L2 branch) | 1 + 2 + 1 |
| `reconstruct_l1_shared_texel` 8-slot loop (inside loop_8, L1 branch) | loop counter; const clamp `shared_kept_present[min(slot,7)]`; `shared_kept_tiles[min(..,287)]` | 1 + 1 + 1 |
| loop_9: store loop (36) | loop counter; const clamp `accum` x1; div guard x2 | 1 + 1 + 2 |
| `animated_light_scale` -> curve eval (per entry) | buffer clamps: `animation_descriptor_indices` 1, `descriptors` 1, `anim_samples` 20 in the file (15 in `sample_color_catmull_rom`, 5 in `sample_curve_catmull_rom`; a given call runs a subset); `naga_mod` guards in the catmull index wrap; Pass B also `compose_weights[min(..,63)]` and `[min(..,3)]` | n/a |

Totals (checked MSL): 10 loops each get one loop counter (30 lines). `sh_compose`: 16 const-index clamps + 35 buffer-length clamps + 52 `naga_div`/`naga_mod` call sites. `animated_direct`: 18 + 35 + 51. All are absent in unchecked.

## Excerpts (sh_compose_checked.metal; Pass B is the same shape)
Loop counter (loop_4, L0 inner per-texel):
```
uint2 loop_bound_4 = uint2(4294967295u);
while(true) {
    if (metal::all(loop_bound_4 == uint2(0u))) { break; }
    loop_bound_4 -= uint2(loop_bound_4.y == 0u, 1u);
```
Private accum clamps and delta read:
```
metal::float3 _e283 = accum.inner[metal::min(unsigned(_e281), 35u)];
metal::float4 _e285 = read_delta_texel(_e284, _e258, tile_texel_3, grid, delta_subblocks, delta_compaction_meta, _buffer_sizes);
accum.inner[metal::min(unsigned(_e279), 35u)] = _e283 + (_e285.xyz * _e266);
```
`read_delta_texel`:
```
uint _e33 = delta_subblocks[metal::min(unsigned(word_base), (_buffer_sizes.size6 - 0 - 4) / 4)];
uint _e39 = delta_subblocks[metal::min(unsigned(word_base + 1u), (_buffer_sizes.size6 - 0 - 4) / 4)];
```
Per-entry:
`affinity_lights[metal::min(unsigned(_e263), (_buffer_sizes.size10 - 0 - 4) / 4)]`

## Direct answers
- Is `accum` indexed through a clamp inside the per-entry/per-texel loops? **Yes.** Every access is `accum.inner[metal::min(unsigned(i), 35u)]`: init, L0 accumulate (read and write), L1/L2 accumulate, store. The index is the loop counter, bounded `< TILE_TEXEL_COUNT` by the loop condition, but naga cannot prove `TILE_TEXEL_COUNT <= 36` (it is a runtime-visible constant in the MSL).
  - The clamp is a cheap `min`, but it sits on the indexed private-array access. Whether Metal's compiler still promotes the array to registers is a downstream question this static gate cannot answer.
- Loop-bounding counters on those loops? **Yes.** All 10 loops, including the two hottest (per-entry and per-texel) and the 8-slot reconstruction loop. The counter is a `uint2` (64-bit emulation) decremented every iteration, plus a `metal::all(...)` break test. The loops are `while(true)` with a break on the real condition, so a counter-carrying form is what the Metal compiler sees. Fixed trip counts (`< TILE_TEXEL_COUNT`) are not exempt.
- Are `delta_subblocks` reads guarded by buffer-size checks? **Yes.** Both words in `read_delta_texel` are clamped against `_buffer_sizes.size6`, as is the `delta_compaction_meta` read in `entry_delta_f16_offset` (`size13`). `affinity_lights` (size10), `anim_samples` (size9), `descriptors`, `probe_indirection` and `affinity_offsets` are clamped too. They are `min` clamps, not branches, so there is no divergent control flow.
- Also present, but not bounds checks: select-guarded integer div/mod (`naga_div`/`naga_mod`) on non-constant-folded divisors, including inside every per-texel iteration (`tile_texel` from `texel_index`, and `read_delta_texel`). These come from `int_div_checks`, which `unchecked()` also disables.

**Verdict line: checks inside per-entry/per-texel loops: yes.** The trusted arm is warranted by the brief's gate.
