# sh-compose-row-cost-spike — research

Derivation behind `index.md`. Source read at de735f026. Nothing was built or run for this note; every hypothesis below comes from reading source.

## Basis

Mac rows: Radeon Pro 5300M, Metal System Trace, labelled pass time divided by camera frames (`gpu_time.py`). Rows come from the run's `[SH streaming]` `compose frame` field. The traces are `measurements/shadow-fill-cost/runs/*-after-*trace*`, and the spike row is `drafts/animated-light-update-rate/research.md` §Spike.

| Pose | Rows (both passes) | Indirect ms | Pass B ms | Indirect µs/row | Pass B µs/row |
|---|---|---|---|---|---|
| hallway arena west end | 2129 | 6.73 | 7.63 | 3.16 | 3.59 |
| kinematic-platform spawn | 2579 | 9.61 | 10.82 | 3.73 | 4.20 |
| kinematic-platform station | 2511 | 9.77 | 10.99 | 3.89 | 4.38 |
| campaign-test spawn (shadow-fill-cost trace) | 730 | 3.11 | 3.56 | 4.26 | 4.88 |
| campaign-test spawn (half-rate spike, 10-01) | 730 | 2.49 | 2.89 | 3.41 | 3.95 |

The GTX 1660 Super read 0.40 and 0.56 ms on campaign-test (owner reading, `POSTRETRO_GPU_TIMING=1`, same research §Perf-floor measurement). At 730 rows that is about 0.55 and 0.77 µs/row. The 5300M is under 2× behind the 1660 Super on raw FP32 throughput and memory bandwidth. A 5–8× per-row gap therefore points to something specific to the backend or the architecture.

What the table shows:
- **µs/row is not one constant.** The same map and rows read 3.41 and 4.26 µs/row in two sessions, about 25% apart. Arms must be interleaved inside one session (index.md Decisions).
- **Maps differ.** µs/row runs 3.2–4.3 on indirect across maps. Per-row work depends on CSR entries per row and on the L0/L1/L2 mix, and neither was recorded. Hence the row-mix finding.
- **The hallway lift pose is unusable as a denominator.** Its rows moved between 198 and 398 within one trace.
- **Today both passes compose the same rows at every pose.** The contributing-row filter makes each pass's rows its own, so after it lands each pass needs its own denominator.

## Post-filter denominator shift

Today a row with no CSR entries still pays the indirection scan, base reads and 36 stores per probe. Whether that is cheap is unmeasured; the floor arm measures it. F1 removes those rows, so a raw µs/row will rise on the rows that remain even though no shader changed. That is why the brief fits a per-row plus per-entry model by brick level instead of comparing µs/row across heads. The model's terms are unaffected by which rows compose, so the spike need not wait for F1, and post-filter savings are projected from the model and the filtered counts (`/validate-plan`, 2026-10-04).

## Hypotheses: source evidence

Both shaders run one 8×8 workgroup per gathered row (one 4×4×4 affinity brick), so each invocation owns one probe. `compose_main` is in `sh_compose.wgsl` and `animated_compose_main` in `animated_direct_sh_compose.wgsl`. `curve_eval.wgsl` is concatenated after each at pipeline build (`sh_streaming/gpu/indirect.rs`, `sh_streaming/direct_compose/passes.rs`).

| Id | Source fact | Lean |
|---|---|---|
| H-a | `var accum: array<vec3<f32>, 36>` (indirect) and `array<vec4<f32>, 36>` (Pass B) live per invocation. They are indexed by `texel_index` inside the init, per-entry and store loops, and the per-entry loop nests inside a runtime-bounded `entry` loop. Unless the compiler unrolls fully, the dynamic index forces private memory. Held in registers they take 108 (indirect) and 144 (Pass B) 32-bit registers before temporaries. | Metal-leaning |
| H-b | `animated_light_scale(affinity_lights[entry])` runs per invocation, per CSR entry, on both the L0 and the L1/L2 paths. Each call reads `animation_descriptor_indices`, a 48-byte descriptor and up to 16 `anim_samples` (4 brightness + 12 colour, `curve_eval.wgsl`), then evaluates Catmull-Rom. All 64 lanes compute the same light's value. Pass B adds term-mask, override and `compose_weights` checks. | All backends |
| H-c | `read_delta_texel` addresses `entry offset + probe_rank × delta_probe_f16_stride + texel × 3 halves`. Adjacent lanes differ by `probe_rank`, 108 halves (216 bytes) apart, so each lane fetches its own sector. An odd half base straddles two words, so each texel costs two loads. | All backends, worse on wide waves |
| H-d | On L1/L2 bricks, at most 8 lanes (L1) or 1 lane (L2) load the shared lattice. Three `workgroupBarrier`s separate each CSR entry. | All backends |
| H-e | wgpu-hal 29's Metal device sets naga's `BoundsCheckPolicy::Restrict` for index and buffer access, and passes `force_loop_bounding` through, unless the module is created with checks off. The only way to turn them off is `Device::create_shader_module_trusted`, which is `unsafe` (wgpu 29.0.1). | Metal-leaning |

Ruled out by reading, with low confidence:
- **Workgroup memory:** about 4.6 KB per workgroup (`shared_kept_tiles` 288 × 16 B, plus 36 B).
- **The `rgba16float` storage writes:** 36 per stored probe, the same count as the output. Not ruled out by measurement: they fall inside the floor arm's fixed share, which the spike measures.
- **Atomics:** neither shader uses any.

## Premise notes

- **Pass B's accumulator alpha is dead.** The init samples a vec4 from `direct_intermediate_atlas`, and the loops carry `prior.a`, but the store writes `select(0.0, 1.0, stored_slot.valid)` for alpha. A vec3 accumulator therefore stores the same bytes by construction, and no separate alpha needs carrying.
- **Pass B's ~13% lead over indirect is weak evidence for H-a.** Pass B also differs in its init read (`textureSampleLevel` on Pass A's intermediate, against the compact base atlas), in its scale (more uniform checks, a compose weight), in having no term-flag gate on its L1/L2 branch, and in a `max` at store.
- **§4's invariant compares streamed with full-resident compose inside one build.** The test that guards it is `sampled_row_gate_capture_matches_full_resident_at_stepped_times` (`crates/postretro/tests/capture_frame.rs`, fixtures in `measurements/sh-probe-streaming/sampled-row-gating/`). A shader change moves both sides equally, so that test cannot detect a lever that changes output. Byte identity with the baseline build is a separate, stricter check.
- **A capture PNG is RGBA8.** A byte-identical capture does not prove byte-identical `rgba16float` atlases. `shadow-fill-cost` M6 used capture PNGs and that was enough there; here the atlas is the product.
- **The CPU curve mirror is tolerance-tested.** `postretro_render_cpu::sh_compose::animated_light_scale` is the CPU source of truth for Pass B's scale, but its tests use `assert_rgb_close`. Scales evaluated on the CPU may differ from in-shader evaluation in the last bits.
- **Reordering loops changes no summation order on L0.** With texels outer and entries inner, each texel still sums base, then entry 0, then entry 1, and so on. Metal's compiler may still contract `accum + delta * scale` into an FMA differently in the new loop shape, so only the byte check settles it.

## Rejected alternative: fuse indirect and Pass B

One dispatch for both passes is blocked on several fronts:
- Each pass already uses all 8 storage bindings (§10). They read different delta sections (27 and 45) with their own offsets, metadata and descriptor maps.
- Their coarsening metadata is separate.
- Pass B reads Pass A's intermediate atlas, so it must follow Pass A, which follows indirect (§7.1 step 5).
- The fused shader would hold both accumulators, which is H-a's pressure doubled.
