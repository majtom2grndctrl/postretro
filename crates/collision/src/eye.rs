// Camera-eye fluid query for presentation: which fluid the eye is in and its signed depth there.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md §Decisions (Presentation)

use glam::Vec3;

use crate::crossings::CROSSING_SEAM_EPSILON;
use crate::leaf::ContentsMask;
use crate::line::{BrushInterval, clip_line, for_each_run};
use crate::resolve::{EnvironmentView, fluid_winner_at_point};
use crate::volumes::EnvironmentVolumes;

/// How far the eye depth looks along the vertical, up and down from the eye,
/// for the fluid's union, metres. Depths are capped at it, and a vertical line
/// meeting no brush of the fluid within it has no depth.
pub const EYE_DEPTH_REACH: f32 = 2.0;

/// The fluid the camera eye is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeFluid {
    /// The fluid index of the winning volume.
    pub fluid: u16,
    /// The winning volume's map-order index. Feed it back to
    /// [`fluid_volume_signed_depth`] and [`fluid_volume_plane_depth`] on later
    /// frames to measure the eye against the same fluid once it has left it.
    pub volume: usize,
    /// The eye's vertical depth in the fluid, metres, always >= 0 here:
    /// [`fluid_volume_signed_depth`] at the eye.
    pub signed_depth: f32,
}

/// The fluid at the presented camera eye: the overlap-rule winner among fluid
/// volumes one of whose brushes contains `eye` (a face counts as inside), or
/// `None`. A non-finite eye is in no fluid. This is a presentation query;
/// movement keys on the resolve's tick eye instead (`research.md` P5).
///
/// Hysteresis for tint and muffle: while off, turn on when this returns a
/// fluid whose `signed_depth >= margin`. While on in a fluid, turn off when
/// [`fluid_volume_plane_depth`] reads `None` or `<= -margin`, and move to
/// another fluid winning at the eye once its plane depth there is `>= margin`.
/// The vertical depth is exact through any seam of the fluid's union, where
/// the plane depth understates; the plane depth never reads farther out than
/// the true distance, and outside a union no seam understates it, so the off
/// test holds on every side and the band never flickers.
pub fn fluid_at_eye(view: EnvironmentView<'_>, eye: Vec3) -> Option<EyeFluid> {
    if !eye.is_finite() {
        return None;
    }
    let volumes = view.volumes;
    let volume = fluid_winner_at_point(volumes, eye)?;
    let fluid = volumes.volume(volume)?.fluid?;
    let signed_depth =
        vertical_fluid_depth(volumes, fluid, eye).map_or(0.0, |depth| depth.max(0.0));
    Some(EyeFluid {
        fluid,
        volume,
        signed_depth,
    })
}

/// Vertical signed depth of `point` in the union of the brushes of every volume
/// carrying volume `volume`'s fluid, metres. Inside, the distance up from the
/// point to where the vertical line through it leaves the union (the surface
/// above); outside, minus the distance along that line to the nearest point of
/// the union, above or below. Capped at [`EYE_DEPTH_REACH`]; `None` when the
/// line meets no brush of the fluid within that reach, when `volume` is out of
/// range or sets no fluid, or for a non-finite point. Seams between brushes or
/// entities of one fluid never read as a surface: covered runs join across a
/// gap within [`CROSSING_SEAM_EPSILON`] of the union.
pub fn fluid_volume_signed_depth(
    view: EnvironmentView<'_>,
    volume: usize,
    point: Vec3,
) -> Option<f32> {
    if !point.is_finite() {
        return None;
    }
    let volumes = view.volumes;
    let fluid = volumes.volume(volume)?.fluid?;
    vertical_fluid_depth(volumes, fluid, point)
}

/// Conservative signed depth of `point` in the union of the brushes of every
/// volume carrying volume `volume`'s fluid, metres: the largest
/// smallest-normalized-plane-distance over those brushes whose bounds lie
/// within [`EYE_DEPTH_REACH`] of it. Positive inside, negative outside, and
/// never farther from zero than the true distance to the union's boundary, so
/// it understates depth near an internal seam. It measures leaving through a
/// side, where the vertical line misses the fluid. `None` when no brush of the
/// fluid is that close, when `volume` is out of range or sets no fluid, or for
/// a non-finite point.
pub fn fluid_volume_plane_depth(
    view: EnvironmentView<'_>,
    volume: usize,
    point: Vec3,
) -> Option<f32> {
    if !point.is_finite() {
        return None;
    }
    let volumes = view.volumes;
    let fluid = volumes.volume(volume)?.fluid?;
    volumes.fluid_plane_depth(fluid, point, EYE_DEPTH_REACH)
}

/// The vertical depth itself, over the line `[point.y - reach, point.y + reach]`.
fn vertical_fluid_depth(volumes: &EnvironmentVolumes, fluid: u16, point: Vec3) -> Option<f32> {
    let bottom = point - Vec3::new(0.0, EYE_DEPTH_REACH, 0.0);
    let length = 2.0 * EYE_DEPTH_REACH;
    // The point's own parameter, rounded the way every clip rounds its planes'
    // parameters from `bottom`, so a point a brush contains lies in its run.
    let at = point.y - bottom.y;
    let mut inside: Option<f32> = None;
    let mut nearest_outside: Option<f32> = None;
    for_each_run(
        |sink| {
            volumes.visit_brushes_on_line(
                bottom,
                Vec3::Y,
                0.0,
                length,
                ContentsMask::FLUID,
                |index, brush, leaf| {
                    if volumes.volume(index).and_then(|v| v.fluid) != Some(fluid) {
                        return;
                    }
                    if let Some(interval) = clip_line(&leaf.planes, bottom, Vec3::Y, 0.0, length) {
                        sink(BrushInterval { brush, interval });
                    }
                },
            )
        },
        |gap_start, gap_end| {
            let midpoint = bottom + Vec3::new(0.0, 0.5 * (gap_start + gap_end), 0.0);
            volumes.fluid_is_near(fluid, midpoint, CROSSING_SEAM_EPSILON)
        },
        |run| {
            if run.start <= at && at <= run.end {
                inside = Some(run.end - at);
            } else {
                let gap = if run.end < at {
                    at - run.end
                } else {
                    run.start - at
                };
                nearest_outside = Some(nearest_outside.map_or(gap, |nearest| nearest.min(gap)));
            }
        },
    );
    inside.or(nearest_outside.map(|gap| -gap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leaf::LeafPlane;
    use crate::test_fixtures::{axis_box, volume};
    use crate::volumes::{EnvironmentVolume, EnvironmentVolumes};

    const EPS: f32 = 1.0e-5;
    const MARGIN: f32 = 0.05;

    fn pool(min: Vec3, max: Vec3, fluid: u16) -> EnvironmentVolume {
        EnvironmentVolume {
            fluid: Some(fluid),
            ..volume(vec![axis_box(min, max)])
        }
    }

    #[test]
    fn eye_depth_reads_distance_below_and_above_a_pool_top() {
        let volumes = EnvironmentVolumes::build(vec![pool(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 2.0, 5.0),
            3,
        )]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);

        let under = fluid_at_eye(view, Vec3::new(0.0, 1.9, 0.0)).expect("eye under the surface");
        assert_eq!(under.fluid, 3);
        assert_eq!(under.volume, 0);
        assert!((under.signed_depth - 0.1).abs() < EPS);

        let on_surface = fluid_at_eye(view, Vec3::new(0.0, 2.0, 0.0)).expect("a face is inside");
        assert!(on_surface.signed_depth.abs() < EPS);

        assert_eq!(fluid_at_eye(view, Vec3::new(0.0, 2.1, 0.0)), None);
        let above = fluid_volume_signed_depth(view, 0, Vec3::new(0.0, 2.1, 0.0)).unwrap();
        assert!((above + 0.1).abs() < EPS, "0.1 m above the top reads -0.1");
    }

    #[test]
    fn eye_hysteresis_band_switches_once_across_a_bob() {
        // An eye bobbing ±4 cm about the surface stays inside the ±5 cm band and
        // never switches; one sweeping ±10 cm switches on once and off once.
        let volumes = EnvironmentVolumes::build(vec![pool(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 2.0, 5.0),
            0,
        )]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);
        let toggles = |amplitude: f32| {
            let mut on: Option<usize> = None;
            let mut toggles = 0;
            for step in 0..400 {
                let phase = step as f32 / 400.0 * std::f32::consts::TAU;
                let eye = Vec3::new(0.0, 2.0 - amplitude * phase.sin(), 0.0);
                match on {
                    None => {
                        if let Some(hit) = fluid_at_eye(view, eye)
                            && hit.signed_depth >= MARGIN
                        {
                            on = Some(hit.volume);
                            toggles += 1;
                        }
                    }
                    Some(volume) => {
                        if fluid_volume_signed_depth(view, volume, eye)
                            .is_none_or(|depth| depth <= -MARGIN)
                        {
                            on = None;
                            toggles += 1;
                        }
                    }
                }
            }
            toggles
        };
        assert_eq!(toggles(0.04), 0);
        assert_eq!(toggles(0.10), 2);
    }

    // Regression: the eye depth was the per-brush plane depth, which reads 0 at
    // a seam inside a pool, so an eye sinking at the seam never turned the tint on.
    #[test]
    fn eye_depth_reads_the_surface_above_through_a_seam_and_plane_depth_stays_conservative() {
        // Two brushes sharing the face x = 0.
        let two = EnvironmentVolume {
            fluid: Some(1),
            ..volume(vec![
                axis_box(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(0.0, 4.0, 4.0)),
                axis_box(Vec3::new(0.0, 0.0, -4.0), Vec3::new(4.0, 4.0, 4.0)),
            ])
        };
        let mut wedge = axis_box(Vec3::new(10.0, 0.0, 0.0), Vec3::new(12.0, 2.0, 2.0));
        // An unnormalized tilted face: x + y <= 13.
        wedge.planes.push(LeafPlane {
            normal: Vec3::new(2.0, 2.0, 0.0),
            distance: 26.0,
        });
        let wedge = EnvironmentVolume {
            fluid: Some(2),
            ..volume(vec![wedge])
        };
        let volumes = EnvironmentVolumes::build(vec![two, wedge]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);

        let seam = fluid_at_eye(view, Vec3::new(0.0, 3.5, 0.0)).unwrap();
        assert!((seam.signed_depth - 0.5).abs() < EPS, "{seam:?}");
        let off_seam = fluid_at_eye(view, Vec3::new(-2.0, 2.0, 0.0)).unwrap();
        assert!((off_seam.signed_depth - 2.0).abs() < EPS);
        // Deeper than the reach reads the reach.
        let deep = fluid_at_eye(view, Vec3::new(-2.0, 0.5, 0.0)).unwrap();
        assert!((deep.signed_depth - EYE_DEPTH_REACH).abs() < EPS);

        // The plane depth understates at the seam, never overstates.
        let plane = fluid_volume_plane_depth(view, 0, Vec3::new(0.0, 2.0, 0.0)).unwrap();
        assert!((0.0..=2.0 + EPS).contains(&plane));
        assert!(
            (fluid_volume_plane_depth(view, 0, Vec3::new(-6.0, 2.0, 0.0)).unwrap() + 2.0).abs()
                < EPS,
            "2 m beside the pool reads -2 m"
        );
        assert_eq!(
            fluid_volume_plane_depth(view, 0, Vec3::new(-7.0, 2.0, 0.0)),
            None,
            "past the reach there is no plane depth"
        );

        // Under the tilted face the vertical depth reaches it; the plane depth
        // is its normalized distance.
        let tilted = fluid_at_eye(view, Vec3::new(11.5, 1.0, 1.0)).unwrap();
        assert_eq!(tilted.fluid, 2);
        assert!((tilted.signed_depth - 0.5).abs() < EPS);
        let tilted_plane = fluid_volume_plane_depth(view, 1, Vec3::new(11.0, 1.5, 1.0)).unwrap();
        assert!((tilted_plane - 0.5 / 2.0_f32.sqrt()).abs() < EPS);
    }

    // Q3: the eye depth runs through every volume of the winner's fluid, never
    // through another fluid.
    #[test]
    fn eye_depth_runs_through_stacked_entities_of_one_fluid_and_stops_at_another_fluid() {
        let layer = |y_min: f32, y_max: f32, fluid: u16| {
            pool(
                Vec3::new(-5.0, y_min, -5.0),
                Vec3::new(5.0, y_max, 5.0),
                fluid,
            )
        };
        let eye = Vec3::new(0.0, 1.9, 0.0);

        let stacked = EnvironmentVolumes::build(vec![layer(0.0, 2.0, 1), layer(2.0, 3.5, 1)]);
        let view = EnvironmentView::new(-9.81, &stacked, &[]);
        let hit = fluid_at_eye(view, eye).unwrap();
        assert!((hit.signed_depth - 1.6).abs() < EPS, "{hit:?}");
        // From either entity, the same union.
        for volume in [0, 1] {
            let depth = fluid_volume_signed_depth(view, volume, eye).unwrap();
            assert!((depth - 1.6).abs() < EPS);
        }

        // A hairline gap of plane noise between the entities is no surface.
        let mut upper = layer(2.0, 3.5, 1);
        upper.brushes[0].planes[3].distance = -2.00002;
        let noisy = EnvironmentVolumes::build(vec![layer(0.0, 2.0, 1), upper]);
        let view = EnvironmentView::new(-9.81, &noisy, &[]);
        assert!((fluid_at_eye(view, eye).unwrap().signed_depth - 1.6).abs() < EPS);
        // A real gap is: 1 cm of air between the entities.
        let gapped = EnvironmentVolumes::build(vec![layer(0.0, 2.0, 1), layer(2.01, 3.5, 1)]);
        let view = EnvironmentView::new(-9.81, &gapped, &[]);
        assert!((fluid_at_eye(view, eye).unwrap().signed_depth - 0.1).abs() < EPS);
        let between = fluid_volume_signed_depth(view, 0, Vec3::new(0.0, 2.004, 0.0)).unwrap();
        assert!((between + 0.004).abs() < EPS, "nearest union point, below");

        let two_fluids = EnvironmentVolumes::build(vec![layer(0.0, 2.0, 1), layer(2.0, 3.5, 2)]);
        let view = EnvironmentView::new(-9.81, &two_fluids, &[]);
        let hit = fluid_at_eye(view, eye).unwrap();
        assert_eq!(hit.fluid, 1);
        assert!((hit.signed_depth - 0.1).abs() < EPS);
    }

    #[test]
    fn eye_queries_on_a_non_finite_eye_report_nothing() {
        let volumes = EnvironmentVolumes::build(vec![pool(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(5.0, 2.0, 5.0),
            0,
        )]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);
        for eye in [
            Vec3::new(f32::NAN, 1.0, 0.0),
            Vec3::new(0.0, f32::NAN, 0.0),
            Vec3::new(0.0, f32::INFINITY, 0.0),
            Vec3::new(f32::NEG_INFINITY, 1.0, 0.0),
        ] {
            assert_eq!(fluid_at_eye(view, eye), None, "{eye:?}");
            assert_eq!(fluid_volume_signed_depth(view, 0, eye), None, "{eye:?}");
            assert_eq!(fluid_volume_plane_depth(view, 0, eye), None, "{eye:?}");
        }
    }

    #[test]
    fn eye_query_ignores_dry_volumes_and_rejects_bad_indices() {
        let gravity_only = EnvironmentVolume {
            gravity: Some(-2.0),
            ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
        };
        let volumes = EnvironmentVolumes::build(vec![gravity_only]);
        let view = EnvironmentView::new(-9.81, &volumes, &[]);
        assert_eq!(fluid_at_eye(view, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_signed_depth(view, 0, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_signed_depth(view, 7, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_plane_depth(view, 0, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_plane_depth(view, 7, Vec3::splat(0.5)), None);
    }
}
