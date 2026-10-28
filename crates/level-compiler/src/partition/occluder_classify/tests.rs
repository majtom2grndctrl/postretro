use super::*;
use crate::map_data::BrushPlane;
use crate::partition::build_bsp_from_brushes;

/// Box brush in its own frame: `half` extents about `center`, turned `yaw`
/// radians about engine up.
fn turned_box(center: DVec3, half: DVec3, yaw: f64) -> BrushVolume {
    let (sin, cos) = yaw.sin_cos();
    let x = DVec3::new(cos, 0.0, -sin);
    let z = DVec3::new(sin, 0.0, cos);
    let mut planes = Vec::new();
    for (axis, extent) in [(x, half.x), (DVec3::Y, half.y), (z, half.z)] {
        for normal in [axis, -axis] {
            planes.push(BrushPlane {
                normal,
                distance: center.dot(normal) + extent,
            });
        }
    }
    let reach = half.length();
    let mut polytope = RegionPolytope::from_aabb(&Aabb {
        min: center - DVec3::splat(reach + 1.0),
        max: center + DVec3::splat(reach + 1.0),
    });
    for plane in &planes {
        polytope = polytope.clip(plane.normal, plane.distance).1;
    }
    BrushVolume {
        planes,
        sides: Vec::new(),
        aabb: polytope.vertex_aabb(),
    }
}

fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    turned_box((min + max) * 0.5, (max - min) * 0.5, 0.0)
}

/// Sealed hall of 0.5 m slabs: floor top at y = 0, interior `size`.
fn hall(size: DVec3) -> Vec<BrushVolume> {
    let t = 0.5;
    let (lo, hi) = (DVec3::new(0.0, 0.0, 0.0), size);
    vec![
        box_brush(DVec3::new(-t, -t, -t), DVec3::new(hi.x + t, lo.y, hi.z + t)),
        box_brush(
            DVec3::new(-t, hi.y, -t),
            DVec3::new(hi.x + t, hi.y + t, hi.z + t),
        ),
        box_brush(DVec3::new(-t, lo.y, -t), DVec3::new(lo.x, hi.y, hi.z + t)),
        box_brush(
            DVec3::new(hi.x, lo.y, -t),
            DVec3::new(hi.x + t, hi.y, hi.z + t),
        ),
        box_brush(DVec3::new(lo.x, lo.y, -t), DVec3::new(hi.x, hi.y, lo.z)),
        box_brush(
            DVec3::new(lo.x, lo.y, hi.z),
            DVec3::new(hi.x, hi.y, hi.z + t),
        ),
    ]
}

/// Classify `extra` standing in `hall_brushes`; verdicts for `extra` only.
fn classify_in(hall_brushes: Vec<BrushVolume>, extra: &[BrushVolume]) -> Vec<OccluderVerdict> {
    let first_extra = hall_brushes.len();
    let mut brushes = hall_brushes;
    brushes.extend_from_slice(extra);
    let tree = build_bsp_from_brushes(&brushes).expect("first-pass tree builds");
    let exterior = HashSet::new();
    let space = OpenSpace::new(&tree, &exterior);
    classify_occluders(&brushes, &space).verdicts[first_extra..].to_vec()
}

#[test]
fn retaining_wall_occludes_and_red_pillar_stays_exempt_in_a_sealed_hall() {
    // Hall 52 m wide (x), 13 m tall, 60 m long (z); the wall crosses the
    // middle 26 m, a pillar stands beyond it.
    let wall = box_brush(DVec3::new(13.0, 0.0, 30.0), DVec3::new(39.0, 6.5, 30.81));
    let pillar = box_brush(DVec3::new(20.0, 0.0, 40.0), DVec3::new(21.016, 3.7, 41.016));

    let verdicts = classify_in(hall(DVec3::new(52.0, 13.0, 60.0)), &[wall, pillar]);

    assert_eq!(
        verdicts,
        vec![OccluderVerdict::Occludes, OccluderVerdict::UnderFloor]
    );
}

#[test]
fn wall_top_decides_against_the_see_over_height() {
    let low = box_brush(DVec3::new(13.0, 0.0, 30.0), DVec3::new(39.0, 2.4, 30.8));
    let high = box_brush(DVec3::new(13.0, 0.0, 30.0), DVec3::new(39.0, 2.6, 30.8));

    let hall_size = DVec3::new(52.0, 13.0, 60.0);
    assert_eq!(
        classify_in(hall(hall_size), std::slice::from_ref(&low)),
        vec![OccluderVerdict::LowTop]
    );
    assert_eq!(
        classify_in(hall(hall_size), std::slice::from_ref(&high)),
        vec![OccluderVerdict::Occludes]
    );
}

#[test]
fn short_wall_passes_on_half_its_open_span_only() {
    // A 12 m wall, under the absolute length, across rooms 20 m and 30 m wide.
    let wall = |room_x: f64| {
        let x0 = (room_x - 12.0) * 0.5;
        box_brush(DVec3::new(x0, 0.0, 15.0), DVec3::new(x0 + 12.0, 4.0, 15.3))
    };

    assert_eq!(
        classify_in(hall(DVec3::new(20.0, 6.0, 30.0)), &[wall(20.0)]),
        vec![OccluderVerdict::Occludes]
    );
    assert_eq!(
        classify_in(hall(DVec3::new(30.0, 6.0, 30.0)), &[wall(30.0)]),
        vec![OccluderVerdict::ShortSpan]
    );
}

#[test]
fn turned_brush_measures_in_its_own_frame() {
    // The 25 m curb turned 45° in plan: its world AABB is a 17.9 m slab, its
    // own frame the 25 × 0.3 × 0.3 m box it was authored as.
    let curb = turned_box(
        DVec3::new(15.0, 0.15, 15.0),
        DVec3::new(12.5, 0.15, 0.15),
        std::f64::consts::FRAC_PI_4,
    );

    let frame = BrushFrame::of(&curb).expect("curb has a frame");
    let mut extents = frame.extents;
    extents.sort_by(f64::total_cmp);
    for (got, want) in extents.iter().zip([0.3, 0.3, 25.0]) {
        assert!((got - want).abs() < 1e-9, "extents {extents:?}");
    }
    assert_eq!(
        classify_in(hall(DVec3::new(30.0, 6.0, 30.0)), &[curb]),
        vec![OccluderVerdict::UnderFloor]
    );
}
