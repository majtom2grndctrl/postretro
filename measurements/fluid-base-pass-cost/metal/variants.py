"""Write ablated fluid shader sources for the base-pass attribution batch.

usage: python3 measurements/fluid-base-pass-cost/metal/variants.py [out_dir]

Each variant is the whole fluid shader module (fluid_surface.wgsl plus the
snippets `fluid_surface.rs` concatenates after it), with one textual edit to
`fs_main`, or to `fluid_baked_light` (the per-vertex baked terms, run from
`vs_main`). An experiment build reads one through
`POSTRETRO_FLUID_WGSL_EXPERIMENT` (never committed; see README.md). Every edit
must match exactly once, so a drifted shader fails loudly instead of timing the
wrong thing.
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SHADERS = ROOT / "crates/renderer/src/shaders"
# Order and separators of `fluid_surface.rs` SHADER_SOURCE.
PARTS = ("fluid_surface.wgsl", "fluid_motion.wgsl", "fluid_reflection.wgsl", "sh_indirection.wgsl", "sh_sample.wgsl", "curve_eval.wgsl",
         "light_falloff.wgsl", "light_eval.wgsl")

# The SH sample now lives in `fluid_baked_light` (per vertex, `world_pos`), so
# `nosh`, `shindirect`, `shshared` and `noocc` change the per-vertex cost, not per-fragment.
SH_CALL = "    let sh = fluid_sample_sh(world_pos, n, has_direct_sh());\n"
# Anchor for fs_main diagnostics; `n` and `in.world_position` are in scope there.
FS_LIGHTING = "    var lighting = in.baked;\n"
WARP_CALL = ("    let uv = fluid_warp_uv(in.uv, in.drift_scroll, dims, row.wave, uniforms.time, "
             "texels_per_pixel);\n")
ALBEDO = "    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy);\n"
FS_OPEN = "fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {\n"
FOG_OPEN = "    if extinction > 0.0 && fluid_fog_seen_from_outside(outward, V) {\n"
FOG_CLOSE = "        surface = fluid_fog_composite(rgb, alpha, row.fog_albedo.rgb * lighting, fog_opacity);\n    }\n"
DYN_OPEN = "    if use_dynamic {\n        for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {\n"

# name -> list of (old, new); applied to fluid_surface.wgsl only.
VARIANTS = {
    # Unchanged source through the override path: the A/B reference.
    "base": [],
    # No SH sample at all: both baked terms read zero.
    "nosh": [(SH_CALL, "    var sh: ShDirPair;\n    sh.a = vec3<f32>(0.0);\n    sh.b = vec3<f32>(0.0);\n")],
    # SH indirect only (the forward pass's sampler), no baked direct SH.
    "shindirect": [(SH_CALL, "    let sh = fluid_sample_sh(in.world_position, n, false);\n")],
    # No runtime light loops (as the DYNAMIC_DIRECT mask bit, but in code).
    "nodyn": [(DYN_OPEN, "    if false {\n        for (var i: u32 = 0u; i < uniforms.light_count; i = i + 1u) {\n")],
    # No whole-texel motion: sample at the unwarped UV.
    "nowarp": [(WARP_CALL, "    let uv = in.uv;\n")],
    # Fog block removed from the code (capture never fogs, so this is code shape only).
    "nofog": [(FOG_OPEN, "    if false {\n")],
    # Warp + albedo sample, nothing else.
    "albedo": [(ALBEDO, ALBEDO + "    return vec4<f32>(albedo.rgb, albedo.a * clamp(row.surface.x, 0.0, 1.0));\n")],
    # Raster and blend floor: a constant colour.
    "flat": [(FS_OPEN, FS_OPEN + "    if uniforms.time > -1.0 { return vec4<f32>(0.2, 0.3, 0.4, 0.6); }\n")],
}

# Candidate fix: one weight pass shared by the indirect and direct atlases.
# Same arithmetic per atlas as sample_sh_indirect_direct_corners, in the same
# order, so the image should match; only loads and weights are hoisted.
SH_SHARED_FNS = """
fn shared_l1_probe_pair(direct_atlas: texture_2d_array<f32>, idx: vec3<i32>, slot: u32, scale: u32,
                        dir: vec3<f32>) -> ShDirPair {
    let local = sh_l1_local(idx, scale);
    var sum_i = vec3<f32>(0.0);
    var sum_d = vec3<f32>(0.0);
    var ws_i = 0.0;
    var ws_d = 0.0;
    for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
        let weight = sh_l1_corner_weight(local, corner, scale);
        if (weight <= 0.0) {
            continue;
        }
        let si = sample_probe_atlas_slot(sh_total_atlas, slot + corner, dir);
        let sd = sample_probe_atlas_slot(direct_atlas, slot + corner, dir);
        let pi = weight * si.a;
        let pd = weight * sd.a;
        sum_i = sum_i + pi * max(si.rgb, vec3<f32>(0.0));
        sum_d = sum_d + pd * max(sd.rgb, vec3<f32>(0.0));
        ws_i = ws_i + pi;
        ws_d = ws_d + pd;
    }
    var r: ShDirPair;
    r.a = select(sum_i / ws_i, vec3<f32>(0.0), ws_i < SH_WEIGHT_EPSILON);
    r.b = select(sum_d / ws_d, vec3<f32>(0.0), ws_d < SH_WEIGHT_EPSILON);
    return r;
}

fn shared_resolved_pair(direct_atlas: texture_2d_array<f32>, idx: vec3<i32>,
                        indirection: ShProbeIndirection, dir: vec3<f32>) -> ShDirPair {
    var r: ShDirPair;
    r.a = vec3<f32>(0.0);
    r.b = vec3<f32>(0.0);
    if (!indirection.valid) {
        return r;
    }
    if (indirection.level == 1u) {
        return shared_l1_probe_pair(direct_atlas, idx, indirection.slot, indirection.scale, dir);
    }
    if (indirection.level == 0u || indirection.level == 2u) {
        r.a = sample_probe_atlas_slot(sh_total_atlas, indirection.slot, dir).rgb;
        r.b = sample_probe_atlas_slot(direct_atlas, indirection.slot, dir).rgb;
    }
    return r;
}

fn shared_indirect_direct_corners(
    direct_atlas: texture_2d_array<f32>,
    gi: vec3<u32>,
    gfrac: vec3<f32>,
    sample_world: vec3<f32>,
    shading_normal: vec3<f32>,
    geo_normal: vec3<f32>,
    reject_backface: bool,
    probe_occlusion_enabled: bool,
) -> ShDirPair {
    let whole = sh_whole_cell_resolution(gi);
    if (whole.available && whole.level == 1u) {
        var si: array<vec4<f32>, 8>;
        var sd: array<vec4<f32>, 8>;
        for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
            si[corner] = sample_probe_atlas_slot(sh_total_atlas, whole.slot + corner, shading_normal);
            sd[corner] = sample_probe_atlas_slot(direct_atlas, whole.slot + corner, shading_normal);
        }
        var sum_i = vec3<f32>(0.0);
        var sum_d = vec3<f32>(0.0);
        var ws_i = 0.0;
        var ws_d = 0.0;
        for (var c: u32 = 0u; c < 8u; c = c + 1u) {
            let corner_offset = sh_corner_offset(c);
            let idx = sh_corner_index(gi, corner_offset);
            let indirection = sh_probe_indirection(idx);
            let outer_weight = sh_probe_weight(
                idx, corner_offset, gfrac, sample_world, geo_normal, indirection.valid,
                reject_backface, true, probe_occlusion_enabled,
            );
            if (outer_weight <= 0.0) {
                continue;
            }
            let local = sh_l1_local(idx, whole.scale);
            var ri = vec3<f32>(0.0);
            var rd = vec3<f32>(0.0);
            var rwi = 0.0;
            var rwd = 0.0;
            for (var corner: u32 = 0u; corner < 8u; corner = corner + 1u) {
                let l1w = sh_l1_corner_weight(local, corner, whole.scale);
                let wi = l1w * si[corner].a;
                let wd = l1w * sd[corner].a;
                ri = ri + wi * max(si[corner].rgb, vec3<f32>(0.0));
                rd = rd + wd * max(sd[corner].rgb, vec3<f32>(0.0));
                rwi = rwi + wi;
                rwd = rwd + wd;
            }
            if (rwi >= SH_WEIGHT_EPSILON) {
                sum_i = sum_i + outer_weight * (ri / rwi);
                ws_i = ws_i + outer_weight;
            }
            if (rwd >= SH_WEIGHT_EPSILON) {
                sum_d = sum_d + outer_weight * (rd / rwd);
                ws_d = ws_d + outer_weight;
            }
        }
        var result: ShDirPair;
        result.a = select(sum_i / ws_i, vec3<f32>(0.0), ws_i < SH_WEIGHT_EPSILON);
        result.b = select(sum_d / ws_d, vec3<f32>(0.0), ws_d < SH_WEIGHT_EPSILON);
        return result;
    }
    if (whole.available && whole.level == 2u) {
        return sample_sh_indirect_direct_corners(direct_atlas, gi, gfrac, sample_world, shading_normal,
            geo_normal, reject_backface, probe_occlusion_enabled);
    }
    var sum_indirect = vec3<f32>(0.0);
    var sum_direct = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var c: u32 = 0u; c < 8u; c = c + 1u) {
        let corner_offset = sh_corner_offset(c);
        let idx = sh_corner_index(gi, corner_offset);
        let indirection = sh_probe_indirection(idx);
        let pair = shared_resolved_pair(direct_atlas, idx, indirection, shading_normal);
        let w = sh_probe_weight(idx, corner_offset, gfrac, sample_world, geo_normal, indirection.valid,
            reject_backface, true, probe_occlusion_enabled);
        sum_indirect = sum_indirect + w * max(pair.a, vec3<f32>(0.0));
        sum_direct = sum_direct + w * max(pair.b, vec3<f32>(0.0));
        weight_sum = weight_sum + w;
    }
    var result: ShDirPair;
    if (weight_sum < SH_WEIGHT_EPSILON) {
        result.a = vec3<f32>(0.0);
        result.b = vec3<f32>(0.0);
        return result;
    }
    result.a = sum_indirect / weight_sum;
    result.b = sum_direct / weight_sum;
    return result;
}
"""
SH_DIRECT_CALL = "        return sample_sh_indirect_direct_corners(\n            sh_direct_atlas,"
VARIANTS["shshared"] = [
    (SH_DIRECT_CALL, "        return shared_indirect_direct_corners(\n            sh_direct_atlas,"),
    ("@fragment\n" + FS_OPEN, SH_SHARED_FNS + "\n@fragment\n" + FS_OPEN),
]
# The level-2 branch of the shared path still calls the original; fine.

# Probe-depth (Chebyshev) visibility off: a look change, attribution only.
VARIANTS["noocc"] = [("    let probe_occlusion = sh_grid.probe_occlusion != 0u;\n",
                      "    let probe_occlusion = false;\n")]

# (The old `vertexsh` arm, SH evaluated per vertex, is now the shipped shader: `base`.)

# Diagnostics: images, never timed.
SH_PATH = """    if uniforms.time > -1.0 && sh_grid.has_sh_volume != 0u {
        // Which SH sampling path each fragment takes: red = L1 whole cell,
        // green = level-2 whole cell, blue (+ green by L1 corner share) = per corner.
        let ow = in.world_position + n * 0.1 * sh_grid.cell_size;
        let gd = max(vec3<f32>(sh_grid.grid_dimensions) - vec3<f32>(1.0), vec3<f32>(0.0));
        let gq = clamp((ow - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6)), vec3<f32>(0.0), gd);
        let gidx = vec3<u32>(floor(gq));
        let whole = sh_whole_cell_resolution(gidx);
        var l1 = 0u;
        for (var k = 0u; k < 8u; k = k + 1u) {
            if sh_probe_indirection(sh_corner_index(gidx, sh_corner_offset(k))).level == 1u { l1 = l1 + 1u; }
        }
        var c = vec3<f32>(0.0, f32(l1) / 8.0, 1.0);
        if whole.available && whole.level == 1u { c = vec3<f32>(1.0, 0.0, 0.0); }
        if whole.available && whole.level == 2u { c = vec3<f32>(0.0, 1.0, 0.0); }
        return vec4<f32>(c, 1.0);
    }
"""
# The same, at the scene point behind the water (the forward pass's fragment).
SH_PATH_FLOOR = SH_PATH.replace(
    "        let ow = in.world_position + n * 0.1 * sh_grid.cell_size;\n",
    "        let behind = fluid_fog_path(uniforms.view_proj, uniforms.camera_position, in.world_position,\n"
    "            textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0));\n"
    "        let floor_pos = in.world_position + normalize(in.world_position - uniforms.camera_position) * behind;\n"
    "        let ow = floor_pos + n * 0.1 * sh_grid.cell_size;\n")
DIAGNOSTICS = {"shpath": [(FS_LIGHTING, SH_PATH + FS_LIGHTING)], "shpathfloor": [(FS_LIGHTING, SH_PATH_FLOOR + FS_LIGHTING)]}


def build(name, edits, extra=None):
    text = (SHADERS / PARTS[0]).read_text()
    for old, new in edits:
        n = text.count(old)
        if n != 1:
            raise SystemExit(f"variant {name}: edit matches {n} times: {old[:60]!r}")
        text = text.replace(old, new)
    rest = [(SHADERS / p).read_text() for p in PARTS[1:]]
    if extra:
        rest = [extra(p, t) for p, t in zip(PARTS[1:], rest)]
    return "\n".join([text, *rest]) + "\n"


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "variants"
    out.mkdir(parents=True, exist_ok=True)
    for name, edits in {**VARIANTS, **DIAGNOSTICS}.items():
        (out / f"{name}.wgsl").write_text(build(name, edits))
        print(out / f"{name}.wgsl")


if __name__ == "__main__":
    main()
