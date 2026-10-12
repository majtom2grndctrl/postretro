// Camera-eye fluid query for presentation: which fluid the eye is in and its signed depth there.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md §Decisions (Presentation)

use glam::Vec3;

use crate::resolve::{EnvironmentView, fluid_winner_at_point};

/// The fluid the camera eye is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeFluid {
    /// The fluid index of the winning volume.
    pub fluid: u16,
    /// The winning volume's map-order index. Feed it back to
    /// [`fluid_volume_signed_depth`] on later frames to measure the eye against
    /// the same volume once it has left it.
    pub volume: usize,
    /// How far inside the union of the volume's brushes the eye is, metres;
    /// always >= 0 here. Semantics as in
    /// [`EnvironmentVolumes::signed_depth`](crate::EnvironmentVolumes::signed_depth).
    pub signed_depth: f32,
}

/// The fluid at the presented camera eye: the overlap-rule winner among fluid
/// volumes one of whose brushes contains `eye` (a face counts as inside), or
/// `None`. This is a presentation query; movement keys on the resolve's tick eye
/// instead (`research.md` P5).
///
/// Hysteresis for tint and muffle: while off, turn on when this returns a fluid
/// whose `signed_depth >= margin`; while on in volume `v`, turn off when
/// [`fluid_volume_signed_depth`]`(view, v, eye) <= -margin`. Both depth signs are
/// conservative (never deeper or farther than the true boundary distance), so
/// the band is at least `2 * margin` wide and never flickers.
pub fn fluid_at_eye(view: EnvironmentView<'_>, eye: Vec3) -> Option<EyeFluid> {
    let volumes = view.volumes;
    let volume = fluid_winner_at_point(volumes, eye)?;
    let fluid = volumes.volume(volume)?.fluid?;
    let signed_depth = volumes.signed_depth(volume, eye)?.max(0.0);
    Some(EyeFluid {
        fluid,
        volume,
        signed_depth,
    })
}

/// Signed depth of `point` relative to fluid volume `volume`, metres: positive
/// inside the union of its brushes, zero on its boundary, negative outside.
/// `None` when `volume` is out of range, sets no fluid, or has no brushes.
/// Semantics as in
/// [`EnvironmentVolumes::signed_depth`](crate::EnvironmentVolumes::signed_depth).
pub fn fluid_volume_signed_depth(
    view: EnvironmentView<'_>,
    volume: usize,
    point: Vec3,
) -> Option<f32> {
    let volumes = view.volumes;
    volumes.volume(volume)?.fluid?;
    volumes.signed_depth(volume, point)
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
        let view = EnvironmentView::new(-9.81, &volumes);

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
        let view = EnvironmentView::new(-9.81, &volumes);
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

    #[test]
    fn eye_depth_is_conservative_inside_a_two_brush_pool_and_at_tilted_faces() {
        // Two brushes sharing the face x = 0: the union's depth at the seam is
        // understated, never overstated.
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
        let view = EnvironmentView::new(-9.81, &volumes);

        let seam = fluid_at_eye(view, Vec3::new(0.0, 2.0, 0.0)).unwrap();
        assert!(seam.signed_depth >= 0.0 && seam.signed_depth <= 2.0 + EPS);
        let off_seam = fluid_at_eye(view, Vec3::new(-2.0, 2.0, 0.0)).unwrap();
        assert!((off_seam.signed_depth - 2.0).abs() < EPS);

        // The tilted face is normalized: (11, 1.5) is 0.5/sqrt(2) inside it.
        let tilted = fluid_at_eye(view, Vec3::new(11.0, 1.5, 1.0)).unwrap();
        assert_eq!(tilted.fluid, 2);
        assert!((tilted.signed_depth - 0.5 / 2.0_f32.sqrt()).abs() < EPS);
    }

    #[test]
    fn eye_query_ignores_dry_volumes_and_rejects_bad_indices() {
        let gravity_only = EnvironmentVolume {
            gravity: Some(-2.0),
            ..volume(vec![axis_box(Vec3::ZERO, Vec3::ONE)])
        };
        let volumes = EnvironmentVolumes::build(vec![gravity_only]);
        let view = EnvironmentView::new(-9.81, &volumes);
        assert_eq!(fluid_at_eye(view, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_signed_depth(view, 0, Vec3::splat(0.5)), None);
        assert_eq!(fluid_volume_signed_depth(view, 7, Vec3::splat(0.5)), None);
    }
}
