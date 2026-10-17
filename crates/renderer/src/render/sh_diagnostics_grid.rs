// Conservative local grid bounds for probe markers, cell wires, and readback.
// See: context/lib/rendering_pipeline.md §12 Debug-Line Renderer

use glam::Vec3;
use std::ops::Range;

pub(super) struct GridBounds {
    x: Range<u32>,
    y: Range<u32>,
    z: Range<u32>,
}

impl GridBounds {
    pub(super) fn around_sphere(
        dims: [u32; 3],
        origin: Vec3,
        cell: Vec3,
        camera: Vec3,
        radius: f32,
    ) -> Self {
        let full = dims.map(|dimension| 0..dimension);
        let mut ranges = full.clone();
        for axis in 0..3 {
            if !origin[axis].is_finite()
                || !camera[axis].is_finite()
                || !cell[axis].is_finite()
                || cell[axis] <= 0.0
                || !radius.is_finite()
                || !(radius * radius).is_finite()
                || radius < 0.0
            {
                return Self {
                    x: full[0].clone(),
                    y: full[1].clone(),
                    z: full[2].clone(),
                };
            }
            let spacing = f64::from(cell[axis]);
            let camera = f64::from(camera[axis]);
            let origin = f64::from(origin[axis]);
            let radius = f64::from(radius);
            // Keep the callers' original f32 sphere predicate authoritative.
            // Expand for its rounding, even at large origins/tiny spacings.
            let magnitude = origin.abs() + camera.abs() + spacing * f64::from(dims[axis]) + radius;
            let padding = magnitude * f64::from(f32::EPSILON) * 8.0 / spacing + 1.0;
            let low = ((camera - radius - origin) / spacing - padding).floor();
            let high = ((camera + radius - origin) / spacing + padding).ceil() + 1.0;
            let limit = f64::from(dims[axis]);
            ranges[axis] = low.clamp(0.0, limit) as u32..high.clamp(0.0, limit) as u32;
        }
        Self {
            x: ranges[0].clone(),
            y: ranges[1].clone(),
            z: ranges[2].clone(),
        }
    }

    /// Inclusive affinity bricks covering every candidate probe. Keep these
    /// in grid coordinates: a world-space round trip can lose whole bricks
    /// when the origin is large relative to the probe spacing.
    pub(super) fn brick_bounds(&self) -> Option<([u32; 3], [u32; 3])> {
        if self.x.is_empty() || self.y.is_empty() || self.z.is_empty() {
            return None;
        }
        Some((
            [self.x.start / 4, self.y.start / 4, self.z.start / 4],
            [
                (self.x.end - 1) / 4,
                (self.y.end - 1) / 4,
                (self.z.end - 1) / 4,
            ],
        ))
    }

    pub(super) fn coordinates(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        self.z.clone().flat_map(move |z| {
            self.y
                .clone()
                .flat_map(move |y| self.x.clone().map(move |x| [x, y, z]))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn full(dims: [u32; 3]) -> impl Iterator<Item = [u32; 3]> {
        (0..dims[2])
            .flat_map(move |z| (0..dims[1]).flat_map(move |y| (0..dims[0]).map(move |x| [x, y, z])))
    }

    fn accepted(coord: [u32; 3], origin: Vec3, cell: Vec3, camera: Vec3, radius: f32) -> bool {
        let pos = origin
            + Vec3::new(
                coord[0] as f32 * cell.x,
                coord[1] as f32 * cell.y,
                coord[2] as f32 * cell.z,
            );
        pos.distance_squared(camera) <= radius * radius
    }

    proptest! {
        #[test]
        fn bounded_grid_preserves_all_sphere_members_in_original_order(
            dims in [1u32..25, 1u32..25, 1u32..25],
            origin in [-1000.0f32..1000.0, -1000.0f32..1000.0, -1000.0f32..1000.0],
            cell in [0.01f32..100.0, 0.01f32..100.0, 0.01f32..100.0],
            camera in [-1500.0f32..1500.0, -1500.0f32..1500.0, -1500.0f32..1500.0],
            radius in 0.0f32..200.0,
        ) {
            let (origin, cell, camera) = (Vec3::from(origin), Vec3::from(cell), Vec3::from(camera));
            let expected: Vec<_> = full(dims).filter(|&coord| accepted(coord, origin, cell, camera, radius)).collect();
            let actual: Vec<_> = GridBounds::around_sphere(dims, origin, cell, camera, radius).coordinates().filter(|&coord| accepted(coord, origin, cell, camera, radius)).collect();
            prop_assert_eq!(actual, expected);
            // Cell wires use the midpoint of two separately rounded positions.
            // Preserve that original predicate, too, rather than assuming it
            // rounds identically to a half-cell-shifted probe lattice.
            let cell_accepted = |coord: [u32; 3]| {
                let min = origin + Vec3::from(coord.map(|n| n as f32)) * cell;
                let center = (min + (min + cell)) * 0.5;
                center.distance_squared(camera) <= radius * radius
            };
            let expected: Vec<_> = full(dims).filter(|&coord| cell_accepted(coord)).collect();
            let actual: Vec<_> = GridBounds::around_sphere(dims, origin + cell * 0.5, cell, camera, radius).coordinates().filter(|&coord| cell_accepted(coord)).collect();
            prop_assert_eq!(actual, expected);
        }
    }

    #[test]
    fn bounded_grid_handles_empty_partial_and_extreme_precision_grids() {
        for (dims, origin, cell, camera, radius) in [
            ([0, 4, 4], Vec3::ZERO, Vec3::ONE, Vec3::ZERO, 4.0),
            (
                [17, 3, 9],
                Vec3::new(-8.0, 2.0, -4.0),
                Vec3::new(0.5, 2.0, 0.25),
                Vec3::new(-8.0, 2.0, -4.0),
                2.0,
            ),
            (
                [12, 4, 8],
                Vec3::splat(1.0e12),
                Vec3::splat(0.001),
                Vec3::splat(1.0e12),
                1.0,
            ),
            ([12, 4, 8], Vec3::ZERO, Vec3::ONE, Vec3::splat(-1000.0), 2.0),
            (
                [12, 4, 8],
                Vec3::ZERO,
                Vec3::ONE,
                Vec3::splat(1.0e30),
                1.0e20,
            ),
        ] {
            let expected: Vec<_> = full(dims)
                .filter(|&coord| accepted(coord, origin, cell, camera, radius))
                .collect();
            let actual: Vec<_> = GridBounds::around_sphere(dims, origin, cell, camera, radius)
                .coordinates()
                .filter(|&coord| accepted(coord, origin, cell, camera, radius))
                .collect();
            assert_eq!(actual, expected);
        }
        assert!(
            GridBounds::around_sphere(
                [197, 47, 239],
                Vec3::ZERO,
                Vec3::ONE,
                Vec3::new(100.0, 20.0, 100.0),
                8.0
            )
            .coordinates()
            .count()
                < 10000
        );
    }
}
