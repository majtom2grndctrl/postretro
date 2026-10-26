// Mirror crop geometry: the NDC remap, the angular pad and the near-plane clip.

use super::*;
use crate::region_walk::SNAP_STEPS;
use crate::visibility::extract_frustum_planes;

const VFOV: f32 = 1.2;
const ASPECT: f32 = 1.6;

/// A camera at `eye` looking along −z.
fn view_proj(eye: Vec3) -> Mat4 {
    glam::camera::rh::proj::directx::perspective(VFOV, ASPECT, 0.1, 500.0)
        * glam::camera::rh::view::look_at_mat4(eye, eye - Vec3::Z, Vec3::Y)
}

fn ndc(m: Mat4, p: Vec3) -> Vec3 {
    let clip = m * p.extend(1.0);
    clip.truncate() / clip.w
}

fn square(centre: Vec3, half: f32) -> [Vec3; 4] {
    [
        centre + Vec3::new(-half, 0.0, -half),
        centre + Vec3::new(half, 0.0, -half),
        centre + Vec3::new(half, 0.0, half),
        centre + Vec3::new(-half, 0.0, half),
    ]
}

#[test]
fn mirror_crop_remap_stretches_the_rect_to_the_screen_and_keeps_depth() {
    let m = view_proj(Vec3::ZERO);
    assert_eq!(ndc_rect_to_full_screen(m, NdcRect::FULL), m);
    let rect = NdcRect {
        min: Vec2::new(-0.5, 0.1),
        max: Vec2::new(0.25, 0.9),
    };
    let cropped = ndc_rect_to_full_screen(m, rect);
    let inverse = m.inverse();
    for (corner, expected) in [
        (Vec3::new(-0.5, 0.1, 0.4), Vec2::NEG_ONE),
        (Vec3::new(0.25, 0.9, 0.7), Vec2::ONE),
        (Vec3::new(-0.125, 0.5, 0.2), Vec2::ZERO),
    ] {
        let world = inverse.project_point3(corner);
        let out = ndc(cropped, world);
        assert!(out.truncate().abs_diff_eq(expected, 1e-4), "{out}");
        assert!((out.z - corner.z).abs() < 1e-4, "depth is unchanged");
    }
    let (full, crop) = (extract_frustum_planes(m), extract_frustum_planes(cropped));
    for i in [NEAR_PLANE_INDEX, NEAR_PLANE_INDEX + 1] {
        assert!(
            full.planes[i]
                .normal
                .abs_diff_eq(crop.planes[i].normal, 1e-6)
        );
    }
}

// The pad is an angle around each ray: a small face on the axis crops to
// the cone of the pad's half-angle, whatever its depth.
#[test]
fn mirror_crop_pads_each_ray_by_the_pad_angle_at_any_depth() {
    let pad = 0.3_f32;
    let fy = 1.0 / (VFOV / 2.0).tan();
    let fx = fy / ASPECT;
    let mut scratch = MirrorCropScratch::default();
    for depth in [0.5, 4.0, 300.0] {
        let face = square(Vec3::new(0.0, 0.0, -depth), depth * 1e-5);
        let rect = mirror_crop_bounds(
            view_proj(Vec3::ZERO),
            Vec3::ZERO,
            [&face[..]],
            pad,
            &mut scratch,
        )
        .expect("the face is on screen");
        assert!((rect.max.x - fx * pad.tan()).abs() < 1e-3, "{rect:?}");
        assert!((rect.min.x + fx * pad.tan()).abs() < 1e-3, "{rect:?}");
        assert!((rect.max.y - fy * pad.tan()).abs() < 1e-3, "{rect:?}");
        assert!((rect.min.y + fy * pad.tan()).abs() < 1e-3, "{rect:?}");
    }
    // A cone that reaches the eye plane leaves the screen edge on that side.
    let edge = Vec3::new(-0.5, 0.0, -1.0);
    let face = square(edge, 1e-4);
    assert!(ndc(view_proj(Vec3::ZERO), edge).x > -1.0, "on screen");
    let rect = mirror_crop_bounds(
        view_proj(Vec3::ZERO),
        Vec3::ZERO,
        [&face[..]],
        1.4,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(rect.min.x, -1.0);
}

// A face under and behind the eye crops from its on-screen part only, and
// faces wholly behind the eye or off screen crop to nothing.
#[test]
fn mirror_crop_clips_faces_at_the_near_plane_and_drops_unseen_faces() {
    let eye = Vec3::new(0.0, 1.0, 0.0);
    let m = view_proj(eye);
    let mut scratch = MirrorCropScratch::default();
    let under = square(Vec3::new(0.0, 0.0, 0.0), 50.0);
    let rect = mirror_crop_bounds(m, eye, [&under[..]], 0.0, &mut scratch).unwrap();
    assert_eq!(rect.min, Vec2::NEG_ONE);
    assert_eq!(rect.max.x, 1.0);
    assert!(
        rect.max.y < 0.0,
        "the water stays below the horizon: {rect:?}"
    );

    let behind = square(Vec3::new(0.0, 0.0, 20.0), 5.0);
    let aside = square(Vec3::new(400.0, 0.0, -10.0), 5.0);
    assert_eq!(
        mirror_crop_rect(m, eye, [&behind[..], &aside[..]], 0.4, &mut scratch),
        None
    );
    assert_eq!(
        crop_mirror_view_proj(m, eye, std::iter::empty(), 0.4, &mut scratch),
        None
    );
}

// The aligned crop holds the bounds, stays on screen, and is an integer zoom
// about a centre on the walk's snap grid, so its grid refines the screen's.
#[test]
fn mirror_crop_aligns_to_an_integer_zoom_about_a_snap_grid_centre() {
    let grid = SNAP_STEPS as f32;
    let mut zoomed = 0;
    for i in 0..2000 {
        let a = (i as f32 * 0.618_034).fract() * 2.0 - 1.0;
        let b = (i as f32 * 0.414_214).fract() * 2.0 - 1.0;
        let width = ((i as f32 * 0.271_828).fract().powi(3) * 2.0).max(1e-4);
        let lo = a.min(b).min(0.99);
        let rect = NdcRect {
            min: Vec2::new(lo, -1.0),
            max: Vec2::new((lo + width).min(1.0), 1.0),
        };
        let aligned = align_to_walk_grid(rect);
        assert!(
            aligned.min.x <= rect.min.x && aligned.max.x >= rect.max.x,
            "{rect:?} {aligned:?}"
        );
        assert!(aligned.min.x >= -1.0 && aligned.max.x <= 1.0);
        assert_eq!((aligned.min.y, aligned.max.y), (-1.0, 1.0));
        let zoom = 2.0 / (aligned.max.x - aligned.min.x);
        // The f32 rect rounds its ends; the walk's grid tolerates far more.
        assert!((zoom - zoom.round()).abs() < 2e-5 * zoom, "zoom {zoom}");
        let centre = (aligned.min.x + aligned.max.x) * 0.5 * grid;
        assert!((centre - centre.round()).abs() < 1e-3, "centre {centre}");
        zoomed += usize::from(zoom.round() > 1.0);
    }
    assert!(zoomed > 1000, "most narrow rects zoom: {zoomed}");
    assert_eq!(align_to_walk_grid(NdcRect::FULL), NdcRect::FULL);
}
