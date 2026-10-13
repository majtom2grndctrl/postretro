// Bounded load-time broad phase for independent acceleration point queries.
// See: context/lib/entity_model.md §7 (Environment Volumes).

use glam::Vec3;

use crate::bvh::{AabbTree, TreeEntry};

const MAX_AXIS_CELLS: usize = 32;
const MAX_BRUSH_CELLS: usize = 64;
const MAX_REFERENCES: usize = 262_144;

/// Cell ranges contain complete candidate bounds, avoiding an extra indirect
/// bounds load per query. The reference cap bounds this duplication to 8 MiB.
/// Construction counts references before allocating the final array, avoiding
/// a second set of duplicated bounds in temporary cell buckets.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PointGrid {
    min: Vec3,
    max: Vec3,
    scale: Vec3,
    dims: [usize; 3],
    offsets: Vec<usize>,
    entries: Vec<TreeEntry<u32>>,
    fallback: AabbTree<u32>,
}

impl PointGrid {
    pub(crate) const fn empty() -> Self {
        Self {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            scale: Vec3::ZERO,
            dims: [0; 3],
            offsets: Vec::new(),
            entries: Vec::new(),
            fallback: AabbTree::empty(),
        }
    }

    pub(crate) fn build(entries: Vec<TreeEntry<u32>>) -> Self {
        Self::build_with_reference_limit(entries, MAX_REFERENCES)
    }

    fn build_with_reference_limit(entries: Vec<TreeEntry<u32>>, reference_limit: usize) -> Self {
        if entries.is_empty() {
            return Self::empty();
        }
        let (min, max) = entries.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), entry| (min.min(entry.min), max.max(entry.max)),
        );
        let extent = max - min;
        let volume = extent.x * extent.y * extent.z;
        if !extent.is_finite()
            || !extent.cmpgt(Vec3::ZERO).all()
            || !(volume > 0.0 && volume.is_finite())
            || entries
                .iter()
                .any(|entry| !valid_bounds(entry.min, entry.max))
        {
            return Self::tree_only(entries);
        }
        let side = (volume / (entries.len() as f32 * 4.0)).cbrt();
        let dims = [extent.x, extent.y, extent.z]
            .map(|axis| (axis / side).ceil().clamp(1.0, MAX_AXIS_CELLS as f32) as usize);
        let scale = Vec3::new(
            dims[0] as f32 / extent.x,
            dims[1] as f32 / extent.y,
            dims[2] as f32 / extent.z,
        );
        if !scale.is_finite() {
            return Self::tree_only(entries);
        }

        let mut counts = vec![0usize; dims[0] * dims[1] * dims[2]];
        let mut local = Vec::new();
        let mut fallback = Vec::new();
        let mut reference_count = 0;
        for entry in entries {
            let low = cell(entry.min, min, scale, dims);
            let high = cell(entry.max, min, scale, dims);
            let count = (high[0] - low[0] + 1) * (high[1] - low[1] + 1) * (high[2] - low[2] + 1);
            if count > MAX_BRUSH_CELLS || reference_count + count > reference_limit {
                fallback.push(entry);
                continue;
            }
            reference_count += count;
            for z in low[2]..=high[2] {
                for y in low[1]..=high[1] {
                    for x in low[0]..=high[0] {
                        counts[(z * dims[1] + y) * dims[0] + x] += 1;
                    }
                }
            }
            local.push((entry, low, high));
        }
        let mut offsets = Vec::with_capacity(counts.len() + 1);
        offsets.push(0);
        for count in counts {
            offsets.push(offsets.last().copied().expect("initial offset") + count);
        }
        let mut entries = vec![
            TreeEntry {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
                payload: 0
            };
            reference_count
        ];
        let mut cursors = offsets[..offsets.len() - 1].to_vec();
        for (entry, low, high) in local {
            for z in low[2]..=high[2] {
                for y in low[1]..=high[1] {
                    for x in low[0]..=high[0] {
                        let cursor = &mut cursors[(z * dims[1] + y) * dims[0] + x];
                        entries[*cursor] = entry.clone();
                        *cursor += 1;
                    }
                }
            }
        }
        Self {
            min,
            max,
            scale,
            dims,
            offsets,
            entries,
            fallback: AabbTree::build(fallback),
        }
    }

    fn tree_only(entries: Vec<TreeEntry<u32>>) -> Self {
        Self {
            fallback: AabbTree::build(entries),
            ..Self::empty()
        }
    }

    pub(crate) fn visit_point(&self, point: Vec3, mut visit: impl FnMut(&u32)) {
        if !self.offsets.is_empty() && point.cmpge(self.min).all() && point.cmple(self.max).all() {
            let cell = cell(point, self.min, self.scale, self.dims);
            let index = (cell[2] * self.dims[1] + cell[1]) * self.dims[0] + cell[0];
            for entry in &self.entries[self.offsets[index]..self.offsets[index + 1]] {
                if point.cmpge(entry.min).all() && point.cmple(entry.max).all() {
                    visit(&entry.payload);
                }
            }
        }
        self.fallback.visit_point(point, visit);
    }
}

pub(crate) fn valid_bounds(min: Vec3, max: Vec3) -> bool {
    min.is_finite() && max.is_finite() && min.cmple(max).all()
}

fn cell(point: Vec3, min: Vec3, scale: Vec3, dims: [usize; 3]) -> [usize; 3] {
    // Build and lookup use the same monotone map, including its rounding and
    // closed upper face. Cell selection only narrows candidates, never results.
    let point = (point - min) * scale;
    [
        (point.x as usize).min(dims[0] - 1),
        (point.y as usize).min(dims[1] - 1),
        (point.z as usize).min(dims[2] - 1),
    ]
}

#[cfg(test)]
#[path = "point_grid_tests.rs"]
mod tests;
