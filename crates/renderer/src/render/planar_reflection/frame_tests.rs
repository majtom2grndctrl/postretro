//! Adapter-backed planar routing proofs: two layers, the SSR skip, layer scales, same frame.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::frame_test_fixtures::*;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, Reference, capture_checked, gap, pixels, rgb,
};
use crate::render::fluid_surface_frame_test::look_draw;
use glam::{Mat4, UVec2, Vec2, Vec3, Vec4};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, FluidReflectionStyle,
};

fn ssr_records(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_pass_records()
}

// AC 7. At very high and at ultra, one frame holds a large selected plane and
// a second reflecting plane at another height, in smooth style. The large
// plane reads the planar layer: the ceiling object, off the main screen,
// lands on its mirrored pixel there, and not on the pixel mirrored across the
// screen; SSR alone (no plane selected) cannot show it. The second plane
// reads SSR: its fragments hold the standing object's reflection, which the
// probe alone (low) cannot show, and match the SSR-only frame within 1/255.
#[test]
fn planar_gpu_two_planes_read_planar_and_ssr_in_one_frame() {
    for quality in [
        FluidReflectionQuality::VeryHigh,
        FluidReflectionQuality::Ultra,
    ] {
        let Some(mut renderer) =
            two_plane_renderer(quality, FluidReflectionStyle::Smooth, &two_plane_world())
        else {
            return;
        };
        let view = fixture::base_view();
        renderer.set_reflection_quality(FluidReflectionQuality::Off);
        capture_checked(&mut renderer, view);
        let scene = fixture::read_scene(&renderer);
        renderer.set_reflection_quality(FluidReflectionQuality::Low);
        let probe_only = capture_checked(&mut renderer, view);
        renderer.set_reflection_quality(quality);
        let ssr_only = capture_checked(&mut renderer, view);
        assert!(
            renderer.full().fluid_surface.planar_layer_view().is_some(),
            "{quality:?}: a candidate plane holds the planar layer"
        );

        select(&mut renderer, view, 0.0);
        let (planar_before, ssr_before) =
            (renderer.planar_reflection_records(), ssr_records(&renderer));
        let frame = capture_checked(&mut renderer, view);
        assert_eq!(renderer.planar_reflection_records(), planar_before + 1);
        assert_eq!(
            ssr_records(&renderer),
            ssr_before + 1,
            "{quality:?}: SSR still records for the second plane"
        );

        // The large plane: planar.
        let pixel = pixel_of(view, CEILING_REFLECTED).expect("the reflection is on screen");
        let water = visible_on(view, &scene, pixel, 0.0, LARGE_X, 0.05)
            .expect("the reflection lies on the large plane");
        assert!(
            fixture::project(view, Vec3::new(-1.3, 4.2, 7.5)).0.y < 0.0,
            "the ceiling object is off the main screen"
        );
        let opposite = mirrored(pixel);
        assert!(visible_on(view, &scene, opposite, 0.0, LARGE_X, 0.05).is_some());
        eprintln!(
            "[PlanarProof] {quality:?}: ceiling reflection at {pixel} (water {water}): planar \
             {:?}, SSR only {:?}, opposite {:?}",
            rgb(&frame, pixel),
            rgb(&ssr_only, pixel),
            rgb(&frame, opposite)
        );
        assert!(magenta(rgb(&frame, pixel)), "{quality:?}: planar shows it");
        assert!(!magenta(rgb(&ssr_only, pixel)), "{quality:?}: SSR cannot");
        assert!(
            !magenta(rgb(&frame, opposite)),
            "{quality:?}: the reflection is not flipped across the screen"
        );

        // The second plane: SSR.
        let (mut second, mut hits) = (0, 0);
        for pixel in pixels() {
            if visible_on(view, &scene, pixel, SECOND_H, SECOND_X, 0.05).is_none() {
                continue;
            }
            second += 1;
            assert!(
                gap(rgb(&frame, pixel), rgb(&ssr_only, pixel)) <= 1,
                "{quality:?}: second-plane pixel {pixel} reads SSR: {:?} against {:?}",
                rgb(&frame, pixel),
                rgb(&ssr_only, pixel)
            );
            if magenta(rgb(&frame, pixel)) {
                hits += 1;
                assert!(
                    !magenta(rgb(&probe_only, pixel)),
                    "{quality:?}: the probe alone shows no object at {pixel}"
                );
            }
        }
        eprintln!("[PlanarProof] {quality:?}: {second} second-plane pixels, {hits} SSR hits");
        assert!(second > 40 && hits >= 4, "{quality:?}: {second}, {hits}");
        renderer.release_level_resources();
    }
}

// AC 8. With both planes drawn and the large one selected, the SSR pass
// records for the second plane and writes nothing on the selected plane's
// pixels. With the selected plane the only reflecting water drawn, no SSR
// pass records while the planar pass still does; with no plane selected, SSR
// covers the water again.
#[test]
fn planar_gpu_ssr_skips_the_selected_plane_and_records_only_for_others() {
    let Some(mut renderer) = two_plane_renderer(
        FluidReflectionQuality::VeryHigh,
        FluidReflectionStyle::Smooth,
        &two_plane_world(),
    ) else {
        return;
    };
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    let (planar, ssr) = (renderer.planar_reflection_records(), ssr_records(&renderer));
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), planar + 1);
    assert_eq!(
        ssr_records(&renderer),
        ssr + 1,
        "SSR records for the second plane"
    );
    let target = fixture::read_reflection_target(&renderer);
    assert_eq!(target.extent, UVec2::splat(FRAME));
    let (mut skipped, mut written) = (0, 0);
    for pixel in pixels() {
        let texel = target.at(pixel.as_ivec2());
        let large = on_extent(view, pixel, 0.0, LARGE_X, 0.05);
        let near_second = on_extent(view, pixel, SECOND_H, SECOND_X, -0.05);
        if large.is_some() && near_second.is_none() {
            assert_eq!(texel, Vec4::ZERO, "SSR wrote the selected plane at {pixel}");
            skipped += 1;
        }
        if on_extent(view, pixel, SECOND_H, SECOND_X, 0.05).is_some() && texel.w > 0.0 {
            written += 1;
        }
    }
    eprintln!("[PlanarProof] SSR skipped {skipped} selected-plane texels, wrote {written}");
    assert!(skipped > 100 && written >= 4, "{skipped}, {written}");

    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    let (planar, ssr) = (renderer.planar_reflection_records(), ssr_records(&renderer));
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), planar + 1);
    assert_eq!(
        ssr_records(&renderer),
        ssr,
        "only the selected plane reflects on screen: no SSR pass"
    );

    deselect(&mut renderer);
    let (planar, ssr) = (renderer.planar_reflection_records(), ssr_records(&renderer));
    capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), planar);
    assert_eq!(
        ssr_records(&renderer),
        ssr + 1,
        "no plane selected: SSR covers it"
    );
    renderer.release_level_resources();
}

// AC 9. Pixel style at very high: the planar layer at half scale and the SSR
// target at very high's full fraction. Every visible fragment of each cell
// reads its CPU composite from its own layer's texel at its own scale, the
// cell centre projected and scaled (the large plane from the planar layer,
// the second from the SSR target), and cells where the other scale would
// read a different composite show the read is scale-specific.
#[test]
fn planar_gpu_layers_read_at_their_own_scale() {
    let Some(mut renderer) = two_plane_renderer(
        FluidReflectionQuality::VeryHigh,
        FluidReflectionStyle::Pixel,
        &two_plane_world(),
    ) else {
        return;
    };
    let view = fixture::base_view();
    renderer.set_reflection_quality(FluidReflectionQuality::Off);
    capture_checked(&mut renderer, view);
    let scene = fixture::read_scene(&renderer);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    select(&mut renderer, view, 0.0);
    let frame = capture_checked(&mut renderer, view);
    let planar = read_planar_layer(&renderer);
    let ssr = fixture::read_reflection_target(&renderer);
    assert_eq!(planar.extent, UVec2::splat(FRAME / 2));
    assert_eq!(ssr.extent, UVec2::splat(FRAME));
    let fluid = &renderer.full().fluid_surface;
    assert_eq!(
        fluid
            .planar_group()
            .expect("the planar group")
            .uploaded()
            .ssr_scale,
        [0.5, 0.5]
    );
    assert_eq!(fluid.scene_inputs().uploaded().ssr_scale, [1.0, 1.0]);
    assert_eq!(
        fluid
            .planar_group()
            .expect("the planar group")
            .uploaded()
            .reflection_style,
        fluid.scene_inputs().uploaded().reflection_style,
        "both groups draw the author's style"
    );

    let reference = Reference::new(view, &scene, fixture::march_tuning(&renderer));
    for (name, h, x, layer, scale, other) in [
        ("planar", 0.0, LARGE_X, &planar, 0.5, 1.0),
        ("ssr", SECOND_H, SECOND_X, &ssr, 1.0, 0.5),
    ] {
        let sample = |at: Vec2, scale: f32| {
            if at.cmpge(Vec2::ZERO).all() && at.cmplt(Vec2::splat(FRAME as f32)).all() {
                layer.at((at * scale).floor().as_ivec2())
            } else {
                Vec4::ZERO
            }
        };
        let (mut fragments, mut discriminating, mut worst) = (0, 0, (0u8, UVec2::ZERO));
        let mut cells = std::collections::BTreeSet::new();
        for pixel in pixels() {
            let Some(water) = visible_on(view, &scene, pixel, h, x, 0.05) else {
                continue;
            };
            if fixture::cell_margin(water) < 0.01 {
                continue;
            }
            let cell = fixture::cell_of(water);
            let centre = fixture::cell_centre(cell, h);
            let (at, _) = fixture::project(view, centre);
            let texel_uv = cell.as_vec2() + 0.5;
            let own = reference.composite(
                FLUID_REFLECTION_STYLE_PIXEL,
                centre,
                texel_uv,
                sample(at, scale),
            );
            let deviation = gap(rgb(&frame, pixel), own);
            if deviation > worst.0 {
                worst = (deviation, pixel);
            }
            fragments += 1;
            if cells.insert((cell.x, cell.y)) {
                let wrong = reference.composite(
                    FLUID_REFLECTION_STYLE_PIXEL,
                    centre,
                    texel_uv,
                    sample(at, other),
                );
                discriminating += usize::from(gap(own, wrong) > 8);
            }
        }
        eprintln!(
            "[PlanarProof] {name}: {fragments} fragments in {} cells, worst {worst:?}, \
             {discriminating} cells read apart at the other scale",
            cells.len()
        );
        assert!(
            worst.0 <= 3,
            "{name}: each cell reads its own layer: {worst:?}"
        );
        assert!(cells.len() >= 6 && discriminating >= 2, "{name}");
    }
    renderer.release_level_resources();
}

/// One mover: a two-sided magenta quad, 1.4 m wide and 1.8 m tall about its
/// bottom centre, which sits at its origin.
fn mover_geometry() -> postretro_level_loader::KinematicGeometry {
    use postretro_level_format::geometry::Vertex;
    let corners = [
        Vec3::new(-0.7, 0.0, 0.0),
        Vec3::new(0.7, 0.0, 0.0),
        Vec3::new(0.7, 1.8, 0.0),
        Vec3::new(-0.7, 1.8, 0.0),
    ];
    let uv = [4.5 / 64.0, 4.5 / 64.0];
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
            uv,
            normal,
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        ));
    }
    let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
    postretro_level_loader::KinematicGeometry {
        movers: vec![postretro_level_loader::LoadedKinematicMover {
            mover_id: MOVER,
            name: "planar-mover".into(),
            tags: Vec::new(),
            origin: Vec3::ZERO,
            path: String::new(),
            speed_mps: 1.0,
            wait_ms: 0.0,
            move_mode: 1,
            start_on_spawn: false,
            vertices,
            indices,
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
        }],
        waypoints: Vec::new(),
    }
}

const MOVER: u32 = 1;

// AC 12 (O6). A mover above the large plane, off the main screen, moves
// between two consecutive frames: each frame's reflection lands on that
// frame's mirrored pixel, and the other pose's pixel holds none of it, so
// the planar pass draws this frame's content before the fluid pass reads it.
#[test]
fn planar_gpu_mirror_shows_this_frames_object() {
    let Some(mut renderer) = two_plane_renderer(
        FluidReflectionQuality::VeryHigh,
        FluidReflectionStyle::Smooth,
        &[standing_object(), fixture::back_wall()],
    ) else {
        return;
    };
    let device = renderer.device.clone();
    renderer
        .full_mut()
        .kinematic_brush
        .install_geometry(&device, Some(&mover_geometry()), 1);
    let view = fixture::base_view();
    select(&mut renderer, view, 0.0);
    let poses = [-1.3_f32, 1.2];
    let pixels: Vec<UVec2> = poses
        .iter()
        .map(|&x| pixel_of(view, Vec3::new(x, -5.1, 7.5)).expect("the reflection is on screen"))
        .collect();
    for (pixel, &x) in pixels.iter().zip(&poses) {
        assert!(
            on_extent(view, *pixel, 0.0, LARGE_X, 0.05).is_some(),
            "pose {x}: its reflection lies on the large plane"
        );
    }
    for (frame_index, &x) in poses.iter().enumerate() {
        // Every present mover rides the transform (shadow) list; the
        // beauty list draws only movers it holds.
        let mover = [crate::render::KinematicMoverInstance {
            mover_id: MOVER,
            transform: Mat4::from_translation(Vec3::new(x, 4.2, 7.5)),
        }];
        renderer.set_kinematic_mover_draws(&mover, &mover);
        let frame = capture_checked(&mut renderer, view);
        let here = pixels[frame_index];
        let there = pixels[1 - frame_index];
        eprintln!(
            "[PlanarProof] mover at x {x}: {here} reads {:?}, {there} reads {:?}",
            rgb(&frame, here),
            rgb(&frame, there)
        );
        assert!(
            magenta(rgb(&frame, here)),
            "frame {frame_index}: this pose reflects"
        );
        assert!(
            !magenta(rgb(&frame, there)),
            "frame {frame_index}: the other pose's pixel holds no stale reflection"
        );
    }
    renderer.release_level_resources();
}
