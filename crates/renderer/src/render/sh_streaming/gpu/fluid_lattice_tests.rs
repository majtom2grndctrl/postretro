//! Adapter-backed proof that a streamed-SH level's fluid grid takes the manifest base lattice.
//! See: context/lib/rendering_pipeline.md §7.3a

use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_render_cpu::fluid_surface::FluidGridLattice;
use postretro_render_cpu::fluid_surface::grid::choose_grid_planes;

use crate::render::Renderer;

use super::load_fixture;

fn pool() -> EnvironmentVolumeRecord {
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 2.0, z],
        uv: [x * 16.0, z * 16.0],
    };
    EnvironmentVolumeRecord {
        source_entity: 0,
        priority: 0,
        gravity: None,
        push: None,
        fluid: "water".to_string(),
        brushes: Vec::new(),
        surface_faces: vec![EnvironmentSurfaceFace {
            texture_index: 0,
            vertices: vec![
                corner(1.1, 0.3),
                corner(1.1, 3.4),
                corner(9.7, 3.4),
                corner(9.7, 0.3),
            ],
        }],
    }
}

// P3: streamed state initialises after the legacy SH resources, which hold
// the placeholder lattice (origin 0, 1 m) and are not present; the grid is
// cut on the manifest base (origin 0.5 here), never the placeholder.
#[test]
fn a_streamed_level_cuts_its_fluid_on_the_manifest_base_lattice() {
    let mut renderer = match Renderer::new_offscreen(16, 16) {
        Ok(renderer) => renderer,
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[FluidLattice] skipped: no adapter ({error:#})");
            return;
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    };
    let (_temp, path) = load_fixture::write_nine_cluster_prl();
    let mut world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let base = world
        .sh_stream_manifest()
        .expect("the fixture streams by default")
        .base()
        .clone();
    let payloads = world.take_gpu_lighting_payloads();
    let geometry = crate::render::level_world_to_geometry(&world, &[]);
    renderer.install_level_geometry(&geometry, payloads);
    assert!(!renderer.full().sh_volume_resources.present);
    assert_eq!(renderer.full().sh_volume_resources.grid_origin, [0.0; 3]);

    let volumes = [pool()];
    renderer.install_fluid_surfaces(&volumes, None);
    let installed = renderer.full().fluid_surface.installed_grid();
    let manifest = FluidGridLattice::new(base.grid_origin, base.cell_size);
    let placeholder = FluidGridLattice::new([0.0; 3], [1.0; 3]);
    let expected = choose_grid_planes(&volumes, manifest.as_ref(), u64::MAX);
    assert!(expected.is_some());
    assert_eq!(installed, expected);
    assert_ne!(
        installed,
        choose_grid_planes(&volumes, placeholder.as_ref(), u64::MAX)
    );
}
