// Planar pixel tilt proofs: SSR parity for the tilted normal and ray, smooth, and the clamped reprojection.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use crate::fluid_surface::motion::displayed_offset;
use crate::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_PIXEL, reflection_cell_within_box, reflection_point,
};
use crate::planar_reflection::MirrorCamera;

/// A sheared, scaled texel grid on a horizontal face, so neither axis is a
/// world axis and the two are not orthogonal.
fn axes() -> FluidTexelAxes {
    FluidTexelAxes {
        u: Vec3::new(0.21, 0.0, 0.04),
        v: Vec3::new(-0.03, 0.0, 0.17),
    }
}

/// The world point at `texel_uv` on the face whose texel origin sits at
/// `origin`.
fn world_at(texel_uv: Vec2, origin: Vec3) -> Vec3 {
    let axes = axes();
    origin + axes.u * texel_uv.x + axes.v * texel_uv.y
}

const EYE: Vec3 = Vec3::new(0.4, 2.3, -1.2);
/// A probe box whose -x face cuts the sampled water, so some cells straddle
/// its rim and the box pull moves them.
const BOX_MIN: Vec3 = Vec3::new(-1.0, -0.5, -4.0);
const BOX_MAX: Vec3 = Vec3::new(6.0, 4.0, 9.0);
const CAPTURE: Vec3 = Vec3::new(2.0, 0.3, 2.0);

/// SSR's pixel-style ray for the same cell: the probe term's point, then the
/// mirrored view kept above the horizon, as `fs_reflection` composes them.
fn ssr_ray(point_position: Vec3, normal: Vec3) -> Vec3 {
    let view = (EYE - point_position).normalize();
    reflection_above_horizon(reflection_mirror(view, normal))
}

// AC 10, CPU half. For the same cell, clock and texels per pixel, the planar
// pass's tilted normal is SSR's, bit for bit, at every cell; its ray is SSR's
// wherever the probe box's rim pull leaves the centre in place; at a rim cell
// the planar ray starts from the unpulled centre (D3) and is the ray from
// there. Over the sample some cells tilt, some rim cells move, and some
// offsets fade flat.
#[test]
fn planar_tilt_matches_ssr_normal_and_unpulled_ray() {
    let origin = Vec3::new(-2.0, 0.0, -1.0);
    let (mut cells, mut tilted, mut kept, mut rim, mut faded) = (0, 0, 0, 0, 0);
    for row in 0..40 {
        for column in 0..40 {
            let texel_uv = Vec2::new(column as f32 + 0.31, row as f32 + 0.62);
            let world = world_at(texel_uv, origin);
            for (time, texels_per_pixel) in [(0.0, 0.1), (1.37, 0.4), (7.9, 0.8), (63.2, 2.0)] {
                cells += 1;
                let planar = planar_tilt_ray(
                    texel_uv,
                    world,
                    Vec3::Y,
                    axes(),
                    EYE,
                    time,
                    texels_per_pixel,
                    1,
                );
                let ssr = reflection_point(
                    FLUID_REFLECTION_STYLE_PIXEL,
                    texel_uv,
                    world,
                    Vec3::Y,
                    axes(),
                    time,
                    texels_per_pixel,
                    BOX_MIN,
                    BOX_MAX,
                    CAPTURE,
                );
                assert_eq!(planar.cell_normal, ssr.normal, "cell {texel_uv} at {time}");
                let offset = displayed_offset(texel_uv.floor().as_ivec2(), time, texels_per_pixel);
                if offset != IVec2::ZERO {
                    tilted += 1;
                    assert_ne!(planar.cell_normal, Vec3::Y);
                } else if texels_per_pixel > 1.5 {
                    faded += 1;
                }
                let centre = reflection_cell_centre(texel_uv, world, axes());
                assert_eq!(planar.cell_centre, centre, "the unpulled centre");
                let pulled = reflection_cell_within_box(centre, axes(), BOX_MIN, BOX_MAX, CAPTURE);
                assert_eq!(pulled, ssr.position);
                if pulled == centre {
                    kept += 1;
                    assert_eq!(planar.tilted_ray, ssr_ray(ssr.position, ssr.normal));
                } else {
                    rim += 1;
                    assert_eq!(planar.tilted_ray, ssr_ray(centre, ssr.normal));
                    assert_ne!(planar.cell_centre, ssr.position, "rim cell {texel_uv}");
                }
            }
        }
    }
    eprintln!(
        "[PlanarTilt] {cells} cells: {tilted} tilted, {kept} kept, {rim} at the rim, {faded} faded"
    );
    assert!(tilted > cells / 5 && kept > cells / 2 && rim > 40 && faded > 100);
}

/// A mirror camera over the plane at height 0 through a 60° square view
/// from `eye` toward `target`, at `extent` texels.
fn mirror(eye: Vec3, target: Vec3, extent: (u32, u32)) -> (MirrorCamera, PlanarMirrorView) {
    let projection =
        glam::camera::rh::proj::directx::perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y);
    let camera = MirrorCamera::new(eye, view, projection, 0.0, None).expect("eye above the plane");
    (camera, PlanarMirrorView::new(camera.raster_vp, extent))
}

/// Mirror depth of a flat ceiling at height `ceiling` over the whole view,
/// as the mirror draws it: the real-world point a texel shows, projected.
fn ceiling_depth(mirror: &PlanarMirrorView, ceiling: f32) -> impl Fn(IVec2) -> f32 + '_ {
    move |texel| {
        let centre = (texel.as_vec2() + 0.5) / mirror.extent;
        let ndc = Vec2::new(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0);
        let near = mirror.inverse_raster_vp.project_point3(ndc.extend(0.5));
        let far = mirror.inverse_raster_vp.project_point3(ndc.extend(0.9));
        let dir = far - near;
        let point = near + dir * ((ceiling - near.y) / dir.y);
        let clip = mirror.raster_vp * point.extend(1.0);
        clip.z / clip.w
    }
}

// Smooth style tilts nothing (D9): at every clock and offset multiplier the
// fragment reads its own mirror texel, measured from itself. Pixel style
// with the offset forced to zero reads its cell centre's own texel: the
// reprojection at the untilted depth lands back where it started.
#[test]
fn planar_tilt_smooth_tilts_nothing_and_zero_offset_reads_the_untilted_texel() {
    let eye = Vec3::new(0.0, 2.0, 0.0);
    let (_, view) = mirror(eye, Vec3::new(0.0, 0.5, 5.0), (64, 64));
    let depth = ceiling_depth(&view, 4.0);
    let axes = FluidTexelAxes {
        u: Vec3::X * 0.25,
        v: Vec3::Z * 0.25,
    };
    let (mut same, mut tilted_away) = (0, 0);
    for row in 0..24 {
        for column in -12..12 {
            let texel_uv = Vec2::new(column as f32 + 0.4, row as f32 + 8.3);
            let world = Vec3::new(texel_uv.x * 0.25, 0.0, texel_uv.y * 0.25);
            for time in [0.0, 0.9, 3.3, 11.1] {
                for scale in [0, 1] {
                    let smooth = planar_resolve_texel(
                        FLUID_REFLECTION_STYLE_SMOOTH,
                        texel_uv,
                        world,
                        Vec3::Y,
                        axes,
                        eye,
                        time,
                        0.1,
                        scale,
                        &view,
                        &depth,
                    )
                    .expect("smooth always samples");
                    assert_eq!(smooth, planar_mirror_texel(world, &view));
                }
                let centre = reflection_cell_centre(texel_uv, world, axes);
                let untilted = planar_mirror_texel(centre, &view);
                let zero = planar_resolve_texel(
                    FLUID_REFLECTION_STYLE_PIXEL,
                    texel_uv,
                    world,
                    Vec3::Y,
                    axes,
                    eye,
                    time,
                    0.1,
                    0,
                    &view,
                    &depth,
                )
                .expect("the ceiling covers every texel");
                // Off by a texel only where the cell centre sits on a texel
                // boundary to within rounding.
                assert!((zero - untilted).abs().max_element() <= 1);
                same += usize::from(zero == untilted);
                let tilted = planar_resolve_texel(
                    FLUID_REFLECTION_STYLE_PIXEL,
                    texel_uv,
                    world,
                    Vec3::Y,
                    axes,
                    eye,
                    time,
                    0.1,
                    1,
                    &view,
                    &depth,
                )
                .expect("the ceiling covers every texel");
                tilted_away += usize::from(tilted != untilted);
            }
        }
    }
    eprintln!("[PlanarTilt] zero offset kept {same} untilted texels; tilt moved {tilted_away}");
    assert!(same >= 24 * 24 * 4 * 99 / 100 && tilted_away > 100);
}

// The reprojection clamps, never decides (D2): a point behind the mirror eye
// or far off its frustum lands on the target's edge, and an untilted texel
// with no content (the clear, or the mask's far value) samples nothing.
#[test]
fn planar_tilt_reprojection_clamps_and_an_empty_untilted_texel_samples_nothing() {
    let eye = Vec3::new(0.0, 2.0, 0.0);
    let (camera, view) = mirror(eye, Vec3::new(0.0, 0.5, 5.0), (40, 30));
    for point in [
        camera.eye - Vec3::new(0.0, 0.0, 3.0),
        Vec3::new(1.0e6, 3.0, 2.0),
        Vec3::new(-1.0e6, 3.0, 2.0),
        Vec3::new(0.0, 1.0e6, 2.0),
        camera.eye,
    ] {
        let texel = planar_mirror_texel(point, &view);
        assert!(
            texel.cmpge(IVec2::ZERO).all() && texel.cmplt(IVec2::new(40, 30)).all(),
            "{point} lands on the target: {texel}"
        );
    }
    let axes = FluidTexelAxes {
        u: Vec3::X * 0.25,
        v: Vec3::Z * 0.25,
    };
    for empty in [0.0, 1.0] {
        let sample = planar_resolve_texel(
            FLUID_REFLECTION_STYLE_PIXEL,
            Vec2::new(1.5, 14.5),
            Vec3::new(0.375, 0.0, 3.625),
            Vec3::Y,
            axes,
            eye,
            2.0,
            0.1,
            1,
            &view,
            |_| empty,
        );
        assert_eq!(sample, None, "depth {empty} holds no content");
    }
    assert!(planar_covered(0.5) && !planar_covered(0.0) && !planar_covered(1.0));
}
