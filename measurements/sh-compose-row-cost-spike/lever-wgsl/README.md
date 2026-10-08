# Lever WGSL export: `array-free,unroll36`

Exact source strings handed to `create_shader_module` for the two SH compose passes, from probes commit `1a052cfed` (branch `sh-compose-row-cost-spike-probes`). Each is `compose_spike::build_source(shader, WGSL_DECODE_HELPER, arms)`: the pass shader + `curve_eval.wgsl` + `sh_indirection.wgsl`, joined by newlines, then the arm rewrites.

| File | What | SHA-256 |
|---|---|---|
| `indirect.baseline.wgsl` | `sh_compose.wgsl` → `compose_main`, no arms | `20511b74fe2b21d003da9c6ce22e801cddd8d4f1e87779ff76804da8206e5778` |
| `indirect.array-free+unroll36.wgsl` | same, lever arms | `fa35ae812915120c801d60c1592f5e38891b63b51cb028b52b329d62601b4052` |
| `indirect.diff` | unified diff, baseline → lever | `98d6c7a6342d154ce8f157d0520042a2ce6ae64dd2d98249cd07124ba0eab36e` |
| `direct.baseline.wgsl` | `animated_direct_sh_compose.wgsl` → `animated_compose_main`, no arms | `e29d6ff53babfe8969c44756c4ac29faeae1cfc522715e7872659716bcd3e325` |
| `direct.array-free+unroll36.wgsl` | same, lever arms | `efeae55e602d252115caa5318925317ccffa985b16221932914eb9de46d22dc5` |
| `direct.diff` | unified diff, baseline → lever | `bfcf164c2907b9b14263bc3a488f26f3a4ef7970365a138c5340cfd1426d3d44` |

**How produced.** A temporary `#[ignore]` test (since removed, never committed) called `ordered_arms("array-free,unroll36")`. That resolves to `[array-free, unroll36, scale-shared]`, since array-free implies scale-shared. The test then called `build_source` for each pass with those arms and with none. Every rewrite goes through `replace_n`, which asserts the exact anchor match count and panics on a miss. All four builds completed, so every anchor matched. Each string passed naga 30 `parse_str` plus `Validator` (all flags and capabilities), with entry points `compose_main` and `animated_compose_main` respectively. `coalesced-b` is not part of this lever, so the direct pass keeps the baseline delta layout (no `repack_texel_major` at upload).

**Baseline = landed shaders.** The probes branch changes no `.wgsl` file. The baseline files are the plan-of-record (`e4cfe8a12`) shaders as concatenated at runtime.

**Legacy pipelines share it.** The non-streamed `sh_compose.rs` and `animated_direct_sh_compose.rs` also build through `compose_spike::compose_source` with the same `WGSL_DECODE_HELPER`, so they get the identical source and the same A arms. They do not get the paired-B twin, which only the streamed `ComposePipelines` builds.

**Landing caveat.** The indirect kernel's codegen is fragile on Metal: small source-shape changes have moved its cost. A build must land this exact shape, not a hand-tidied equivalent. It must then re-measure with a paired A/B (baseline vs landed, alternating per frame in one launch) before claiming the win. Port the lever into the shader files themselves; do not keep the rewrite layer.
