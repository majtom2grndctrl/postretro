// Spike-only probes for `sh-compose-row-cost-spike` (throwaway branch; never lands).
// See: context/plans/in-progress/sh-compose-row-cost-spike/index.md
//
// `POSTRETRO_SPIKE_ARMS` (comma list) selects WGSL rewrites of the two SH compose
// shaders at pipeline creation. Every rewrite is anchored on exact source text
// and panics when its anchor does not match the expected count: an arm that
// silently failed to apply would corrupt the experiment.
//
// Arm kinds (the findings note carries the same typing):
// - floor / ablation: timing-only, never offered as a lever.
// - lever: intended byte-identical to the baseline; proven by atlas dump.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ComposeShader {
    Indirect,
    AnimatedDirect,
}

/// Application order is fixed (structural levers first, then ablations) so a
/// stacked arm list composes the same way whatever order the env var names.
const KNOWN_ARMS: &[&str] = &[
    // levers
    "scan-parallel",
    "texel-outer",
    "array-free",
    "unroll36",
    "l0-only",
    "scale-shared",
    "vec3-accum",
    "const-tile",
    // floor and ablations
    "floor",
    "no-base",
    "stores-off",
    "accum-scalar",
    "const-scale",
    "rank0",
    "skip-coarse",
    "single-slot",
];

pub(crate) fn arms() -> &'static [&'static str] {
    static ARMS: OnceLock<Vec<&'static str>> = OnceLock::new();
    ARMS.get_or_init(|| {
        let raw = std::env::var("POSTRETRO_SPIKE_ARMS").unwrap_or_default();
        let selected = ordered_arms(&raw);
        log::info!(
            "[SH spike] compose arms: {}",
            if selected.is_empty() {
                "baseline".to_string()
            } else {
                selected.join(",")
            }
        );
        selected
    })
}

/// The paired B arm list (`POSTRETRO_SPIKE_ARMS_B`), when set. Its pipelines
/// alternate with the A pipelines frame by frame inside one launch, so both
/// arms share memory placement, clocks and thermal state.
pub(crate) fn arms_b() -> Option<&'static [&'static str]> {
    static ARMS_B: OnceLock<Option<Vec<&'static str>>> = OnceLock::new();
    ARMS_B
        .get_or_init(|| {
            let raw = std::env::var("POSTRETRO_SPIKE_ARMS_B").ok()?;
            let selected = ordered_arms(&raw);
            log::info!(
                "[SH spike] paired B compose arms: {}",
                if selected.is_empty() {
                    "baseline".to_string()
                } else {
                    selected.join(",")
                }
            );
            Some(selected)
        })
        .as_deref()
}

/// A compose pipeline plus its optional paired-B twin. `next` alternates
/// A, B, A, B per dispatch call when B exists.
pub(crate) struct ComposePipelines {
    a: wgpu::ComputePipeline,
    b: Option<wgpu::ComputePipeline>,
    calls: u64,
}

impl ComposePipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        shader: ComposeShader,
        decode_helper: &str,
        label: &str,
        layout: &wgpu::PipelineLayout,
        entry_point: &str,
    ) -> Self {
        let build = |arms: &[&str], suffix: &str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&format!("{label} Shader{suffix}")),
                source: wgpu::ShaderSource::Wgsl(build_source(shader, decode_helper, arms).into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(&format!("{label} Pipeline{suffix}")),
                layout: Some(layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        Self {
            a: build(arms(), ""),
            b: arms_b().map(|arms| build(arms, " [B]")),
            calls: 0,
        }
    }

    /// The pipeline for this dispatch and whether it is the paired B arm.
    pub(crate) fn next(&mut self) -> (&wgpu::ComputePipeline, bool) {
        let use_b = self.b.is_some() && self.calls % 2 == 1;
        self.calls += 1;
        match (&self.b, use_b) {
            (Some(b), true) => (b, true),
            _ => (&self.a, false),
        }
    }
}

fn ordered_arms(raw: &str) -> Vec<&'static str> {
    let mut requested: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|arm| !arm.is_empty() && *arm != "baseline")
        .collect();
    for arm in &requested {
        assert!(
            KNOWN_ARMS.contains(arm),
            "POSTRETRO_SPIKE_ARMS names unknown arm `{arm}`"
        );
    }
    assert!(
        !(requested.contains(&"texel-outer") && requested.contains(&"array-free")),
        "texel-outer and array-free are alternative restructures"
    );
    for modifier in ["unroll36", "l0-only"] {
        assert!(
            !requested.contains(&modifier) || requested.contains(&"array-free"),
            "`{modifier}` modifies array-free"
        );
    }
    // texel-outer and array-free read per-entry scales from the shared cache.
    if requested.contains(&"texel-outer") || requested.contains(&"array-free") {
        requested.push("scale-shared");
    }
    KNOWN_ARMS
        .iter()
        .copied()
        .filter(|arm| requested.contains(arm))
        .collect()
}

/// Build one compose shader's full source (shader + curve helpers + decode
/// helper) and apply the selected arms.
pub(crate) fn compose_source(shader: ComposeShader, decode_helper: &str) -> String {
    build_source(shader, decode_helper, arms())
}

fn build_source(shader: ComposeShader, decode_helper: &str, arms: &[&str]) -> String {
    let body = match shader {
        ComposeShader::Indirect => include_str!("../shaders/sh_compose.wgsl"),
        ComposeShader::AnimatedDirect => include_str!("../shaders/animated_direct_sh_compose.wgsl"),
    };
    let mut src = [
        body,
        "\n",
        include_str!("../shaders/curve_eval.wgsl"),
        "\n",
        decode_helper,
    ]
    .concat();
    for arm in arms {
        // vec3-accum targets Pass B; the indirect accumulator already is vec3.
        if *arm == "vec3-accum" && shader == ComposeShader::Indirect {
            continue;
        }
        apply_arm(&mut src, shader, arm, arms);
    }
    src
}

fn replace_n(src: &mut String, from: &str, to: &str, expected: usize, arm: &str) {
    let found = src.matches(from).count();
    assert_eq!(
        found, expected,
        "spike arm `{arm}`: anchor matched {found} times, expected {expected}: {from:?}"
    );
    *src = src.replace(from, to);
}

fn replace_once(src: &mut String, from: &str, to: &str, arm: &str) {
    replace_n(src, from, to, 1, arm);
}

const LET_END: &str = "    let end = affinity_offsets[offset_index + 1u];\n";
const LEVEL_BLOCK: &str = "    let level = cell_level(cell_index);
    let offset_index = cell_index * 2u;
    let start = affinity_offsets[offset_index];
    let end = affinity_offsets[offset_index + 1u];
";
const ACCUM_COMMENT: &str =
    "    // Keeping the accumulator private lets one shared kept lattice serve all\n";
const SHARED_DECL: &str = "var<workgroup> shared_brick_indirection: u32;\n";

fn apply_arm(src: &mut String, shader: ComposeShader, arm: &str, all: &[&str]) {
    use ComposeShader::{AnimatedDirect, Indirect};
    match arm {
        "scan-parallel" => {
            replace_once(
                src,
                SHARED_DECL,
                "var<workgroup> shared_brick_indirection: u32;
var<workgroup> spike_first_valid: atomic<u32>;
var<workgroup> spike_words: array<u32, 64>;
",
                arm,
            );
            replace_once(
                src,
                "    if (local_probe == 0u) {
        shared_brick_indirection = 0u;
        for (var candidate_local = 0u;",
                "    // spike scan-parallel: the scan takes the first valid candidate in
    // local order, which is the lowest local index whose word decodes valid.
    if (local_probe == 0u) {
        atomicStore(&spike_first_valid, 64u);
    }
    workgroupBarrier();
    var spike_word = 0u;
    if (in_grid) {
        spike_word = probe_indirection[probe_index];
    }
    spike_words[local_probe] = spike_word;
    if (decode_sh_probe_indirection(spike_word).valid) {
        atomicMin(&spike_first_valid, local_probe);
    }
    workgroupBarrier();
    if (local_probe == 0u) {
        let spike_first = atomicLoad(&spike_first_valid);
        shared_brick_indirection = select(0u, spike_words[min(spike_first, 63u)], spike_first < 64u);
    }
    if (false) {
        shared_brick_indirection = 0u;
        for (var candidate_local = 0u;",
                arm,
            );
        }
        "texel-outer" => {
            replace_once(src, LEVEL_BLOCK, "", arm);
            let fused = match shader {
                Indirect => {
                    "    // spike texel-outer: L0 rows fuse base read, entry sum and store per
    // texel, so their accumulator is one vec3 instead of a 36-entry array.
    if (level == 0u) {
        if (output_is_stored) {
            let spike_animated = use_indirect_animated && local_probe_is_kept(cell_index, local_probe);
            let spike_rank = within_cell_rank(cell_index, local_probe);
            for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                let tile_texel = vec2<u32>(
                    texel_index % RUNTIME_TILE_DIMENSION,
                    texel_index / RUNTIME_TILE_DIMENSION,
                );
                var spike_accum = vec3<f32>(0.0);
                if (use_indirect_static) {
                    spike_accum = sample_compact_base_atlas(stored_slot.slot, tile_texel).rgb;
                }
                if (spike_animated) {
                    for (var entry = start; entry < end; entry = entry + 1u) {
                        spike_accum = spike_accum
                            + read_delta_texel(entry, spike_rank, tile_texel).rgb * spike_scale(entry, start, spike_cached);
                    }
                }
                textureStore(
                    sh_total_atlas,
                    vec2<i32>(tile_origin.xy + tile_texel),
                    i32(tile_origin.z),
                    vec4<f32>(spike_accum, select(0.0, 1.0, stored_slot.valid)),
                );
            }
        }
        return;
    }
"
                }
                AnimatedDirect => {
                    "    // spike texel-outer: L0 rows fuse intermediate read, entry sum and store
    // per texel, so their accumulator is one vec3 instead of a 36-entry array.
    if (level == 0u) {
        if (output_is_stored) {
            let spike_animated = local_probe_is_kept(cell_index, local_probe);
            let spike_rank = within_cell_rank(cell_index, local_probe);
            for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
                let tile_texel = vec2<u32>(
                    texel_index % RUNTIME_TILE_DIMENSION,
                    texel_index / RUNTIME_TILE_DIMENSION,
                );
                let atlas_texel = tile_origin.xy + tile_texel;
                let uv = (vec2<f32>(atlas_texel) + 0.5)
                    / vec2<f32>(textureDimensions(direct_intermediate_atlas));
                var spike_accum = textureSampleLevel(
                    direct_intermediate_atlas,
                    intermediate_sampler,
                    uv,
                    i32(tile_origin.z),
                    0.0,
                ).rgb;
                if (spike_animated) {
                    for (var entry = start; entry < end; entry = entry + 1u) {
                        spike_accum = spike_accum
                            + read_delta_texel(entry, spike_rank, tile_texel).rgb * spike_scale(entry, start, spike_cached);
                    }
                }
                textureStore(
                    direct_composed_atlas,
                    vec2<i32>(tile_origin.xy + tile_texel),
                    i32(tile_origin.z),
                    vec4<f32>(
                        max(spike_accum, vec3<f32>(0.0)),
                        select(0.0, 1.0, stored_slot.valid),
                    ),
                );
            }
        }
        return;
    }
"
                }
            };
            replace_once(
                src,
                ACCUM_COMMENT,
                &format!("{LEVEL_BLOCK}{fused}{ACCUM_COMMENT}"),
                arm,
            );
        }
        // Modifiers of array-free; applied inside its rewrite.
        "unroll36" | "l0-only" => {}
        "array-free" => {
            let unroll = all.contains(&"unroll36");
            let l0_only = all.contains(&"l0-only");
            replace_once(src, LEVEL_BLOCK, "", arm);
            let (gate_l0, gate_coarse, base_init, store) = match shader {
                Indirect => (
                    "use_indirect_animated && local_probe_is_kept(cell_index, local_probe)",
                    "use_indirect_animated",
                    "                var spike_accum = vec3<f32>(0.0);
                if (use_indirect_static) {
                    spike_accum = sample_compact_base_atlas(stored_slot.slot, tile_texel).rgb;
                }
",
                    "                textureStore(
                    sh_total_atlas,
                    vec2<i32>(tile_origin.xy + tile_texel),
                    i32(tile_origin.z),
                    vec4<f32>(spike_accum, select(0.0, 1.0, stored_slot.valid)),
                );
",
                ),
                AnimatedDirect => (
                    "local_probe_is_kept(cell_index, local_probe)",
                    "true",
                    "                let atlas_texel = tile_origin.xy + tile_texel;
                let uv = (vec2<f32>(atlas_texel) + 0.5)
                    / vec2<f32>(textureDimensions(direct_intermediate_atlas));
                var spike_accum = textureSampleLevel(
                    direct_intermediate_atlas,
                    intermediate_sampler,
                    uv,
                    i32(tile_origin.z),
                    0.0,
                ).rgb;
",
                    "                textureStore(
                    direct_composed_atlas,
                    vec2<i32>(tile_origin.xy + tile_texel),
                    i32(tile_origin.z),
                    vec4<f32>(
                        max(spike_accum, vec3<f32>(0.0)),
                        select(0.0, 1.0, stored_slot.valid),
                    ),
                );
",
                ),
            };
            // The coarse branch is compiled out under l0-only: ids 27/45 are
            // uniform L0 by compiler policy (`enforce_id41_only_coarsening_policy`).
            let coarse_gate = if l0_only { "false" } else { "spike_coarse" };
            let body = format!(
                "{base_init}                if (level == 0u) {{
                    if (spike_l0) {{
                        for (var entry = start; entry < end; entry = entry + 1u) {{
                            spike_accum = spike_accum
                                + read_delta_texel(entry, spike_rank, tile_texel).rgb * spike_scale(entry, start, spike_cached);
                        }}
                    }}
                }} else if ({coarse_gate}) {{
                    for (var entry = start; entry < end; entry = entry + 1u) {{
                        var delta = vec3<f32>(0.0);
                        if (level == 2u) {{
                            var spike_tile = vec3<f32>(0.0);
                            if (spike_rep_kept) {{
                                spike_tile = read_delta_texel(entry, spike_rep_rank, tile_texel).rgb;
                            }}
                            delta = spike_tile * f32(spike_rep_kept);
                        }} else {{
                            delta = spike_reconstruct_l1(cell_index, entry, local_probe, tile_texel);
                        }}
                        spike_accum = spike_accum + delta * spike_scale(entry, start, spike_cached);
                    }}
                }}
{store}"
            );
            let texels = if unroll {
                // spike unroll36: one block per texel with constant coordinates,
                // so the texel loop carries no loop-bound counter or index math.
                (0..36)
                    .map(|t| {
                        format!(
                            "            {{
                let tile_texel = vec2<u32>({x}u, {y}u);
{body}            }}
",
                            x = t % 6,
                            y = t / 6
                        )
                    })
                    .collect::<String>()
            } else {
                format!(
                    "            for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {{
                let tile_texel = vec2<u32>(
                    texel_index % RUNTIME_TILE_DIMENSION,
                    texel_index / RUNTIME_TILE_DIMENSION,
                );
{body}            }}
"
                )
            };
            let fused = format!(
                "    // spike array-free: every level fuses the base read, the entry sum and the
    // store per texel. L1/L2 lanes reconstruct from their own reads of the kept
    // corner tiles, so the kernel holds no 36-entry accumulator and no barriers
    // after the scale cache.
    if (grid.row_count > 0u) {{
        if (output_is_stored) {{
            let spike_l0 = {gate_l0};
            let spike_coarse = {gate_coarse};
            let spike_rank = within_cell_rank(cell_index, local_probe);
            let spike_rep = l2_representative_local(cell_index);
            let spike_rep_kept = local_probe_is_kept(cell_index, spike_rep);
            let spike_rep_rank = within_cell_rank(cell_index, spike_rep);
{texels}        }}
        return;
    }}
"
            );
            replace_once(
                src,
                ACCUM_COMMENT,
                &format!("{LEVEL_BLOCK}{fused}{ACCUM_COMMENT}"),
                arm,
            );
            replace_once(
                src,
                "fn reconstruct_l1_shared_texel(",
                "// spike array-free: the shared-lattice reconstruction, reading each kept
// corner tile straight from the delta payload. Same slot order and arithmetic.
fn spike_reconstruct_l1(cell: u32, entry: u32, target_local: u32, tile_texel: vec2<u32>) -> vec3<f32> {
    var accum = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var slot = 0u; slot < MAX_KEPT_TILES; slot = slot + 1u) {
        let corner = l1_corner_local(slot);
        if (local_probe_is_kept(cell, corner)) {
            let weight = l1_corner_weight(target_local, corner);
            if (weight > 0.0) {
                accum = accum + read_delta_texel(entry, within_cell_rank(cell, corner), tile_texel).rgb * weight;
                weight_sum = weight_sum + weight;
            }
        }
    }
    if (weight_sum > 0.0) {
        return accum / weight_sum;
    }
    return vec3<f32>(0.0);
}

fn reconstruct_l1_shared_texel(",
                arm,
            );
        }
        "scale-shared" => {
            replace_n(
                src,
                "animated_light_scale(affinity_lights[entry])",
                "spike_scale(entry, start, spike_cached)",
                2,
                arm,
            );
            replace_once(
                src,
                SHARED_DECL,
                "var<workgroup> shared_brick_indirection: u32;
var<workgroup> spike_scales: array<vec3<f32>, 64>;

fn spike_scale(entry: u32, start: u32, cached: bool) -> vec3<f32> {
    if (cached) {
        return spike_scales[entry - start];
    }
    return animated_light_scale(affinity_lights[entry]);
}
",
                arm,
            );
            replace_once(
                src,
                LET_END,
                "    let end = affinity_offsets[offset_index + 1u];
    // spike scale-shared: one lane per CSR entry evaluates its light's scale
    // once per workgroup instead of every lane evaluating every entry.
    let spike_cached = end - start <= 64u;
    if (spike_cached && local_probe < end - start) {
        spike_scales[local_probe] = animated_light_scale(affinity_lights[start + local_probe]);
    }
    workgroupBarrier();
",
                arm,
            );
        }
        "vec3-accum" => {
            replace_once(
                src,
                "    var accum: array<vec4<f32>, 36>;",
                "    var accum: array<vec3<f32>, 36>;",
                arm,
            );
            replace_once(
                src,
                "            );
        } else {
            accum[texel_index] = vec4<f32>(0.0);",
                "            ).rgb;
        } else {
            accum[texel_index] = vec3<f32>(0.0);",
                arm,
            );
            replace_once(
                src,
                "                    accum[texel_index] = vec4<f32>(
                        prior.rgb + read_delta_texel(entry, probe_rank, tile_texel).rgb * scale,
                        prior.a,
                    );",
                "                    accum[texel_index] = prior + read_delta_texel(entry, probe_rank, tile_texel).rgb * scale;",
                arm,
            );
            replace_once(
                src,
                "accum[texel_index] = vec4<f32>(prior.rgb + delta * scale, prior.a);",
                "accum[texel_index] = prior + delta * scale;",
                arm,
            );
        }
        "const-tile" => replace_once(
            src,
            "    let texel_index = tile_texel.y * grid.tile_dimension + tile_texel.x;
    let texel_f16_count = grid.delta_probe_f16_stride
        / (grid.tile_dimension * grid.tile_dimension);",
            "    // spike const-tile: PRL validation pins the tile to 6x6 RGB16F texels.
    let texel_index = tile_texel.y * RUNTIME_TILE_DIMENSION + tile_texel.x;
    let texel_f16_count = 3u;",
            arm,
        ),
        "floor" => {
            replace_once(src, LET_END, "    let end = start; // spike floor\n", arm);
        }
        "no-base" => match shader {
            Indirect => replace_once(
                src,
                "accum[texel_index] = sample_compact_base_atlas(stored_slot.slot, tile_texel).rgb;",
                "accum[texel_index] = vec3<f32>(0.0); // spike no-base",
                arm,
            ),
            AnimatedDirect => replace_once(
                src,
                "            accum[texel_index] = textureSampleLevel(
                direct_intermediate_atlas,
                intermediate_sampler,
                uv,
                i32(tile_origin.z),
                0.0,
            );",
                "            accum[texel_index] = vec4<f32>(0.0); // spike no-base",
                arm,
            ),
        },
        "stores-off" => replace_once(
            src,
            "    if (output_is_stored) {
        for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
            let tile_texel = vec2<u32>(
                texel_index % RUNTIME_TILE_DIMENSION,
                texel_index / RUNTIME_TILE_DIMENSION,
            );
            textureStore(",
            "    // spike stores-off: a never-true guard the compiler cannot fold.
    if (output_is_stored && accum[0].x == -1.0e30) {
        for (var texel_index = 0u; texel_index < TILE_TEXEL_COUNT; texel_index = texel_index + 1u) {
            let tile_texel = vec2<u32>(
                texel_index % RUNTIME_TILE_DIMENSION,
                texel_index / RUNTIME_TILE_DIMENSION,
            );
            textureStore(",
            arm,
        ),
        "accum-scalar" => {
            let (from, to) = match shader {
                Indirect => (
                    "    var accum: array<vec3<f32>, 36>;",
                    "    var accum: array<vec3<f32>, 1>; // spike accum-scalar",
                ),
                AnimatedDirect => (
                    "    var accum: array<vec4<f32>, 36>;",
                    "    var accum: array<vec4<f32>, 1>; // spike accum-scalar",
                ),
            };
            replace_once(src, from, to, arm);
            replace_n(src, "accum[texel_index]", "accum[0]", 7, arm);
        }
        "const-scale" => replace_once(
            src,
            "fn animated_light_scale(light_index: u32) -> vec3<f32> {\n",
            "fn animated_light_scale(light_index: u32) -> vec3<f32> {
    return vec3<f32>(0.5); // spike const-scale
}

fn spike_unused_light_scale(light_index: u32) -> vec3<f32> {
",
            arm,
        ),
        "rank0" => replace_once(
            src,
            "        + probe_rank * grid.delta_probe_f16_stride\n",
            "        + 0u * grid.delta_probe_f16_stride // spike rank0\n",
            arm,
        ),
        "skip-coarse" => {
            let from = match shader {
                Indirect => {
                    "            for (var entry = start; entry < end; entry = entry + 1u) {
                if (local_probe < MAX_KEPT_TILES) {"
                }
                AnimatedDirect => {
                    "        for (var entry = start; entry < end; entry = entry + 1u) {
            if (local_probe < MAX_KEPT_TILES) {"
                }
            };
            let to = from.replace("entry < end;", "entry < start; /* spike skip-coarse */");
            replace_once(src, from, &to, arm);
        }
        "single-slot" => replace_once(
            src,
            "for (var slot = 0u; slot < MAX_KEPT_TILES; slot = slot + 1u) {",
            "for (var slot = 0u; slot < 1u; slot = slot + 1u) { // spike single-slot",
            arm,
        ),
        other => unreachable!("arm `{other}` validated in ordered_arms()"),
    }
}

/// Per-pass composed-row mix for one window of compose frames: rows and CSR
/// entries by brick level (L0/L1/L2), from the section's whole-level metadata.
#[derive(Default)]
pub(crate) struct RowCountWindow {
    frames: u32,
    min_rows: u64,
    max_rows: u64,
}

impl RowCountWindow {
    const WINDOW: u32 = 120;

    pub(crate) fn record(
        &mut self,
        pass: &str,
        rows: &[u32],
        metadata: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    ) {
        let Some(metadata) = metadata else {
            return;
        };
        let total = rows.len() as u64;
        if self.frames == 0 {
            self.min_rows = total;
            self.max_rows = total;
        }
        self.min_rows = self.min_rows.min(total);
        self.max_rows = self.max_rows.max(total);
        self.frames += 1;
        if self.frames < Self::WINDOW {
            return;
        }
        let mut counts = [0_u64; 6];
        for &row in rows {
            let row = row as usize;
            let level = usize::from(metadata.cell_levels.get(row).copied().unwrap_or(0).min(2));
            let entries = match (
                metadata.affinity_offsets.get(row),
                metadata.affinity_offsets.get(row + 1),
            ) {
                (Some(start), Some(end)) => u64::from(end.saturating_sub(*start)),
                _ => 0,
            };
            counts[level] += 1;
            counts[3 + level] += entries;
        }
        log::info!(
            "[SH spike counts] {pass}: frames {frames} rows min {min} max {max} | last rows L0 {r0} L1 {r1} L2 {r2} | entries L0 {e0} L1 {e1} L2 {e2}",
            frames = self.frames,
            min = self.min_rows,
            max = self.max_rows,
            r0 = counts[0],
            r1 = counts[1],
            r2 = counts[2],
            e0 = counts[3],
            e1 = counts[4],
            e2 = counts[5],
        );
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(src: &str, label: &str) {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|err| panic!("{label}: arm source parses: {}", err.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|err| panic!("{label}: arm source validates: {err:?}"));
    }

    // Every arm, alone and in the stacks the batches use, must rewrite and validate.
    #[test]
    fn every_arm_rewrites_and_validates() {
        let decode = crate::render::sh_indirection::WGSL_DECODE_HELPER;
        let mut lists: Vec<String> = KNOWN_ARMS
            .iter()
            .filter(|a| !matches!(**a, "unroll36" | "l0-only"))
            .map(|a| (*a).to_string())
            .collect();
        lists.extend(
            [
                "baseline",
                "floor,no-base",
                "floor,scan-parallel",
                "floor,stores-off",
                "texel-outer",
                "scan-parallel,texel-outer,vec3-accum",
                "scan-parallel,texel-outer,vec3-accum,const-tile",
                "array-free",
                "floor,array-free",
                "scan-parallel,array-free,const-tile",
                "array-free,unroll36",
                "array-free,l0-only",
                "array-free,unroll36,l0-only",
            ]
            .map(String::from),
        );
        for list in &lists {
            let arms = ordered_arms(list);
            for shader in [ComposeShader::Indirect, ComposeShader::AnimatedDirect] {
                validate(&build_source(shader, decode, &arms), &format!("{list} {shader:?}"));
            }
        }
    }
}

impl crate::render::Renderer {
    /// Spike-only: copy the streamed composed indirect and animated-direct
    /// atlases to `<dir>/{indirect,direct}.bin` (tight rows, all layers) plus a
    /// `dims.json`. Blocks on the GPU; capture-only.
    pub fn spike_dump_sh_atlases(&self, dir: &std::path::Path) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let full = self.full();
        let state = full
            .sh_streaming
            .as_ref()
            .context("spike atlas dump needs streamed SH")?;
        let (total, direct) = state
            .spike_composed_atlases()
            .context("spike atlas dump before streamed SH GPU init")?;
        std::fs::create_dir_all(dir)?;
        let mut dims = Vec::new();
        for (name, texture) in [("indirect", Some(total)), ("direct", direct)] {
            let Some(texture) = texture else {
                continue;
            };
            let size = texture.size();
            let texel_bytes = texture
                .format()
                .block_copy_size(None)
                .context("atlas format has a copy size")?;
            let tight = size.width * texel_bytes;
            let padded = tight.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer_size =
                u64::from(padded) * u64::from(size.height) * u64::from(size.depth_or_array_layers);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Spike SH Atlas Dump"),
                size: buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Spike SH Atlas Dump Encoder"),
                });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(size.height),
                    },
                },
                size,
            );
            self.queue.raw().submit(std::iter::once(encoder.finish()));
            let slice = buffer.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .context("waiting for spike atlas dump")?;
            let view = slice.get_mapped_range().context("spike atlas dump map")?;
            let mut out = Vec::with_capacity(
                tight as usize * size.height as usize * size.depth_or_array_layers as usize,
            );
            for row in view.chunks_exact(padded as usize) {
                out.extend_from_slice(&row[..tight as usize]);
            }
            drop(view);
            buffer.unmap();
            std::fs::write(dir.join(format!("{name}.bin")), &out)?;
            dims.push(format!(
                "\"{name}\": {{\"width\": {}, \"height\": {}, \"layers\": {}, \"format\": \"{:?}\"}}",
                size.width,
                size.height,
                size.depth_or_array_layers,
                texture.format()
            ));
        }
        std::fs::write(dir.join("dims.json"), format!("{{{}}}\n", dims.join(", ")))?;
        log::info!("[SH spike] dumped composed SH atlases to {}", dir.display());
        Ok(())
    }
}
