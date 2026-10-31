// Which patches lie in each static light's reach, without tracing a ray.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;
use rayon::prelude::*;

use super::patches::Patch;
use crate::map_data::{LightType, MapLight};
use crate::sh_bake::light_reaches_point;

/// Edge of the bucket grid patch centroids are sorted into, in metres. A
/// light visits only the buckets its range's bounding box overlaps.
const BUCKET_SIZE_M: f64 = 4.0;

/// Patch centroids bucketed on a uniform grid, buckets sorted by cell, so a
/// light's reach query visits the buckets near it rather than every patch.
pub(crate) struct PatchReachIndex<'a> {
    patches: &'a [Patch],
    /// `(cell, first, end)` into `order`, ascending by cell (x, then y, z).
    buckets: Vec<([i32; 3], u32, u32)>,
    /// Patch indices grouped by bucket.
    order: Vec<u32>,
}

impl<'a> PatchReachIndex<'a> {
    pub(crate) fn new(patches: &'a [Patch]) -> Self {
        let mut keyed: Vec<([i32; 3], u32)> = patches
            .iter()
            .enumerate()
            .map(|(index, patch)| (bucket_of(patch.centroid.as_dvec3()), index as u32))
            .collect();
        keyed.sort_unstable();
        let mut buckets: Vec<([i32; 3], u32, u32)> = Vec::new();
        for (at, &(cell, _)) in keyed.iter().enumerate() {
            match buckets.last_mut() {
                Some((last, _, end)) if *last == cell => *end = at as u32 + 1,
                _ => buckets.push((cell, at as u32, at as u32 + 1)),
            }
        }
        Self {
            patches,
            buckets,
            order: keyed.into_iter().map(|(_, index)| index).collect(),
        }
    }

    /// How many patches `light` reaches: centroids that pass the bounce's
    /// own range test ([`light_reaches_point`]), so a layer holds at most
    /// two entries (one per side) per counted patch.
    pub(crate) fn count_in_reach(&self, light: &MapLight) -> u64 {
        if light.light_type == LightType::Directional {
            return self.patches.len() as u64;
        }
        let range = f64::from(light.falloff_range.max(1.0e-4));
        let origin = light.origin;
        if !origin.is_finite() || !range.is_finite() {
            // No box bounds the reach: test every patch.
            return self
                .patches
                .iter()
                .filter(|patch| light_reaches_point(light, patch.centroid))
                .count() as u64;
        }
        // One bucket of margin each way: the test runs in f32, the box in f64.
        let lo = bucket_of(origin - DVec3::splat(range)).map(|c| c.saturating_sub(1));
        let hi = bucket_of(origin + DVec3::splat(range)).map(|c| c.saturating_add(1));
        let first = self
            .buckets
            .partition_point(|&(cell, _, _)| cell[0] < lo[0]);
        self.buckets[first..]
            .iter()
            .take_while(|&&(cell, _, _)| cell[0] <= hi[0])
            .filter(|&&(cell, _, _)| (lo[1]..=hi[1]).contains(&cell[1]))
            .filter(|&&(cell, _, _)| (lo[2]..=hi[2]).contains(&cell[2]))
            .map(|&(_, start, end)| {
                self.order[start as usize..end as usize]
                    .iter()
                    .filter(|&&index| {
                        light_reaches_point(light, self.patches[index as usize].centroid)
                    })
                    .count() as u64
            })
            .sum()
    }

    /// [`Self::count_in_reach`] for every light, in slice order.
    pub(crate) fn counts_in_reach(&self, lights: &[&MapLight]) -> Vec<u64> {
        lights
            .par_iter()
            .map(|light| self.count_in_reach(light))
            .collect()
    }
}

fn bucket_of(point: DVec3) -> [i32; 3] {
    let cell = (point / BUCKET_SIZE_M).floor();
    [cell.x, cell.y, cell.z].map(|c| c.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
}
