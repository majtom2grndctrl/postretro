// SH compose compute pass — stub phase.
//
// Per-frame compose of the static base SH irradiance volume plus per-light
// animated deltas into a parallel set of "total" SH band textures. SH
// consumers (forward, billboard, fog) sample the total textures, so any
// animated-delta data the full compose pass adds is automatically picked up
// without consumer-side branching.
//
// Stub behavior: this dispatch is a pure base→total copy. Delta data is
// zero. Validates that consumers correctly use the total textures and that
// the compose pipeline wiring is correct, before Task D's delta path is
// landed. The cost is one ~Rgba16Float blit of a typically-small 3D probe
// grid (~300 KB even for large maps).
//
// Dispatch shape: one workgroup per (8, 8, 1) probe-volume tile in
// `workgroup_id.{x,y,z}`. Each thread copies one probe across all 9 SH
// bands, with bounds checks against the actual grid dimensions.

@group(0) @binding(0) var sh_base_band0: texture_3d<f32>;
@group(0) @binding(1) var sh_base_band1: texture_3d<f32>;
@group(0) @binding(2) var sh_base_band2: texture_3d<f32>;
@group(0) @binding(3) var sh_base_band3: texture_3d<f32>;
@group(0) @binding(4) var sh_base_band4: texture_3d<f32>;
@group(0) @binding(5) var sh_base_band5: texture_3d<f32>;
@group(0) @binding(6) var sh_base_band6: texture_3d<f32>;
@group(0) @binding(7) var sh_base_band7: texture_3d<f32>;
@group(0) @binding(8) var sh_base_band8: texture_3d<f32>;

@group(0) @binding(9)  var sh_total_band0: texture_storage_3d<rgba16float, write>;
@group(0) @binding(10) var sh_total_band1: texture_storage_3d<rgba16float, write>;
@group(0) @binding(11) var sh_total_band2: texture_storage_3d<rgba16float, write>;
@group(0) @binding(12) var sh_total_band3: texture_storage_3d<rgba16float, write>;
@group(0) @binding(13) var sh_total_band4: texture_storage_3d<rgba16float, write>;
@group(0) @binding(14) var sh_total_band5: texture_storage_3d<rgba16float, write>;
@group(0) @binding(15) var sh_total_band6: texture_storage_3d<rgba16float, write>;
@group(0) @binding(16) var sh_total_band7: texture_storage_3d<rgba16float, write>;
@group(0) @binding(17) var sh_total_band8: texture_storage_3d<rgba16float, write>;

struct GridDims {
    dims: vec3<u32>,
    _pad: u32,
};
@group(0) @binding(18) var<uniform> grid: GridDims;

@compute @workgroup_size(4, 4, 4)
fn compose_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= grid.dims.x || gid.y >= grid.dims.y || gid.z >= grid.dims.z) {
        return;
    }
    let p = vec3<i32>(i32(gid.x), i32(gid.y), i32(gid.z));

    // Stub: copy each base band to the corresponding total band.
    textureStore(sh_total_band0, p, textureLoad(sh_base_band0, p, 0));
    textureStore(sh_total_band1, p, textureLoad(sh_base_band1, p, 0));
    textureStore(sh_total_band2, p, textureLoad(sh_base_band2, p, 0));
    textureStore(sh_total_band3, p, textureLoad(sh_base_band3, p, 0));
    textureStore(sh_total_band4, p, textureLoad(sh_base_band4, p, 0));
    textureStore(sh_total_band5, p, textureLoad(sh_base_band5, p, 0));
    textureStore(sh_total_band6, p, textureLoad(sh_base_band6, p, 0));
    textureStore(sh_total_band7, p, textureLoad(sh_base_band7, p, 0));
    textureStore(sh_total_band8, p, textureLoad(sh_base_band8, p, 0));
}
