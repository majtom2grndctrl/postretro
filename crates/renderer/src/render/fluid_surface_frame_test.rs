//! Adapter-backed fluid surface draw through the production capture recorder.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::{ClearColor, FluidSurfaceDraw, Renderer, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_loader::ShDrainBatch;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const SIDE: u32 = 16;
const WHITE: ClearColor = ClearColor {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const ALL: VisibleCells = VisibleCells::DrawAll;

fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(SIDE, SIDE) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[FluidSurfaceProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}

/// A screen-height quad spanning clip x `x0..x1` at clip depth 0.5 under an
/// identity view-projection, wound clockwise from the camera so it is seen
/// from inside its volume.
fn fluid_quad(x0: f32, x1: f32) -> EnvironmentVolumeRecord {
    // Texel UVs over one tile of the 64-texel placeholder checkerboard.
    let corner = |x: f32, y: f32| EnvironmentSurfaceVertex {
        position: [x, y, 0.5],
        uv: [(x + 1.0) * 32.0, (y + 1.0) * 32.0],
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
                corner(x0, -1.0),
                corner(x0, 1.0),
                corner(x1, 1.0),
                corner(x1, -1.0),
            ],
        }],
    }
}

fn full_screen_fluid() -> EnvironmentVolumeRecord {
    fluid_quad(-1.0, 1.0)
}

/// Mean red channel of one captured frame over its left and right halves.
fn capture_mean_red_halves(renderer: &mut Renderer) -> (f32, f32) {
    let pixels = renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &ALL,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &ALL,
                fog_cells: &[],
                movers: &[],
            },
            None,
            Mat4::IDENTITY,
            Vec3::new(0.0, 0.0, -1.0),
            &[],
            &[],
            1.0,
            WHITE,
            true,
            ShDrainBatch::default(),
        )
        .unwrap()
        .frame
        .unwrap();
    let (mut left, mut right) = (0u32, 0u32);
    for (index, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if (index as u32 % SIDE) < SIDE / 2 {
            left += u32::from(px[0]);
        } else {
            right += u32::from(px[0]);
        }
    }
    let half_count = (pixels.len() / 8) as f32;
    (left as f32 / half_count, right as f32 / half_count)
}

fn capture_mean_red(renderer: &mut Renderer) -> f32 {
    let (left, right) = capture_mean_red_halves(renderer);
    (left + right) * 0.5
}

// The pass builds, binds its material and parameter window, and draws: an
// unlit fluid face (no SH, no lights, zero ambient floor) blends black over
// the white clear at its surface opacity, no draw leaves the clear untouched,
// and a lit face shows its albedo.
#[test]
fn fluid_surface_blends_over_the_scene_at_its_surface_opacity() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()]);

    renderer.set_fluid_surface_draws(&[]);
    let clear = capture_mean_red(&mut renderer);

    renderer.set_fluid_surface_draws(&[FluidSurfaceDraw {
        volume_index: 0,
        surface_opacity: 0.5,
    }]);
    let half = capture_mean_red(&mut renderer);

    renderer.set_fluid_surface_draws(&[FluidSurfaceDraw {
        volume_index: 0,
        surface_opacity: 1.0,
    }]);
    let opaque = capture_mean_red(&mut renderer);

    eprintln!("[FluidSurfaceProof] mean red: clear={clear} half={half} opaque={opaque}");
    assert!(clear > 200.0, "white clear expected, got {clear}");
    assert!(opaque < 1.0, "an unlit opaque face is black, got {opaque}");
    assert!(
        half > opaque + 20.0 && half < clear - 20.0,
        "half opacity must land between clear ({clear}) and opaque ({opaque}), got {half}"
    );

    // Lit by the ambient floor, the face shows its material: the placeholder's
    // magenta squares carry red.
    renderer.set_ambient_floor(1.0);
    let lit = capture_mean_red(&mut renderer);
    eprintln!("[FluidSurfaceProof] mean red lit by the ambient floor: {lit}");
    assert!(
        lit > 20.0,
        "the ambient floor must light the fluid's albedo, got {lit}"
    );

    // Unloading drops the geometry and closes the pass.
    renderer.release_level_resources();
    renderer.set_fluid_surface_draws(&[FluidSurfaceDraw {
        volume_index: 0,
        surface_opacity: 1.0,
    }]);
    assert!(!renderer.full().fluid_surface.has_draws());
}

// Each draw reads its own volume's opacity: the parameter row is addressed by
// the draw's first instance, so a second volume must not read the first's.
#[test]
fn fluid_surface_draws_read_each_volumes_own_opacity_slot() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // A dry volume between them takes no slot, so volume 2 is slot 1.
    let dry = EnvironmentVolumeRecord {
        surface_faces: Vec::new(),
        ..fluid_quad(0.0, 1.0)
    };
    renderer.install_fluid_surfaces(&[fluid_quad(-1.0, 0.0), dry, fluid_quad(0.0, 1.0)]);
    renderer.set_fluid_surface_draws(&[
        FluidSurfaceDraw {
            volume_index: 2,
            surface_opacity: 0.5,
        },
        FluidSurfaceDraw {
            volume_index: 0,
            surface_opacity: 1.0,
        },
    ]);
    let (left, right) = capture_mean_red_halves(&mut renderer);
    eprintln!("[FluidSurfaceProof] per-volume opacity: left={left} right={right}");
    assert!(left < 1.0, "volume 0 is opaque and unlit, got {left}");
    assert!(
        right > 100.0 && right < 240.0,
        "volume 2 is half transparent, got {right}"
    );
}

// R14: a mid-level drift re-resolve rewrites only the drift buffer. The next
// frame still draws at the same opacity, and a later frame with unchanged
// opacities uploads no parameter rows.
#[test]
fn fluid_drift_re_resolve_keeps_draws_and_uploads_no_parameter_rows() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()]);
    let draws = [FluidSurfaceDraw {
        volume_index: 0,
        surface_opacity: 0.5,
    }];
    renderer.set_fluid_surface_draws(&draws);
    let before = capture_mean_red(&mut renderer);
    assert_eq!(renderer.full().fluid_surface.param_uploads(), Some(1));

    renderer.set_fluid_surface_drift(&[Vec3::new(3.0, 0.0, 0.0)]);
    renderer.set_fluid_surface_draws(&draws);
    let after = capture_mean_red(&mut renderer);
    renderer.set_fluid_surface_draws(&draws);
    let later = capture_mean_red(&mut renderer);

    eprintln!("[FluidSurfaceProof] re-resolve: before={before} after={after} later={later}");
    assert!(
        (after - before).abs() < 2.0 && (later - before).abs() < 2.0,
        "the pool keeps its opacity across a re-resolve: {before} → {after} → {later}"
    );
    assert_eq!(
        renderer.full().fluid_surface.param_uploads(),
        Some(1),
        "unchanged opacities upload no parameter rows"
    );
}
