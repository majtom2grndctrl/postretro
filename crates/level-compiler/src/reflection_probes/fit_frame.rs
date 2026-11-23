// Reflection-probe box-fit frame: the world axis a facing fits along, as a signed axis permutation.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::DVec3;

use super::box_fit::WorldBounds;

/// The frame a box fit runs in: local +Y is the world axis nearest a probe's
/// facing, signed toward it, and local X and Z are the other two world axes.
/// A signed permutation, so the fit's axis-aligned box rules hold in it and
/// map back to a world box exactly. Water's frame is the identity: every
/// conversion returns its input untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FitFrame {
    /// The world axis local +Y lies on.
    up: usize,
    /// Whether local +Y points down that axis.
    negative: bool,
}

impl FitFrame {
    /// The world frame: an up-facing probe's.
    pub(crate) const UP: Self = Self {
        up: 1,
        negative: false,
    };

    /// The frame whose local +Y is the world axis nearest `normal`, ties
    /// going to Y, then X: a sloped face fits the room as the floor, ceiling
    /// or wall it is nearest.
    pub(crate) fn nearest(normal: DVec3) -> Self {
        let a = normal.abs();
        let up = if a.y >= a.x && a.y >= a.z {
            1
        } else if a.x >= a.z {
            0
        } else {
            2
        };
        Self {
            up,
            negative: normal[up] < 0.0,
        }
    }

    /// The world axes of local X, Y and Z.
    fn axes(self) -> [usize; 3] {
        match self.up {
            0 => [1, 0, 2],
            1 => [0, 1, 2],
            _ => [0, 2, 1],
        }
    }

    fn signed(self, value: f64) -> f64 {
        if self.negative { -value } else { value }
    }

    pub(crate) fn to_local(self, world: DVec3) -> DVec3 {
        if self == Self::UP {
            return world;
        }
        let [x, y, z] = self.axes();
        DVec3::new(world[x], self.signed(world[y]), world[z])
    }

    pub(crate) fn to_world(self, local: DVec3) -> DVec3 {
        if self == Self::UP {
            return local;
        }
        let [x, y, z] = self.axes();
        let mut world = DVec3::ZERO;
        world[x] = local.x;
        world[y] = self.signed(local.y);
        world[z] = local.z;
        world
    }

    /// The world box of the local box `lo..hi`.
    pub(crate) fn box_to_world(self, lo: DVec3, hi: DVec3) -> (DVec3, DVec3) {
        if self == Self::UP {
            return (lo, hi);
        }
        let (a, b) = (self.to_world(lo), self.to_world(hi));
        (a.min(b), a.max(b))
    }

    /// `bounds` in this frame.
    pub(crate) fn bounds_to_local(self, bounds: &WorldBounds) -> WorldBounds {
        if self == Self::UP {
            return *bounds;
        }
        let (a, b) = (self.to_local(bounds.min), self.to_local(bounds.max));
        WorldBounds {
            min: a.min(b),
            max: a.max(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_axis_frame_round_trips_and_points_local_up_along_its_facing() {
        for normal in [
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Z,
            DVec3::NEG_Z,
            DVec3::new(0.0, 0.7, 0.71),
            DVec3::new(0.0, 0.71, 0.7),
        ] {
            let frame = FitFrame::nearest(normal);
            let p = DVec3::new(1.5, -2.25, 0.75);
            assert_eq!(frame.to_world(frame.to_local(p)), p, "{normal}");
            assert!(frame.to_world(DVec3::Y).dot(normal) > 0.7, "{normal}");
            let (lo, hi) = frame.box_to_world(DVec3::new(-1.0, 0.0, -2.0), DVec3::ONE);
            assert!(lo.cmple(hi).all(), "{normal}: {lo} {hi}");
        }
        assert_eq!(FitFrame::nearest(DVec3::Y), FitFrame::UP);
        // A 45° slope fits as the floor it is nearest.
        assert_eq!(
            FitFrame::nearest(DVec3::new(0.0, 1.0, 1.0).normalize()),
            FitFrame::UP
        );
    }
}
