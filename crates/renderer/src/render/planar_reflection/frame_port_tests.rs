//! Adapter-backed planar proofs at very high on the standing-quad scene, plus layer lifetime.
//! See: context/lib/rendering_pipeline.md §7.3a

use super::frame_test_fixtures::*;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    self as fixture, FRAME, capture_checked, gap, pixels, rgb,
};
use crate::render::fluid_surface_frame_test::look_draw;
use glam::{UVec2, Vec3, Vec4};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::PlanarPreset;

fn ssr_records(renderer: &Renderer) -> u32 {
    renderer.full().fluid_surface.reflection_pass_records()
}

// The mask marks only water the main view sees within R: a pixel with no
// water, water behind the standing quad, and water past R read c = 0, while
// near visible water mirroring the room reads c > 0.
#[test]
fn planar_gpu_mask_rejects_dry_occluded_and_beyond_r_pixels() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    renderer.set_planar_preset_override(Some(PlanarPreset {
        r: Some(6.0),
        band_r: 1.0,
        ..full_scale()
    }));
    fixture::capture_checked(&mut renderer, view);
    let reflected = read_planar_layer(&renderer);
    assert_eq!(reflected.extent, UVec2::splat(FRAME));
    let scene = fixture::read_scene(&renderer);
    let (mut dry, mut far, mut wet, mut occluded) = (0, 0, 0, 0);
    for pixel in pixels() {
        let sample = reflected.at(pixel.as_ivec2());
        match fixture::on_plane(view, fixture::centre(pixel), 0.0) {
            Some(water) if fixture::on_pool(water, 0.01) => {
                let (_, water_depth) = fixture::project(view, water);
                if water_depth > scene.depth_at(pixel) + 0.00001 {
                    assert_eq!(sample.w, 0.0, "occluded water at {pixel}");
                    occluded += 1;
                } else if water.distance(view.eye) >= 6.02 {
                    assert_eq!(sample.w, 0.0, "water past R at {pixel}");
                    far += 1;
                } else if sample.w > 0.0 {
                    wet += 1;
                }
            }
            Some(water) if fixture::on_pool(water, -0.03) => {}
            _ => {
                assert_eq!(sample.w, 0.0, "dry pixel {pixel}");
                dry += 1;
            }
        }
    }
    eprintln!("[PlanarProof] mask: dry={dry} far={far} wet={wet} occluded={occluded}");
    assert!(
        dry > 20 && far > 20 && wet > 20 && occluded > 20,
        "dry={dry}, far={far}, wet={wet}, occluded={occluded}"
    );
    renderer.release_level_resources();
}

// The standing quad's reflection lands on its mirrored pixel in the planar
// layer, magenta, and not on the pixel mirrored across the screen. With the
// selected plane the
// only reflecting water drawn, SSR records nothing; once a second plane
// draws, SSR records for it.
#[test]
fn planar_gpu_asymmetric_world_lands_on_mirrored_pixel_and_ssr_skips_the_selected_plane() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    let before = ssr_records(&renderer);
    capture_checked(&mut renderer, view);
    assert_eq!(
        ssr_records(&renderer),
        before,
        "only the selected plane reflects"
    );
    let reflected = read_planar_layer(&renderer);
    let pixel = pixel_of(view, Vec3::new(-0.8, -0.75, 4.0)).expect("on screen");
    let sample = reflected.at(pixel.as_ivec2());
    assert!(
        sample.w > 0.95 && sample.x > 0.01 && sample.z > 0.01 && sample.y < 0.01,
        "expected magenta at {pixel}, got {sample:?}"
    );
    let opposite = reflected.at(mirrored(pixel).as_ivec2());
    assert!(
        opposite.x < 0.01,
        "asymmetric reflection was flipped twice: {opposite:?}"
    );

    renderer.install_fluid_surfaces(
        &[fixture::base_pool(), fixture::pool(1, 0.4, &[3.0, 6.0])],
        Some(&fixture::green_probe(&[(0, 0), (1, 0)])),
    );
    renderer.set_fluid_surface_draws(&[
        look_draw(0, fixture::mirror()),
        look_draw(1, fixture::mirror()),
    ]);
    let (before, planar) = (ssr_records(&renderer), renderer.planar_reflection_records());
    capture_checked(&mut renderer, view);
    assert_eq!(
        ssr_records(&renderer),
        before + 1,
        "SSR records for the other plane"
    );
    assert_eq!(renderer.planar_reflection_records(), planar + 1);
    assert!(read_planar_layer(&renderer).at(pixel.as_ivec2()).w > 0.95);
    renderer.release_level_resources();
}

// A frame with no selected plane records no planar pass and reads none of an
// earlier frame's planar layer: its water draws the SSR-only frame byte for
// byte.
#[test]
fn planar_gpu_no_selected_plane_clears_previous_confidence() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    deselect(&mut renderer);
    let ssr_only = capture_checked(&mut renderer, view);
    select(&mut renderer, view, 0.0);
    let planar = capture_checked(&mut renderer, view);
    assert!(
        read_planar_layer(&renderer)
            .texels
            .iter()
            .any(|p| p.w > 0.0)
    );
    assert!(
        planar != ssr_only,
        "the selected plane reads the planar layer"
    );
    deselect(&mut renderer);
    let records = renderer.planar_reflection_records();
    let after = capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records);
    assert!(
        after == ssr_only,
        "no earlier frame's planar confidence reaches the water"
    );
    renderer.release_level_resources();
}

// Regression: a fluid reload that changes the plane's faces
// but keeps its bounds and height refreshes the water the mask covers. The
// mask rasters the installed fluid draws, so it holds no geometry of its own.
#[test]
fn planar_gpu_changed_faces_with_same_plane_bounds_refresh_the_water_mask() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    capture_checked(&mut renderer, view);
    let before = read_planar_layer(&renderer);
    let mut replacement = fixture::base_pool();
    replacement.surface_faces[0].vertices.remove(1);
    renderer.install_fluid_surfaces(&[replacement], Some(&fixture::green_probe(&[(0, 0)])));
    renderer.set_fluid_surface_draws(&[look_draw(0, fixture::mirror())]);
    capture_checked(&mut renderer, view);
    let after = read_planar_layer(&renderer);
    assert!(
        before
            .texels
            .iter()
            .zip(&after.texels)
            .any(|(old, new)| old.w > 0.0 && new.w == 0.0),
        "the removed triangle's water reads c = 0"
    );
    assert!(after.texels.iter().any(|pixel| pixel.w > 0.0));
    renderer.release_level_resources();
}

// D fades by the distance from the mirror eye to the reflected surface: with
// D at 3 m every mirrored surface lies beyond it, so the planar layer holds
// c = 0 everywhere. With no plane selected the frame is the SSR-only frame,
// and dropping to high and back restores each tier's frame byte for byte.
#[test]
fn planar_gpu_d_distance_fades_and_disabling_restores_ssr_bytes() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    deselect(&mut renderer);
    let very_high_ssr = capture_checked(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    let high = capture_checked(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    select(&mut renderer, view, 0.0);
    renderer.set_planar_preset_override(Some(PlanarPreset {
        d: Some(3.0),
        band_d: 1.0,
        ..full_scale()
    }));
    capture_checked(&mut renderer, view);
    assert!(
        read_planar_layer(&renderer)
            .texels
            .iter()
            .all(|p| *p == Vec4::ZERO),
        "all reflected geometry is beyond D = 3 m"
    );
    deselect(&mut renderer);
    assert!(capture_checked(&mut renderer, view) == very_high_ssr);
    select(&mut renderer, view, 0.0);
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert!(
        capture_checked(&mut renderer, view) == high,
        "below very high a selected plane changes nothing"
    );
    renderer.release_level_resources();
}

// An upper water plane over
// the selected lower one reads SSR, not the lower plane's planar layer and
// not the probe alone. The upper plane covers every visible pixel of the
// lower at full weight, so the frame matches the frame with no plane selected
// within 1/255 (the lower draw's rounding under the upper's unit coverage),
// and differs from the probe-only frame.
#[test]
fn planar_gpu_other_water_plane_reads_ssr() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    renderer.install_fluid_surfaces(
        &[
            fixture::base_pool(),
            fixture::pool(1, 1.0, &fixture::POOL_X),
        ],
        Some(&fixture::green_probe(&[(0, 0), (1, 0)])),
    );
    renderer.set_fluid_surface_draws(&[
        look_draw(0, fixture::mirror()),
        look_draw(1, fixture::mirror()),
    ]);
    deselect(&mut renderer);
    let ssr_only = capture_checked(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::Low);
    let probe_only = capture_checked(&mut renderer, view);
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    select(&mut renderer, view, 0.0);
    let records = renderer.planar_reflection_records();
    let planar = capture_checked(&mut renderer, view);
    assert_eq!(renderer.planar_reflection_records(), records + 1);
    assert!(
        read_planar_layer(&renderer)
            .texels
            .iter()
            .any(|p| p.w > 0.0)
    );
    let worst = pixels()
        .map(|pixel| gap(rgb(&planar, pixel), rgb(&ssr_only, pixel)))
        .max()
        .unwrap_or(0);
    assert!(
        worst <= 1,
        "the upper plane reads SSR, never the selected lower plane's planar layer: {worst}"
    );
    let added = pixels()
        .filter(|&pixel| gap(rgb(&planar, pixel), rgb(&probe_only, pixel)) > 16)
        .count();
    assert!(
        added > 20,
        "SSR adds to the probe on the upper plane: {added}"
    );
    renderer.release_level_resources();
}

// The layer lives at the planar tiers on a level with a candidate plane,
// before any plane is selected; high releases it and builds nothing more; a
// level with no probe-bound face allocates none at very high; unload
// releases it.
#[test]
fn planar_gpu_layer_lives_at_planar_tiers_with_a_candidate_plane() {
    let Some((mut renderer, view)) = standing_quad_renderer() else {
        return;
    };
    deselect(&mut renderer);
    // The fixture's draws staged their parameter rows: a frame submits them
    // before the installs below.
    capture_checked(&mut renderer, view);
    let extent = renderer.full().fluid_surface.planar_layer_extent();
    assert_eq!(extent, Some((FRAME, FRAME)));
    assert_eq!(renderer.planar_mirror_extent(), extent);
    renderer.set_planar_preset_override(None);
    assert_eq!(
        renderer.full().fluid_surface.planar_layer_extent(),
        Some((FRAME / 2, FRAME / 2)),
        "very high's own preset: half scale"
    );
    let builds = renderer.full().fluid_surface.planar_layer_builds();
    renderer.set_reflection_quality(FluidReflectionQuality::Ultra);
    assert_eq!(
        renderer.full().fluid_surface.planar_layer_builds(),
        builds,
        "two planar tiers with one scale build nothing"
    );
    renderer.set_reflection_quality(FluidReflectionQuality::High);
    assert_eq!(renderer.full().fluid_surface.planar_layer_extent(), None);
    assert_eq!(renderer.planar_mirror_extent(), None);
    assert!(renderer.planar_reflection_preset().is_none());
    renderer.set_reflection_quality(FluidReflectionQuality::VeryHigh);
    assert!(
        renderer
            .full()
            .fluid_surface
            .planar_layer_extent()
            .is_some()
    );
    renderer.install_fluid_surfaces(&[fixture::base_pool()], None);
    assert_eq!(
        renderer.full().fluid_surface.planar_layer_extent(),
        None,
        "no probe-bound face, no candidate plane"
    );
    renderer.install_fluid_surfaces(
        &[fixture::base_pool()],
        Some(&fixture::green_probe(&[(0, 0)])),
    );
    assert!(
        renderer
            .full()
            .fluid_surface
            .planar_layer_extent()
            .is_some()
    );
    renderer.release_level_resources();
    assert_eq!(renderer.full().fluid_surface.planar_layer_extent(), None);
    assert_eq!(renderer.planar_mirror_extent(), None);
}
