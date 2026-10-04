// DX12 shader-compile proof under FXC, the compiler wgpu falls back to when no
// `dxcompiler.dll` is present. FXC rejects shapes Vulkan and Metal accept: a
// loop it must unroll but cannot (X3511), forced by an implicit-derivative
// sample inside a varying-length loop or by a dynamic write into an array
// nested in a struct. Builds every full-init pipeline through the offscreen
// renderer; wgpu's default error handler panics on the first one FXC refuses.
// Pipelines that need level geometry (the cull passes, direct-SH promotion
// compose) are built at level install and are not covered here.
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": FXC
// only runs inside a DX12 pipeline build. Windows-only, `#[ignore]`-gated
// because FXC takes tens of seconds, and self-skips without a DX12 adapter:
//   cargo test -p postretro-renderer --lib dx12_fxc -- --ignored
// See: context/lib/rendering_pipeline.md §8

use super::Renderer;

fn fxc_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                shader_compiler: wgpu::Dx12Compiler::Fxc,
                ..Default::default()
            },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

// Regression: FXC X3511 on the SDF shadow pass (struct-nested array write in
// `select_sdf_lights`) and the forward pass (spot PCF `textureSampleCompare`
// inside the light loop) blocked every DX12 launch.
#[test]
#[ignore = "on demand: FXC compiles every renderer pipeline (tens of seconds)"]
fn dx12_fxc_compiles_every_full_init_pipeline() {
    match Renderer::new_offscreen_on(&fxc_instance(), 64, 64) {
        Ok(_) => {}
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("DX12 FXC test skipped: no DX12 adapter ({error:#})");
        }
        Err(error) => panic!("DX12 offscreen renderer initialization failed: {error:#}"),
    }
}
