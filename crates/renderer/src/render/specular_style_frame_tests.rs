// The specular style's transport through the real renderer: the bytes the
// forward pass (group 0, `spec_shadowmask_flags` bit 2) and the mover pass
// (`KinematicLightParams.specular_style`) read on a recorded frame, across
// installs, unloads, full-renderer rebuilds and live changes. What each word
// does to a highlight is proven by the forward and mover readback harnesses
// (`world_specular_test.rs`, `kinematic_specular_test.rs`).
// See: context/lib/rendering_pipeline.md §4 (Texel-centre highlight)

use glam::{Mat4, Vec3};
use postretro_level_loader::ShDrainBatch;
use postretro_render_cpu::frame_uniforms::{
    KINEMATIC_SPECULAR_STYLE_SMOOTH, SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH, SpecularStyle, UNIFORM_SIZE,
};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::gpu_create::gpu_creation_counts;
use super::gpu_test_harness::{offscreen_renderer_or_skip, read_uniform_bytes};
use super::{ClearColor, KinematicMoverInstance, Renderer, ShSampleRegionSets};

const MOVER: u32 = 3;
const EXTENT: u32 = 32;

/// One mover: a two-sided quad 1.4 m wide and 1.8 m tall about its bottom
/// centre, at its origin.
fn mover_geometry() -> postretro_level_loader::KinematicGeometry {
    use postretro_level_format::geometry::Vertex;
    let corners = [
        Vec3::new(-0.7, 0.0, 0.0),
        Vec3::new(0.7, 0.0, 0.0),
        Vec3::new(0.7, 1.8, 0.0),
        Vec3::new(-0.7, 1.8, 0.0),
    ];
    let mut vertices = Vec::new();
    for (corner, normal) in corners
        .iter()
        .map(|corner| (*corner, [0.0, 0.0, 1.0]))
        .chain(
            corners
                .iter()
                .rev()
                .map(|corner| (*corner, [0.0, 0.0, -1.0])),
        )
    {
        vertices.push(Vertex::new(
            corner.to_array(),
            [0.1, 0.1],
            normal,
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    postretro_level_loader::KinematicGeometry {
        movers: vec![postretro_level_loader::LoadedKinematicMover {
            mover_id: MOVER,
            name: "specular-style-mover".into(),
            tags: Vec::new(),
            origin: Vec3::ZERO,
            path: String::new(),
            speed_mps: 1.0,
            wait_ms: 0.0,
            move_mode: 1,
            start_on_spawn: false,
            vertices,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            face_meta: Vec::new(),
            spin_axis: Vec3::ZERO,
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".into(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            open_sound: None,
            close_sound: None,
            blocked_sound: None,
            crush_sound: None,
            sealed_portal_ids: Vec::new(),
            carried_lights: Vec::new(),
            crush_damage_type: None,
            light_ramp: Default::default(),
        }],
        waypoints: Vec::new(),
    }
}

/// Install a level: no world geometry, one drawn mover in front of the
/// camera, so a recorded frame runs the mover pass.
fn install_level(renderer: &mut Renderer) {
    renderer.install_empty_level_for_test();
    add_mover(renderer);
}

/// Give the active level one drawn mover in front of the camera, so a
/// recorded frame runs the mover pass. Level data, never a style commit.
pub(super) fn add_mover(renderer: &mut Renderer) {
    let device = renderer.device.clone();
    renderer
        .level_mut()
        .kinematic
        .replace_geometry(&device, &mover_geometry(), 1);
    let movers = [KinematicMoverInstance {
        mover_id: MOVER,
        transform: Mat4::IDENTITY,
    }];
    renderer.set_kinematic_mover_draws(&movers, &movers);
}

/// One recorded frame, looking at the mover from 3 m.
pub(super) fn frame(renderer: &mut Renderer) {
    let eye = Vec3::new(0.0, 0.9, 3.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.9, 0.0), Vec3::Y);
    let projection = glam::camera::rh::proj::directx::perspective(1.2, 1.0, 0.05, 100.0);
    let all = VisibleCells::DrawAll;
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &all,
                path: VisibilityPath::EmptyWorldFallback,
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &all,
                fog_cells: &[],
                movers: &[],
            },
            None,
            projection * view,
            eye,
            &[],
            &[],
            1.0,
            ClearColor {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            true,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records");
}

/// The style each pass reads, as the GPU holds its per-frame uniform after
/// the last recorded frame: the forward word's bit 2 and the mover word.
pub(super) fn transported(renderer: &Renderer) -> (bool, u32) {
    let full = renderer.full();
    let group0 = read_uniform_bytes(renderer, &full.uniform_buffer, UNIFORM_SIZE as u64);
    let flags = u32::from_ne_bytes(group0[124..128].try_into().expect("a word"));
    let mover = read_uniform_bytes(
        renderer,
        full.kinematic_brush.light_params_buffer_for_test(),
        32,
    );
    let word = u32::from_ne_bytes(mover[28..32].try_into().expect("a word"));
    (flags & SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH != 0, word)
}

pub(super) fn expected(style: SpecularStyle) -> (bool, u32) {
    match style {
        SpecularStyle::Pixel => (false, 0),
        SpecularStyle::Smooth => (true, KINEMATIC_SPECULAR_STYLE_SMOOTH),
    }
}

// Style transport half: a style committed with no level loaded,
// before a full-renderer rebuild, or before an unload reaches both passes'
// uniforms on the first frame of the next installed level. Each install is
// checked under both styles, so neither a stale nor a default word passes.
#[test]
fn specular_style_reaches_the_first_frame_of_each_install() {
    let test = "specular_style_reaches_the_first_frame_of_each_install";
    for style in SpecularStyle::ALL {
        let other = match style {
            SpecularStyle::Pixel => SpecularStyle::Smooth,
            SpecularStyle::Smooth => SpecularStyle::Pixel,
        };
        let Some(mut renderer) = offscreen_renderer_or_skip(test, EXTENT, EXTENT) else {
            return;
        };
        // Full init has run and no level is installed: commit, then install.
        assert!(renderer.active_level_set().is_none());
        renderer.set_specular_style(style);
        install_level(&mut renderer);
        frame(&mut renderer);
        assert_eq!(
            transported(&renderer),
            expected(style),
            "{style:?}: first install"
        );

        // Unload, commit the other style with no level, then install again.
        renderer.release_level_resources();
        assert!(renderer.active_level_set().is_none());
        renderer.set_specular_style(other);
        install_level(&mut renderer);
        frame(&mut renderer);
        assert_eq!(
            transported(&renderer),
            expected(other),
            "{other:?}: unload then install"
        );

        // Commit, then rebuild the full renderer (surface recovery), then
        // install: the rebuilt passes read the committed style.
        renderer.set_specular_style(style);
        renderer.finish_full_init().expect("full init rebuilds");
        assert_eq!(renderer.specular_style(), style, "boot state survives");
        install_level(&mut renderer);
        frame(&mut renderer);
        assert_eq!(
            transported(&renderer),
            expected(style),
            "{style:?}: rebuild then install"
        );
    }
}

// Live-change transport half: a live change reaches the very next frame's forward
// and mover uniforms, creating no pipeline or bind group and publishing no
// level set beyond what an unchanged frame does.
#[test]
fn a_live_specular_style_change_reaches_the_next_frame_without_a_rebuild() {
    let test = "a_live_specular_style_change_reaches_the_next_frame_without_a_rebuild";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, EXTENT, EXTENT) else {
        return;
    };
    install_level(&mut renderer);
    frame(&mut renderer);
    frame(&mut renderer);
    assert_eq!(transported(&renderer), expected(SpecularStyle::Pixel));
    let set = renderer.active_level_set();

    let before = gpu_creation_counts();
    frame(&mut renderer);
    let unchanged = gpu_creation_counts().since(before);

    for style in [
        SpecularStyle::Smooth,
        SpecularStyle::Pixel,
        SpecularStyle::Smooth,
    ] {
        renderer.set_specular_style(style);
        let before = gpu_creation_counts();
        frame(&mut renderer);
        let changed = gpu_creation_counts().since(before);
        assert_eq!(transported(&renderer), expected(style), "{style:?}");
        assert_eq!(changed.pipelines, 0, "{style:?}: no pipeline builds");
        assert_eq!(
            changed.bind_groups, unchanged.bind_groups,
            "{style:?}: no bind group beyond an unchanged frame's"
        );
        assert_eq!(changed, unchanged, "{style:?}: the frame creates as before");
        assert_eq!(
            renderer.active_level_set(),
            set,
            "{style:?}: same level set"
        );
    }
}

// One read per frame: group 0 snapshots the boot-state style before its
// upload, and the mover params upload reads that copy, never the live boot
// state, so a commit landing between the two writes cannot split a frame's
// forward and mover styles. Source pin: no hook sits between the two writes
// for a recorded frame to exercise.
#[test]
fn forward_and_mover_uploads_read_one_per_frame_specular_style_snapshot() {
    let frame = include_str!("renderer_frame.rs");
    let snapshot = frame
        .find("full.frame_specular_style = *specular_style;")
        .expect("each group-0 upload must snapshot the boot-state style");
    let upload = frame
        .find("specular_style: full.frame_specular_style,")
        .expect("group 0 must pack the frame snapshot");
    assert!(
        snapshot < upload,
        "snapshot the style before packing group 0"
    );
    let render = include_str!("renderer_render_frame.rs");
    let args = render
        .split("kinematic_brush.write_light_params(")
        .nth(1)
        .expect("the mover pass uploads its params while recording")
        .split_once(");")
        .expect("the params upload call terminates")
        .0;
    assert!(
        args.contains("full.frame_specular_style,"),
        "the mover params must carry the style snapshotted with group 0"
    );
    assert!(
        !render.contains("self.specular_style()"),
        "scene recording must not read the live boot-state style"
    );
}
