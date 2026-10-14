// Fluid depth fog reference tests: path solve, transmittance, per-face
// proximity, the outside gate and the straight-alpha composite.
// See: context/lib/testing_guide.md

use super::*;
use crate::fluid_surface::FluidSurfaceLook;

// The engine's projection (rendering_pipeline.md §11): right-handed, depth
// 0..1, near 0.1, far 4096. The depth pre-pass clears to 1.0 and tests `Less`,
// so a pixel nothing drew over reads 1.0.
const NEAR: f32 = 0.1;
const FAR: f32 = 4096.0;
const CLEARED_DEPTH: f32 = 1.0;

struct Camera {
    eye: Vec3,
    forward: Vec3,
    view_proj: Mat4,
}

fn camera(eye: Vec3, target: Vec3) -> Camera {
    let view = glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(1.2, 16.0 / 9.0, NEAR, FAR);
    Camera {
        eye,
        forward: (target - eye).normalize(),
        view_proj: proj * view,
    }
}

fn ndc_depth(view_proj: Mat4, point: Vec3) -> f32 {
    let clip = view_proj * point.extend(1.0);
    clip.z / clip.w
}

/// A camera above a pool whose surface is the plane y = 0, looking down
/// across it, and ray directions spread over its view.
fn pool_view() -> (Camera, Vec<Vec3>) {
    let cam = camera(Vec3::new(1.0, 2.0, 3.0), Vec3::new(2.0, -1.0, -4.0));
    let rays = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.3, 0.1, 0.0),
        Vec3::new(-0.4, -0.2, 0.1),
        Vec3::new(0.1, 0.3, -0.2),
    ]
    .iter()
    .map(|&jitter| (cam.forward + jitter).normalize())
    .collect();
    (cam, rays)
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: expected {expected}, got {actual} (tolerance {tolerance})"
    );
}

fn assert_vec4_close(actual: Vec4, expected: Vec4, what: &str) {
    assert!(
        (actual - expected).abs().max_element() <= 1.0e-5,
        "{what}: expected {expected}, got {actual}"
    );
}

#[test]
fn fog_path_matches_the_world_space_distance_to_the_scene_behind_the_surface() {
    let (cam, rays) = pool_view();
    for direction in rays {
        let surface_t = -cam.eye.y / direction.y;
        let frag = cam.eye + direction * surface_t;
        for extra in [0.05, 0.3, 1.0, 2.5, 8.0] {
            let scene = frag + direction * extra;
            let depth = ndc_depth(cam.view_proj, scene);
            let path = fog_path(cam.view_proj, cam.eye, frag, depth);
            assert_close(path, extra, 2.0e-3 + extra * 1.0e-3, "fog path");
        }
        // A scene point in front of the surface (nearer than the fragment)
        // leaves no path; the depth test would have dropped it anyway.
        let nearer = ndc_depth(cam.view_proj, cam.eye + direction * (surface_t * 0.5));
        assert_eq!(fog_path(cam.view_proj, cam.eye, frag, nearer), 0.0);
    }
}

#[test]
fn fog_path_matches_the_world_space_distance_at_the_near_plane_and_the_cleared_far_depth() {
    let (cam, rays) = pool_view();
    for direction in rays {
        // The ray meets the near plane where its forward component is NEAR.
        let near_t = NEAR / direction.dot(cam.forward);
        let near_frag = cam.eye + direction * near_t;
        assert_close(
            ndc_depth(cam.view_proj, near_frag),
            0.0,
            1.0e-4,
            "near depth",
        );
        for extra in [0.5, 3.0] {
            let depth = ndc_depth(cam.view_proj, near_frag + direction * extra);
            let path = fog_path(cam.view_proj, cam.eye, near_frag, depth);
            assert_close(path, extra, 2.0e-3, "fog path from the near plane");
        }

        // Nothing drew behind the surface: the path runs on to the far plane.
        let frag = cam.eye + direction * (-cam.eye.y / direction.y);
        let far_t = FAR / direction.dot(cam.forward);
        let expected = far_t - (frag - cam.eye).length();
        let path = fog_path(cam.view_proj, cam.eye, frag, CLEARED_DEPTH);
        // At depth 1 the solve subtracts clip terms that differ by about
        // near/far of their size, so f32 holds it to about half a percent of
        // the far distance; transmittance there is zero either way.
        assert_close(
            path,
            expected,
            expected * 1.0e-2,
            "fog path to the far plane",
        );
        assert!(fog_transmittance(3.0 / 50.0, path) < 1.0e-6);
    }
}

#[test]
fn transmittance_at_one_fog_distance_hides_ninety_five_percent() {
    for fog_distance in [0.01, 0.25, 1.0, 3.0, 40.0] {
        let row = FluidSurfaceLook {
            fog_distance: Some(fog_distance),
            ..FluidSurfaceLook::surface_only(0.5)
        }
        .pack();
        let transmittance = fog_transmittance(row.fog_extinction(), fog_distance);
        assert_close(transmittance, 0.05, 1.0e-3, "transmittance at fogDistance");
        assert_close(
            fog_transmittance(row.fog_extinction(), 0.0),
            1.0,
            0.0,
            "no path",
        );
    }
}

fn murky_fragment(cam: &Camera, frag: Vec3, outward: Vec3, depth: f32) -> FluidFogFragment {
    FluidFogFragment {
        view_proj: cam.view_proj,
        camera: cam.eye,
        world_position: frag,
        outward,
        scene_depth: depth,
        extinction: 3.0,
        fog_albedo: Vec3::new(0.04, 0.055, 0.0475),
        lighting: Vec3::splat(2.0),
        weight: 1.0,
    }
}

#[test]
fn a_face_seen_from_inside_or_without_fog_outputs_the_surface_unchanged() {
    let rgb = Vec3::new(0.3, 0.6, 0.9);
    let alpha = 0.4;

    // Seen from inside: the camera is below the surface, looking up at it.
    let cam = camera(Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.5, 0.0, -2.0));
    let frag = Vec3::new(0.25, 0.0, -1.0);
    let depth = ndc_depth(cam.view_proj, frag + (frag - cam.eye) * 3.0);
    let inside = murky_fragment(&cam, frag, Vec3::Y, depth);
    assert!(!fog_seen_from_outside(
        Vec3::Y,
        (cam.eye - frag).normalize()
    ));
    assert_eq!(fog_fragment(rgb, alpha, &inside), rgb.extend(alpha));

    // Fog omitted packs extinction 0: exactly today's output, from outside.
    let (cam, rays) = pool_view();
    let frag = cam.eye + rays[0] * (-cam.eye.y / rays[0].y);
    let depth = ndc_depth(cam.view_proj, frag + rays[0] * 2.0);
    let clear = FluidFogFragment {
        extinction: FluidSurfaceLook::surface_only(alpha)
            .pack()
            .fog_extinction(),
        ..murky_fragment(&cam, frag, Vec3::Y, depth)
    };
    assert_eq!(clear.extinction, 0.0);
    assert_eq!(fog_fragment(rgb, alpha, &clear), rgb.extend(alpha));

    // The same fragment with fog does draw murk, so the gate is what held.
    let murky = murky_fragment(&cam, frag, Vec3::Y, depth);
    assert_ne!(fog_fragment(rgb, alpha, &murky), rgb.extend(alpha));
}

#[test]
fn proximity_ramps_linearly_over_one_band_above_the_plane_and_is_zero_below() {
    let frag = Vec3::new(3.0, 1.0, -2.0);
    let at = |height: f32| fog_proximity(Vec3::new(5.0, 1.0 + height, 4.0), frag, Vec3::Y);
    assert_eq!(at(0.0), 0.0);
    assert_eq!(at(-0.01), 0.0);
    assert_eq!(at(-3.0), 0.0);
    assert_close(at(0.25 * FLUID_FOG_BAND), 0.25, 1.0e-5, "quarter band");
    assert_close(at(0.5 * FLUID_FOG_BAND), 0.5, 1.0e-5, "half band");
    assert_close(at(FLUID_FOG_BAND), 1.0, 1.0e-5, "one band");
    assert_eq!(at(10.0), 1.0);
}

// The fog layer scales by the frame's eye weight times the face's own
// proximity, the two values the shader multiplies: full at 1, none at 0.
#[test]
fn the_fog_layer_scales_by_the_eye_weight_times_the_face_proximity() {
    let (cam, rays) = pool_view();
    let frag = cam.eye + rays[1] * (-cam.eye.y / rays[1].y);
    let depth = ndc_depth(cam.view_proj, frag + rays[1] * 0.5);
    let rgb = Vec3::new(0.8, 0.2, 0.1);
    let alpha = 0.3;
    let base = murky_fragment(&cam, frag, Vec3::Y, depth);
    let path = fog_path(cam.view_proj, cam.eye, frag, depth);
    assert_close(path, 0.5, 2.0e-3, "fog path");
    let transmittance = fog_transmittance(base.extinction, path);
    let proximity = fog_proximity(cam.eye, frag, Vec3::Y);
    assert_eq!(proximity, 1.0, "the eye is well above the band");
    let fog_rgb = base.fog_albedo * base.lighting;
    for weight in [0.0, 0.25, 1.0] {
        let out = fog_fragment(rgb, alpha, &FluidFogFragment { weight, ..base });
        let opacity = fog_layer_opacity(weight, proximity, transmittance);
        assert_close(opacity, weight * (1.0 - transmittance), 1.0e-4, "layer");
        assert_vec4_close(
            out,
            fog_composite(rgb, alpha, fog_rgb, opacity),
            "weighted composite",
        );
    }
    // Weight 0 (a swimmer) leaves the surface as drawn without fog.
    let none = fog_fragment(
        rgb,
        alpha,
        &FluidFogFragment {
            weight: 0.0,
            ..base
        },
    );
    assert_vec4_close(none, rgb.extend(alpha), "no weight, no fog");
    // Full weight at full proximity hides nearly all of the scene: alpha is
    // the surface over a 1 − T fog layer.
    let full = fog_fragment(rgb, alpha, &base);
    assert_close(
        full.w,
        1.0 - (1.0 - alpha) * transmittance,
        1.0e-4,
        "full fog alpha",
    );
}

fn straight_alpha_blend(src: Vec4, dst: Vec3) -> Vec3 {
    src.truncate() * src.w + dst * (1.0 - src.w)
}

#[test]
fn the_composite_blended_over_the_scene_equals_surface_over_fog_over_scene() {
    let scene = Vec3::new(0.9, 0.7, 0.2);
    let fog_rgb = Vec3::new(0.05, 0.1, 0.08);
    let rgb = Vec3::new(0.2, 0.4, 0.6);
    for alpha in [0.0, 0.3, 0.75, 1.0] {
        for fog_opacity in [0.0, 0.2, 0.95, 1.0] {
            let out = fog_composite(rgb, alpha, fog_rgb, fog_opacity);
            let fog_over_scene = fog_rgb * fog_opacity + scene * (1.0 - fog_opacity);
            let expected = rgb * alpha + fog_over_scene * (1.0 - alpha);
            let blended = straight_alpha_blend(out, scene);
            assert!(
                (blended - expected).abs().max_element() <= 1.0e-5,
                "alpha {alpha}, fog {fog_opacity}: {blended} vs {expected}"
            );
        }
    }
    // Opacity 0 and no fog layer leaves the scene exactly as it was.
    let nothing = fog_composite(rgb, 0.0, fog_rgb, 0.0);
    assert_eq!(nothing.w, 0.0);
    assert_eq!(straight_alpha_blend(nothing, scene), scene);
}

// Pin P9: two fogged faces on one pixel (a split pool's seam, or a near pool
// over a far one) stack as one composite at T², never past the fog colour.
#[test]
fn the_same_fog_composited_twice_equals_one_composite_at_transmittance_squared() {
    let scene = Vec3::new(0.6, 0.5, 0.4);
    let fog_rgb = Vec3::new(0.04, 0.055, 0.0475);
    for transmittance in [0.9, 0.5, 0.05] {
        let layer = fog_layer_opacity(1.0, 1.0, transmittance);
        let once = straight_alpha_blend(fog_composite(Vec3::ZERO, 0.0, fog_rgb, layer), scene);
        let twice = straight_alpha_blend(fog_composite(Vec3::ZERO, 0.0, fog_rgb, layer), once);
        let squared = fog_layer_opacity(1.0, 1.0, transmittance * transmittance);
        let expected =
            straight_alpha_blend(fog_composite(Vec3::ZERO, 0.0, fog_rgb, squared), scene);
        assert!(
            (twice - expected).abs().max_element() <= 1.0e-5,
            "T {transmittance}: {twice} vs {expected}"
        );
        // Denser, toward the fog colour, never past it.
        for channel in 0..3 {
            let (low, high) = if fog_rgb[channel] < scene[channel] {
                (fog_rgb[channel], once[channel])
            } else {
                (once[channel], fog_rgb[channel])
            };
            assert!(twice[channel] >= low - 1.0e-6 && twice[channel] <= high + 1.0e-6);
        }
    }
}

// Pin P13: each face's proximity is its own. With the eye half a band above
// an upper pool, the upper face fogs at half strength while a lower pool in
// view fogs fully; a floating volume's downward bottom face is seen from
// outside by an eye below it.
#[test]
fn each_face_takes_its_own_proximity_upper_and_lower_pools_and_a_floating_bottom() {
    let eye = Vec3::new(0.0, 0.04, 0.0);
    let upper = Vec3::new(1.0, 0.0, -1.0);
    let lower = Vec3::new(3.0, -2.0, -6.0);
    assert_close(
        fog_proximity(eye, upper, Vec3::Y),
        0.5,
        1.0e-5,
        "upper pool",
    );
    assert_eq!(fog_proximity(eye, lower, Vec3::Y), 1.0, "lower pool");

    let bottom = Vec3::new(1.0, 5.0, 1.0);
    let down = Vec3::NEG_Y;
    let below = |gap: f32| Vec3::new(1.0, 5.0 - gap, 3.0);
    assert!(fog_seen_from_outside(
        down,
        (below(0.04) - bottom).normalize()
    ));
    assert_close(
        fog_proximity(below(0.04), bottom, down),
        0.5,
        1.0e-5,
        "0.04 m below",
    );
    assert_close(
        fog_proximity(below(FLUID_FOG_BAND), bottom, down),
        1.0,
        1.0e-5,
        "one band below",
    );
    assert_eq!(fog_proximity(below(1.0), bottom, down), 1.0);
}

// An eye lowered from one band above the surface to the plane, before the tint
// triggers, takes the face's proximity from 1 to 0 monotonically; bobbing
// across the plane it is 0 at every sample below, and between frames it moves
// by at most the height change over the band.
#[test]
fn proximity_falls_monotonically_to_the_plane_and_bobbing_moves_it_within_its_bound() {
    let frag = Vec3::new(0.0, 1.0, -3.0);
    let at = |height: f32| fog_proximity(Vec3::new(0.0, 1.0 + height, 0.0), frag, Vec3::Y);

    let steps = 200;
    let mut previous = at(FLUID_FOG_BAND);
    assert_close(previous, 1.0, 1.0e-5, "one band up");
    for step in 1..=steps {
        let height = FLUID_FOG_BAND * (1.0 - step as f32 / steps as f32);
        let proximity = at(height);
        assert!(proximity <= previous, "rose while lowering at {height}");
        previous = proximity;
    }
    assert_eq!(previous, 0.0, "zero at the plane");

    // ±0.05 m about the plane at a 2 Hz bob, sampled at 60 fps.
    let height = |frame: u32| 0.05 * (std::f32::consts::TAU * 2.0 * frame as f32 / 60.0).sin();
    for frame in 0..600 {
        let (h0, h1) = (height(frame), height(frame + 1));
        if h1 <= 0.0 {
            assert_eq!(at(h1), 0.0, "below the plane at frame {}", frame + 1);
        }
        let change = (at(h1) - at(h0)).abs();
        assert!(
            change <= (h1 - h0).abs() / FLUID_FOG_BAND + 1.0e-5,
            "frame {frame}: proximity moved {change} for a {} m bob step",
            (h1 - h0).abs()
        );
    }
}
