// Slab containment: on oblique charts far from the world origin, every
// shadow-trace start the sampler builds lies in its texel's slab box.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::f32::consts::TAU;

use glam::{DVec3, Vec3};

use super::flood::Emitter;
use super::slab::ChartFrame;
use super::test_fixtures::point_light;
use crate::chart_raster::{
    CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_seed, chart_texel_world_position,
};
use crate::lightmap_bake::{Chart, RAY_EPSILON, area_sample_target, hard_ray_target};
use crate::map_data::MapLight;
use crate::texel_footprint::TexelSampler;

/// The footprint's sample count: the walk's offsets and the emitter lattice
/// its soft targets come from.
const SAMPLES: u32 = 32;

/// An oblique chart, its normal off every axis, with unequal texel steps
/// that are not powers of two, and `interior` texels. Its texels sit by
/// `first`; its origin lies `back` metres behind `first` along its axes,
/// with its UV rect shifted to match, so `back` moves only the origin.
fn oblique_chart(first: Vec3, back: [f32; 2], interior: [u32; 2]) -> Chart {
    let normal = Vec3::new(0.62, -0.47, 0.81).normalize();
    let u_axis = normal.cross(Vec3::new(0.3, 0.9, 0.1)).normalize();
    let v_axis = normal.cross(u_axis).normalize();
    let step = [0.137f32, 0.211];
    let padding = 2 * CHART_PADDING_TEXELS;
    Chart {
        origin: first - u_axis * back[0] - v_axis * back[1],
        u_axis,
        v_axis,
        uv_min: [back[0] - 1.73, back[1] + 0.41],
        uv_extent: [interior[0] as f32 * step[0], interior[1] as f32 * step[1]],
        normal,
        width_texels: interior[0] + padding,
        height_texels: interior[1] + padding,
        leaf_index: 0,
        window: None,
    }
}

/// Emitters about the chart's middle, hard and soft: straight above, and
/// low over the plane from several sides, so trace directions run from
/// along the normal to nearly in the plane.
fn lights(chart: &Chart) -> Vec<MapLight> {
    let (width, height) = chart_interior_dims(chart);
    let middle = chart_texel_world_position(chart, width / 2, height / 2).as_dvec3();
    let (u, v, n) = (
        chart.u_axis.as_dvec3(),
        chart.v_axis.as_dvec3(),
        chart.normal.as_dvec3(),
    );
    [
        (n * 2.0, 0.0),
        (n * 2.0, 0.5),
        (u * 6.0 + n * 0.05, 0.0),
        (v * -6.0 + n * 0.05, 0.0),
        (u * -6.0 + v * 3.0 + n * 0.6, 0.5),
        (u * 2.0 + v * 9.0 + n * 0.3, 0.2),
    ]
    .into_iter()
    .map(|(offset, size): (DVec3, f32)| {
        let mut light = point_light(middle + offset, 20.0);
        light.light_size = size;
        light
    })
    .collect()
}

/// Every footprint offset the walk adds to a texel centre, in texel units:
/// the light's own sample set.
fn footprint_offsets(light: &MapLight) -> Vec<[f32; 2]> {
    match TexelSampler::new(light, SAMPLES, true, false) {
        TexelSampler::Footprint { footprint, .. } => footprint.offsets().to_vec(),
        TexelSampler::Centre(_) => unreachable!("the floor is on"),
    }
}

/// The texel's own footprint rotation, as `footprint_rotation` takes it
/// from the seed's high half, and a sweep of others.
fn rotations(seed: u64) -> Vec<(f32, f32)> {
    let turn = (seed >> 32) as u32 as f32 / u32::MAX as f32;
    std::iter::once(turn * TAU)
        .chain((0..6).map(|i| i as f32 * TAU / 6.0 + 0.1))
        .map(f32::sin_cos)
        .collect()
}

/// The point `segment_clear` starts tracing `from → to` at: one
/// `RAY_EPSILON` along the segment. `None` when the segment is shorter.
fn trace_start(from: Vec3, to: Vec3) -> Option<Vec3> {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return None;
    }
    Some(from + delta / length * RAY_EPSILON)
}

/// The worst excursion of a trace start past its texel box, in metres, on
/// one frame axis, with where it happened.
struct Worst {
    excess: f64,
    at: Option<(i32, i32, usize, Vec3)>,
}

/// Every shadow-trace start of every texel of `chart` toward `light`,
/// with the floor on or off, checked against its texel box; the worst
/// excursion past one.
fn worst_excursion(chart: &Chart, light: &MapLight, penumbra_floor: bool) -> Worst {
    let frame = ChartFrame::new(chart, penumbra_floor).expect("an orthonormal frame");
    assert!(
        frame.emitter_clears(Emitter::of(light)),
        "the emitter clears the slab"
    );
    let offsets = if penumbra_floor {
        footprint_offsets(light)
    } else {
        // The centre sampler traces from the texel centre alone.
        vec![[0.0, 0.0]]
    };
    let (width, height) = chart_interior_dims(chart);
    // `TexelFrame::for_chart`'s parent-grid steps: this chart is its own grid.
    let pitch = [
        chart.u_axis * (chart.uv_extent[0] / width as f32),
        chart.v_axis * (chart.uv_extent[1] / height as f32),
    ];
    let lift = chart.normal * RAY_EPSILON;
    let mut worst = Worst {
        excess: f64::NEG_INFINITY,
        at: None,
    };
    for ty in 0..height {
        for tx in 0..width {
            let world_p = chart_texel_world_position(chart, tx, ty);
            let seed = chart_texel_seed(chart, tx, ty);
            let texel_box = frame.texel_box(tx, ty);
            for (sin, cos) in rotations(seed) {
                for (k, &[x, y]) in offsets.iter().enumerate() {
                    let point = if k == 0 {
                        world_p
                    } else {
                        world_p + (pitch[0] * (x * cos - y * sin) + pitch[1] * (x * sin + y * cos))
                    };
                    let origin = point + lift;
                    // The centre traces to the emitter's centre; a soft
                    // emitter's other samples to any point of its lattice.
                    let targets: Vec<Vec3> = if k == 0 || light.light_size <= 0.0 {
                        vec![hard_ray_target(point, light)]
                    } else {
                        (0..SAMPLES)
                            .map(|i| area_sample_target(point, light, seed, i, SAMPLES))
                            .collect()
                    };
                    for target in targets {
                        let start = trace_start(origin, target)
                            .expect("a target clearing the slab lies over RAY_EPSILON away");
                        assert!(
                            target.distance(start) > RAY_EPSILON,
                            "a target clearing the slab lies over RAY_EPSILON past the start"
                        );
                        let p = frame.frame_coords(start.as_dvec3());
                        let excess = (texel_box.lo - p).max(p - texel_box.hi);
                        for axis in 0..3 {
                            if excess[axis] > worst.excess {
                                worst = Worst {
                                    excess: excess[axis],
                                    at: Some((tx, ty, axis, start)),
                                };
                            }
                        }
                    }
                }
            }
        }
    }
    worst
}

/// Assert every shadow-trace start of every texel of `chart`, toward each
/// of its [`lights`], floor on and off, lies in its texel's box.
fn assert_slab_holds(chart: &Chart) {
    for light in lights(chart) {
        for penumbra_floor in [true, false] {
            let worst = worst_excursion(chart, &light, penumbra_floor);
            assert!(
                worst.excess <= 0.0,
                "chart at {:?}, light at {:?} size {}, floor {penumbra_floor}: a trace start \
                 lies {:.3e} m past its texel box at (tx, ty, frame axis, start) {:?}",
                chart.origin,
                light.origin,
                light.light_size,
                worst.excess,
                worst.at
            );
        }
    }
}

/// Where the far-from-origin charts sit: just past 1024 m on x and z, where
/// `f32` spacing doubles.
const FAR: Vec3 = Vec3::new(1030.17, -1104.29, 1031.53);

#[test]
fn reach_slab_holds_every_trace_start_of_an_oblique_chart_far_from_the_origin() {
    assert_slab_holds(&oblique_chart(FAR, [0.0, 0.0], [48, 37]));
}

// The walk's centres drift from their exact affine positions by the `f32`
// rounding of `origin + u·local_u + v·local_v`. With the origin 11 km back
// along the plane those terms are large and the drift, about 4e-4 m, as
// large as the rounding bound on the adds after the centre, so the box
// holds the starts only through the measured drift, on the side it falls.
#[test]
fn reach_slab_holds_the_trace_starts_of_a_chart_whose_centres_drift() {
    assert_slab_holds(&oblique_chart(FAR, [9000.0, 7000.0], [3, 2]));
}

// The rounding bound on a trace start scales with its distance from the
// world origin. Charts on each axis plane, far out on the other two, fail a
// bound read from any single coordinate.
#[test]
fn reach_slab_holds_the_trace_starts_of_charts_near_each_axis_plane() {
    for first in [
        Vec3::new(40.3, -1504.7, 1403.1),
        Vec3::new(1403.1, -40.3, 1504.7),
        Vec3::new(1504.7, -1403.1, 40.3),
    ] {
        assert_slab_holds(&oblique_chart(first, [0.0, 0.0], [8, 6]));
    }
}
