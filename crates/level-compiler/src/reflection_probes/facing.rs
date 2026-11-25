// Reflector facings: which way a probe's faces look, and the plane frame placement measures them in.
// See: context/lib/resource_management.md §5.1, §5.4

use std::fmt;

use glam::{DVec2, DVec3};

use super::fit_frame::FitFrame;
use super::placement::UP_FACING_MIN_COS;
use super::plane_basis::PlaneBasis;

/// Which way a probe's faces look. A probe binds faces of one facing, and its
/// capture point lifts off them along its normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Facing {
    /// Within 1° of up: water and glossy floors. Heights are `y` and plane
    /// coordinates `(x, z)`, the vertical projection water's placement has
    /// always measured, so up-facing placement is unchanged by the others.
    Up,
    /// Any other facing: walls, ceilings and slopes, in the frame of the
    /// facing's first face's normal.
    Other(PlaneBasis),
}

impl Facing {
    /// The facing's unit normal: exactly +Y when up.
    pub(crate) fn normal(&self) -> DVec3 {
        match self {
            Self::Up => DVec3::Y,
            Self::Other(basis) => basis.normal,
        }
    }

    pub(crate) fn is_up(&self) -> bool {
        matches!(self, Self::Up)
    }

    /// `p`'s height along the normal.
    pub(crate) fn height(&self, p: DVec3) -> f64 {
        match self {
            Self::Up => p.y,
            Self::Other(basis) => basis.height(p),
        }
    }

    /// `p`'s coordinates in the facing's plane.
    pub(crate) fn project(&self, p: DVec3) -> DVec2 {
        match self {
            Self::Up => DVec2::new(p.x, p.z),
            Self::Other(basis) => basis.project(p),
        }
    }

    /// The point at plane coordinates `q` and `height` along the normal.
    pub(crate) fn lift(&self, q: DVec2, height: f64) -> DVec3 {
        match self {
            Self::Up => DVec3::new(q.x, height, q.y),
            Self::Other(basis) => basis.lift(q, height),
        }
    }

    /// The two unit axes of the facing's plane: X and Z when up.
    pub(crate) fn in_plane_axes(&self) -> [DVec3; 2] {
        match self {
            Self::Up => [DVec3::X, DVec3::Z],
            Self::Other(basis) => basis.axes(),
        }
    }

    /// The frame the facing's box fit runs in.
    pub(crate) fn fit_frame(&self) -> FitFrame {
        match self {
            Self::Up => FitFrame::UP,
            Self::Other(basis) => FitFrame::nearest(basis.normal),
        }
    }
}

impl fmt::Display for Facing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Up => write!(f, "facing up"),
            Self::Other(basis) => {
                let n = basis.normal;
                write!(f, "facing ({:.3}, {:.3}, {:.3})", n.x, n.y, n.z)
            }
        }
    }
}

/// Assigns facings to normals in the order they come: a normal within 1° of
/// up ([`UP_FACING_MIN_COS`]) is [`Facing::Up`]; any other joins the first
/// facing seen whose normal lies within 1° of it, or starts its own. Faces
/// come in id order, so the facings never depend on anything else.
#[derive(Debug, Default)]
pub(crate) struct Facings {
    others: Vec<PlaneBasis>,
}

impl Facings {
    pub(crate) fn facing_of(&mut self, normal: DVec3) -> Facing {
        if normal.y >= UP_FACING_MIN_COS {
            return Facing::Up;
        }
        if let Some(basis) = self
            .others
            .iter()
            .find(|basis| basis.normal.dot(normal) >= UP_FACING_MIN_COS)
        {
            return Facing::Other(*basis);
        }
        let basis = PlaneBasis::new(normal);
        self.others.push(basis);
        Facing::Other(basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normals_within_a_degree_share_a_facing_and_opposite_ones_never_do() {
        let mut facings = Facings::default();
        let wall = facings.facing_of(DVec3::X);
        let tilt = |degrees: f64| {
            let (s, c) = degrees.to_radians().sin_cos();
            DVec3::new(c, s, 0.0)
        };
        assert_eq!(facings.facing_of(tilt(0.5)), wall);
        assert_ne!(facings.facing_of(tilt(2.0)), wall);
        assert_ne!(facings.facing_of(DVec3::NEG_X), wall);
        assert_eq!(facings.facing_of(DVec3::new(0.0, 1.0, 1e-3)), Facing::Up);
        assert_ne!(facings.facing_of(DVec3::NEG_Y), Facing::Up);
    }

    // The compiler's up-facing test is the runtime's: a face the compiler
    // binds to an up-facing probe clamps its lookups to the horizon, and
    // any other reads about its own normal, either side of 1°.
    #[test]
    fn up_facing_cosine_matches_the_runtime_hemisphere_axis() {
        use postretro_render_cpu::fluid_surface::reflection_hemisphere::{
            FLUID_REFLECTION_UP, REFLECTION_UP_FACING_MIN_COS, reflection_hemisphere_axis,
        };
        assert_eq!(UP_FACING_MIN_COS as f32, REFLECTION_UP_FACING_MIN_COS);
        let mut facings = Facings::default();
        for degrees in [0.0, 0.5, 0.99, 1.01, 2.0, 45.0] {
            let (s, c) = f64::to_radians(degrees).sin_cos();
            let normal = DVec3::new(s, c, 0.0);
            let runtime_up = reflection_hemisphere_axis(normal.as_vec3()) == FLUID_REFLECTION_UP;
            assert_eq!(facings.facing_of(normal).is_up(), runtime_up, "{degrees}°");
        }
    }

    #[test]
    fn up_measures_as_water_always_has_and_others_round_trip() {
        let p = DVec3::new(1.5, -2.25, 0.75);
        assert_eq!(Facing::Up.project(p), DVec2::new(1.5, 0.75));
        assert_eq!(Facing::Up.height(p), -2.25);
        assert_eq!(Facing::Up.lift(DVec2::new(1.5, 0.75), -2.25), p);
        let mut facings = Facings::default();
        for normal in [
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::new(0.0, 1.0, 1.0).normalize(),
        ] {
            let facing = facings.facing_of(normal);
            let back = facing.lift(facing.project(p), facing.height(p));
            assert!((back - p).length() < 1e-12, "{facing}");
            let [u, v] = facing.in_plane_axes();
            assert!(u.dot(normal).abs() < 1e-12 && v.dot(normal).abs() < 1e-12);
        }
    }
}
