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

/// Host bytes a [`PatchReachIndex`] holds per patch at its peak, while it
/// builds: the keyed scratch beside the buckets (at most one per patch) and
/// the patch order.
pub(crate) const REACH_INDEX_BYTES_PER_PATCH: u64 =
    (size_of::<([i32; 3], u32)>() + size_of::<([i32; 3], u32, u32)>() + size_of::<u32>()) as u64;

/// Patch centroids bucketed on a uniform grid, buckets sorted by cell, so a
/// light's reach query visits the buckets near it rather than every patch.
pub(crate) struct PatchReachIndex<'a> {
    patches: &'a [Patch],
    /// `(cell, first, end)` into `order`, ascending by cell (x, then y, z).
    buckets: Vec<([i32; 3], u32, u32)>,
    /// Patch indices grouped by bucket.
    order: Vec<u32>,
    #[cfg(test)]
    _held: crate::live_bytes::Held,
}

impl<'a> PatchReachIndex<'a> {
    /// Every allocation is exact, so the index and its build scratch hold
    /// at most [`REACH_INDEX_BYTES_PER_PATCH`] per patch.
    pub(crate) fn new(patches: &'a [Patch]) -> Self {
        let mut keyed: Vec<([i32; 3], u32)> = patches
            .iter()
            .enumerate()
            .map(|(index, patch)| (bucket_of(patch.centroid.as_dvec3()), index as u32))
            .collect();
        #[cfg(test)]
        let keyed_held = crate::live_bytes::hold_vec("reach index build", &keyed);
        keyed.sort_unstable();
        let cells = keyed.chunk_by(|a, b| a.0 == b.0).count();
        let mut buckets: Vec<([i32; 3], u32, u32)> = Vec::with_capacity(cells);
        for (at, &(cell, _)) in keyed.iter().enumerate() {
            match buckets.last_mut() {
                Some((last, _, end)) if *last == cell => *end = at as u32 + 1,
                _ => buckets.push((cell, at as u32, at as u32 + 1)),
            }
        }
        let mut order = Vec::with_capacity(keyed.len());
        order.extend(keyed.iter().map(|&(_, index)| index));
        #[cfg(test)]
        let _held = {
            let held = crate::live_bytes::hold(
                "reach index",
                (buckets.capacity() * size_of::<([i32; 3], u32, u32)>()
                    + order.capacity() * size_of::<u32>()) as u64,
            );
            drop(keyed_held);
            held
        };
        Self {
            patches,
            buckets,
            order,
            #[cfg(test)]
            _held,
        }
    }

    /// How many patches `light` reaches: centroids that pass the bounce's
    /// own range test ([`light_reaches_point`]), so a layer holds at most
    /// two entries (one per side) per counted patch.
    pub(crate) fn count_in_reach(&self, light: &MapLight) -> u64 {
        let mut count = 0u64;
        self.for_each_in_reach(light, |_| count += 1);
        count
    }

    /// The patches [`Self::count_in_reach`] counts, ascending by index: the
    /// only patches a layer shades.
    /// Exact: counted first, so the list holds no growth slack.
    pub(crate) fn in_reach(&self, light: &MapLight) -> Vec<u32> {
        let mut patches = Vec::with_capacity(self.count_in_reach(light) as usize);
        self.for_each_in_reach(light, |index| patches.push(index));
        patches.sort_unstable();
        patches
    }

    /// Visit each patch whose centroid `light` reaches, bucket by bucket.
    fn for_each_in_reach(&self, light: &MapLight, mut visit: impl FnMut(u32)) {
        let reaches =
            |index: u32| light_reaches_point(light, self.patches[index as usize].centroid);
        if light.light_type == LightType::Directional {
            (0..self.patches.len() as u32).for_each(visit);
            return;
        }
        let range = f64::from(light.falloff_range.max(1.0e-4));
        let origin = light.origin;
        if !origin.is_finite() || !range.is_finite() {
            // No box bounds the reach: test every patch.
            (0..self.patches.len() as u32)
                .filter(|&index| reaches(index))
                .for_each(visit);
            return;
        }
        // One bucket of margin each way: the test runs in f32, the box in f64.
        let lo = bucket_of(origin - DVec3::splat(range)).map(|c| c.saturating_sub(1));
        let hi = bucket_of(origin + DVec3::splat(range)).map(|c| c.saturating_add(1));
        let first = self
            .buckets
            .partition_point(|&(cell, _, _)| cell[0] < lo[0]);
        for &(cell, start, end) in self.buckets[first..]
            .iter()
            .take_while(|&&(cell, _, _)| cell[0] <= hi[0])
        {
            if !(lo[1]..=hi[1]).contains(&cell[1]) || !(lo[2]..=hi[2]).contains(&cell[2]) {
                continue;
            }
            for &index in &self.order[start as usize..end as usize] {
                if reaches(index) {
                    visit(index);
                }
            }
        }
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
