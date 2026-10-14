//! Adapter-backed fluid surface draw through the production capture recorder.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::{ClearColor, FluidSurfaceDraw, Renderer, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::fluid_surface::{
    FLUID_PARAMS_WINDOW_BYTES, FLUID_SLOTS_PER_WINDOW, FluidSurfaceLook, slot_window,
};
use postretro_render_cpu::render_extent::{Extent, RenderResolutionPolicy};
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

fn clear_draw(volume_index: u32, opacity: f32) -> FluidSurfaceDraw {
    look_draw(volume_index, FluidSurfaceLook::surface_only(opacity))
}

fn look_draw(volume_index: u32, look: FluidSurfaceLook) -> FluidSurfaceDraw {
    FluidSurfaceDraw {
        volume_index,
        params: look.pack(),
    }
}

/// A screen-height quad spanning clip x `x0..x1` at clip depth 0.5 under an
/// identity view-projection, its outward normal toward the capture camera at
/// z = −1, so it is seen from outside its volume.
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

/// The same quad wound the other way: its outward normal points away from
/// the camera, so the camera sees it from inside its volume.
fn full_screen_fluid_from_inside() -> EnvironmentVolumeRecord {
    let mut volume = full_screen_fluid();
    volume.surface_faces[0].vertices.reverse();
    volume
}

/// One captured frame's RGBA8 pixels at the renderer's capture extent.
fn capture(renderer: &mut Renderer) -> Vec<u8> {
    renderer
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
        .unwrap()
}

/// Mean red channel of one captured frame over its left and right halves.
fn capture_mean_red_halves(renderer: &mut Renderer) -> (f32, f32) {
    let pixels = capture(renderer);
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

    renderer.set_fluid_surface_draws(&[clear_draw(0, 0.5)]);
    let half = capture_mean_red(&mut renderer);

    renderer.set_fluid_surface_draws(&[clear_draw(0, 1.0)]);
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
    renderer.set_fluid_surface_draws(&[clear_draw(0, 1.0)]);
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
    renderer.set_fluid_surface_draws(&[clear_draw(2, 0.5), clear_draw(0, 1.0)]);
    let (left, right) = capture_mean_red_halves(&mut renderer);
    eprintln!("[FluidSurfaceProof] per-volume opacity: left={left} right={right}");
    assert!(left < 1.0, "volume 0 is opaque and unlit, got {left}");
    assert!(
        right > 100.0 && right < 240.0,
        "volume 2 is half transparent, got {right}"
    );
}

// Pin P1: a level whose draws span two params windows gives every volume its
// own row. The first window's last slot and the second window's first slot
// draw in one frame at distinct opacities, each read back from the pixels;
// the second window binds at a dynamic offset that is a multiple of 256.
#[test]
fn fluid_surface_draws_across_two_params_windows_read_their_own_rows() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let last_of_first = FLUID_SLOTS_PER_WINDOW - 1;
    let first_of_second = FLUID_SLOTS_PER_WINDOW;
    assert_eq!(slot_window(last_of_first), (0, FLUID_SLOTS_PER_WINDOW - 1));
    assert_eq!(slot_window(first_of_second), (1, 0));
    let alignment = u64::from(renderer.device.limits().min_uniform_buffer_offset_alignment);
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES % 256, 0);
    assert_eq!(FLUID_PARAMS_WINDOW_BYTES % alignment, 0);

    // What each opacity reads as on the right half from a one-window level
    // (the capture tonemaps, so 0.5 is not mid-grey).
    let mut reference = |opacity: f32| {
        renderer.install_fluid_surfaces(&[fluid_quad(-1.0, 0.0), fluid_quad(0.0, 1.0)]);
        renderer.set_fluid_surface_draws(&[clear_draw(1, opacity)]);
        capture_mean_red_halves(&mut renderer).1
    };
    let (half, light, dark) = (reference(0.5), reference(0.2), reference(0.9));
    assert!(
        (half - light).abs() > 10.0 && (half - dark).abs() > 10.0,
        "reference opacities must read apart: 0.5={half} 0.2={light} 0.9={dark}"
    );

    // Every volume draws a face, so volume i is slot i. All but the two under
    // test sit off screen.
    let volumes: Vec<EnvironmentVolumeRecord> = (0..=first_of_second + 1)
        .map(|volume| match volume {
            v if v == last_of_first => fluid_quad(-1.0, 0.0),
            v if v == first_of_second => fluid_quad(0.0, 1.0),
            _ => fluid_quad(3.0, 3.5),
        })
        .collect();
    renderer.install_fluid_surfaces(&volumes);
    // Slot 0 and the second window's second slot carry other opacities, so a
    // draw that read the wrong window or element would show.
    renderer.set_fluid_surface_draws(&[
        clear_draw(0, 0.2),
        clear_draw(last_of_first, 1.0),
        clear_draw(first_of_second, 0.5),
        clear_draw(first_of_second + 1, 0.9),
    ]);
    let (left, right) = capture_mean_red_halves(&mut renderer);
    eprintln!(
        "[FluidSurfaceProof] P1 two-window GPU case ran: slot {last_of_first} (window 0) \
         left={left}, slot {first_of_second} (window 1, offset {FLUID_PARAMS_WINDOW_BYTES}) \
         right={right} (one-window reference {half})"
    );
    assert!(left < 1.0, "window 0's last slot is opaque, got {left}");
    assert!(
        (right - half).abs() < 2.0,
        "window 1's first slot reads its own 0.5 row ({half}), got {right}"
    );
}

// A fogged fluid seen from outside draws murk over the scene behind it, even
// at surface opacity 0; the eye fog weight scales it, defaulting to 1 before
// any write; from inside the volume, or with fog omitted, it draws nothing.
#[test]
fn fluid_depth_fog_draws_from_outside_and_scales_with_the_eye_weight() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    assert_eq!(
        renderer.full().fluid_surface.scene_inputs().fog_weight(),
        1.0
    );
    // Opacity 0, fog distance 0.5: under the identity camera the path to the
    // cleared far depth is 0.5 at the centre, so transmittance is about 0.05
    // there; the face is unlit, so the murk is black over the white clear.
    let murk = FluidSurfaceLook {
        fog_distance: Some(0.5),
        fog_color: [0.5, 0.5, 0.5],
        ..FluidSurfaceLook::surface_only(0.0)
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()]);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let full = capture_mean_red(&mut renderer);

    renderer.set_fluid_fog_weight(0.5);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let half = capture_mean_red(&mut renderer);

    renderer.set_fluid_fog_weight(0.0);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let none = capture_mean_red(&mut renderer);

    // The capture tonemaps and encodes sRGB, so a ~3% linear remainder of the
    // white scene reads near 50 of 255.
    eprintln!("[FluidSurfaceProof] depth fog GPU case ran: weight 1={full} 0.5={half} 0={none}");
    assert!(full < 70.0, "full murk hides the white scene, got {full}");
    assert!(
        half > full + 60.0 && half < none - 30.0,
        "half weight draws half the murk, got {half} between {full} and {none}"
    );
    assert!(none > 250.0, "weight 0 draws no murk, got {none}");

    // Seen from inside, the same fluid draws no fog whatever the weight.
    renderer.set_fluid_fog_weight(1.0);
    renderer.install_fluid_surfaces(&[full_screen_fluid_from_inside()]);
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let inside = capture_mean_red(&mut renderer);
    eprintln!("[FluidSurfaceProof] depth fog seen from inside: {inside}");
    assert!(inside > 250.0, "no fog from inside, got {inside}");
}

// A mid-level drift re-resolve rewrites only the drift buffer. The next
// frame still draws at the same opacity, and a later frame with unchanged
// opacities uploads no parameter rows.
#[test]
fn fluid_drift_re_resolve_keeps_draws_and_uploads_no_parameter_rows() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(&[full_screen_fluid()]);
    let draws = [clear_draw(0, 0.5)];
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

fn scene_inputs_extent(renderer: &Renderer) -> (Extent, u32) {
    let (view, builds) = renderer.full().fluid_surface.scene_inputs().scene_depth();
    let texture = view.texture();
    (Extent::new(texture.width(), texture.height()), builds)
}

// Pin P4: group 5 exists from full init without a level, follows the scene
// depth through every resize with or without a level installed, and level
// install and release never rebuild it; the frame after a resize draws fog
// over the new depth.
#[test]
fn fluid_scene_inputs_follow_the_scene_depth_across_resizes_with_and_without_a_level() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (extent, built) = scene_inputs_extent(&renderer);
    assert_eq!(
        extent,
        renderer.scene_extent(),
        "built at full init, no level"
    );

    renderer.record_surface_size(40, 24);
    renderer.commit_extents().expect("the surface changed");
    assert_eq!(
        scene_inputs_extent(&renderer),
        (Extent::new(40, 24), built + 1),
        "resize without a level"
    );

    renderer.install_fluid_surfaces(&[full_screen_fluid()]);
    assert_eq!(
        scene_inputs_extent(&renderer).1,
        built + 1,
        "level install leaves group 5 alone"
    );

    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    renderer.record_surface_size(64, 48);
    renderer.commit_extents().expect("both extents changed");
    let scene = renderer.scene_extent();
    assert_eq!(scene, Extent::new(32, 24));
    assert_eq!(
        scene_inputs_extent(&renderer),
        (scene, built + 2),
        "resize with a level"
    );

    // Capture pins divisor 1, one more scene rebuild; the fogged frame after
    // it draws over the new depth.
    let murk = FluidSurfaceLook {
        fog_distance: Some(0.5),
        ..FluidSurfaceLook::surface_only(0.0)
    };
    renderer.set_fluid_surface_draws(&[look_draw(0, murk)]);
    let pixels = capture(&mut renderer);
    let scene = renderer.scene_extent();
    assert_eq!(scene, Extent::new(64, 48));
    assert_eq!(pixels.len(), (64 * 48 * 4) as usize);
    let mean_red = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| f32::from(px[0]))
        .sum::<f32>()
        / (64.0 * 48.0);
    assert_eq!(scene_inputs_extent(&renderer), (scene, built + 3));
    assert!(
        mean_red < 70.0,
        "fog draws after the resize, got {mean_red}"
    );

    renderer.release_level_resources();
    assert_eq!(
        scene_inputs_extent(&renderer),
        (scene, built + 3),
        "level release leaves group 5 alone"
    );
    eprintln!(
        "[FluidSurfaceProof] P4 scene-inputs GPU case ran: group 5 at {}x{} after {} builds, \
         mean red {mean_red}",
        scene.width,
        scene.height,
        built + 3
    );
}
