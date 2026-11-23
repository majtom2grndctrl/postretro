// An orthonormal frame of a reflector plane, shared by exclusion and the world reflector faces.
// See: context/lib/resource_management.md §5.2, §5.4

use glam::{DVec2, DVec3};

/// An orthonormal frame of the reflector: heights along its normal, 2D
/// coordinates in its plane. A pure function of the normal, so two faces with
/// one normal share one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlaneBasis {
    pub(super) normal: DVec3,
    u: DVec3,
    v: DVec3,
}

impl PlaneBasis {
    pub(super) fn new(normal: DVec3) -> Self {
        let normal = normal.normalize();
        // The world axis least aligned with the normal, made perpendicular.
        let a = normal.abs();
        let helper = if a.x <= a.y && a.x <= a.z {
            DVec3::X
        } else if a.y <= a.z {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let u = (helper - normal * helper.dot(normal)).normalize();
        Self {
            normal,
            u,
            v: normal.cross(u),
        }
    }

    pub(super) fn height(&self, p: DVec3) -> f64 {
        p.dot(self.normal)
    }

    pub(super) fn project(&self, p: DVec3) -> DVec2 {
        DVec2::new(p.dot(self.u), p.dot(self.v))
    }

    /// The point at plane coordinates `p` and `height` along the normal: the
    /// inverse of [`Self::project`] and [`Self::height`].
    pub(super) fn lift(&self, p: DVec2, height: f64) -> DVec3 {
        self.u * p.x + self.v * p.y + self.normal * height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_basis_lift_inverts_project_and_height_for_any_normal() {
        for normal in [
            DVec3::Y,
            -DVec3::Y,
            DVec3::X,
            DVec3::new(0.0, 1.0, 1.0),
            DVec3::new(0.3, -0.2, 0.9),
        ] {
            let basis = PlaneBasis::new(normal);
            let p = DVec3::new(1.5, -2.25, 0.75);
            let back = basis.lift(basis.project(p), basis.height(p));
            assert!((back - p).length() < 1e-12, "{normal}: {back} vs {p}");
        }
    }
}
