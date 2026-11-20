// Point-to-region index: world-space buckets over the regions' XZ footprints,
// built once at load, so `region_at` and `resolve_region_at` visit the regions
// near a point instead of every region.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;

use super::NavRegionRecord;

/// Stack room for the snap query's bucket cursors. A bucket at least as wide as
/// the snap tolerance spans at most 6 buckets per axis with the margin below.
const MAX_SNAP_BUCKETS: usize = 64;
/// Upper bound on bucket count; past it buckets widen.
const MAX_BUCKETS: usize = 1 << 22;
/// Past this many buckets' width (`2^22`), f32 spacing at a coordinate can
/// exceed one bucket, so a one-bucket rounding margin no longer covers it.
const EXACT_SPAN_BUCKETS: f32 = 4_194_304.0;

/// World-space XZ buckets, each listing the regions whose footprint reaches
/// it, in ascending region index (compressed rows).
#[derive(Debug, Clone)]
pub(super) struct RegionIndex {
    base: [f32; 2],
    bucket: f32,
    dims: [usize; 2],
    /// `starts[b]..starts[b + 1]` indexes `regions` for bucket `b`
    /// (`b = z * dims[0] + x`).
    starts: Vec<u32>,
    regions: Vec<u32>,
    /// Points with a coordinate this far from the origin take the linear scan,
    /// where one bucket of rounding margin may not cover f32 spacing.
    exact_limit: f32,
}

// Visit counter for the scale proof (P3): regions examined by lookups on this
// thread.
#[cfg(test)]
thread_local! {
    pub(super) static REGIONS_EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
pub(super) fn count_examined() {
    #[cfg(test)]
    REGIONS_EXAMINED.with(|count| count.set(count.get() + 1));
}

impl RegionIndex {
    /// Build over `regions`. `None` when a footprint is non-finite or the
    /// bucket width is unusable: every lookup then keeps the linear scan.
    pub(super) fn build(
        regions: &[NavRegionRecord],
        cell_size: f32,
        snap_tolerance: f32,
    ) -> Option<Self> {
        if regions.is_empty() {
            return None;
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for region in regions {
            for axis in 0..2 {
                let (lo, hi) = ordered(region.world_min_xz[axis], region.world_max_xz[axis]);
                if !lo.is_finite() || !hi.is_finite() {
                    return None;
                }
                min[axis] = min[axis].min(lo);
                max[axis] = max[axis].max(hi);
            }
        }
        let mut bucket = cell_size.max(snap_tolerance);
        if !bucket.is_finite() || bucket <= 0.0 {
            return None;
        }
        let dims = loop {
            let span = |axis: usize| ((max[axis] - min[axis]) / bucket).floor() as usize + 1;
            let dims = [span(0), span(1)];
            if dims[0].saturating_mul(dims[1]) <= MAX_BUCKETS {
                break dims;
            }
            bucket *= 2.0;
        };
        let max_abs = [min[0], max[0], min[1], max[1]]
            .into_iter()
            .fold(0.0f32, |acc, v| acc.max(v.abs()));
        let exact_limit = bucket * EXACT_SPAN_BUCKETS;
        if max_abs + snap_tolerance >= exact_limit {
            return None;
        }

        let mut index = Self {
            base: min,
            bucket,
            dims,
            starts: Vec::new(),
            regions: Vec::new(),
            exact_limit,
        };
        // Two passes over the same footprints: count, then fill, so each
        // bucket's list ascends by region index.
        let bucket_count = dims[0] * dims[1];
        let mut counts = vec![0u32; bucket_count + 1];
        for region in regions {
            index.for_each_bucket(region, |bucket| counts[bucket + 1] += 1);
        }
        for i in 0..bucket_count {
            counts[i + 1] += counts[i];
        }
        let mut cursor = counts.clone();
        let mut entries = vec![0u32; counts[bucket_count] as usize];
        for (region_index, region) in regions.iter().enumerate() {
            index.for_each_bucket(region, |bucket| {
                entries[cursor[bucket] as usize] = region_index as u32;
                cursor[bucket] += 1;
            });
        }
        index.starts = counts;
        index.regions = entries;
        Some(index)
    }

    /// The bucket coordinate of `value` on `axis`, before clamping. Rounding is
    /// monotonic, so `a <= b` gives `bucket_of(a) <= bucket_of(b)`: a footprint
    /// containing a point always shares the point's bucket.
    #[inline]
    fn bucket_of(&self, value: f32, axis: usize) -> i64 {
        ((value - self.base[axis]) / self.bucket).floor() as i64
    }

    fn clamped_range(&self, lo: f32, hi: f32, axis: usize, margin: i64) -> Option<(usize, usize)> {
        let last = self.dims[axis] as i64 - 1;
        let first = (self.bucket_of(lo, axis) - margin).max(0);
        let end = (self.bucket_of(hi, axis) + margin).min(last);
        (first <= end).then_some((first as usize, end as usize))
    }

    fn for_each_bucket(&self, region: &NavRegionRecord, mut visit: impl FnMut(usize)) {
        let (x_lo, x_hi) = ordered(region.world_min_xz[0], region.world_max_xz[0]);
        let (z_lo, z_hi) = ordered(region.world_min_xz[1], region.world_max_xz[1]);
        let (Some((x0, x1)), Some((z0, z1))) = (
            self.clamped_range(x_lo, x_hi, 0, 0),
            self.clamped_range(z_lo, z_hi, 1, 0),
        ) else {
            return;
        };
        for z in z0..=z1 {
            for x in x0..=x1 {
                visit(z * self.dims[0] + x);
            }
        }
    }

    fn bucket_regions(&self, bucket: usize) -> &[u32] {
        &self.regions[self.starts[bucket] as usize..self.starts[bucket + 1] as usize]
    }

    fn covers(&self, position: Vec3) -> bool {
        position.is_finite()
            && position.x.abs() < self.exact_limit
            && position.z.abs() < self.exact_limit
    }

    /// The regions whose footprint may contain `position`, ascending, or
    /// `None` when the point needs the linear scan.
    pub(super) fn containing_candidates(&self, position: Vec3) -> Option<&[u32]> {
        if !self.covers(position) {
            return None;
        }
        let x = self.bucket_of(position.x, 0);
        let z = self.bucket_of(position.z, 1);
        if x < 0 || z < 0 || x >= self.dims[0] as i64 || z >= self.dims[1] as i64 {
            return Some(&[]);
        }
        Some(self.bucket_regions(z as usize * self.dims[0] + x as usize))
    }

    /// Visit, in ascending region index and once each, every region whose
    /// footprint may lie within `tolerance` of `position` on XZ. Returns
    /// `false` without visiting when the point needs the linear scan.
    pub(super) fn for_each_snap_candidate(
        &self,
        position: Vec3,
        tolerance: f32,
        mut visit: impl FnMut(usize),
    ) -> bool {
        if !self.covers(position) || !tolerance.is_finite() {
            return false;
        }
        // One bucket of margin each side absorbs the rounding in `p ± tol`
        // and in the snap test's own `min - p` (within `exact_limit`).
        let ranges = (
            self.clamped_range(position.x - tolerance, position.x + tolerance, 0, 1),
            self.clamped_range(position.z - tolerance, position.z + tolerance, 1, 1),
        );
        let (Some((x0, x1)), Some((z0, z1))) = ranges else {
            return true;
        };
        let mut cursors = [(0usize, 0usize); MAX_SNAP_BUCKETS];
        let mut count = 0;
        for z in z0..=z1 {
            for x in x0..=x1 {
                if count == MAX_SNAP_BUCKETS {
                    debug_assert!(false, "snap span exceeds the bucket cursor room");
                    return false;
                }
                let bucket = z * self.dims[0] + x;
                cursors[count] = (
                    self.starts[bucket] as usize,
                    self.starts[bucket + 1] as usize,
                );
                count += 1;
            }
        }
        // K-way merge of ascending lists, skipping repeats of one region.
        let cursors = &mut cursors[..count];
        let mut last = None;
        loop {
            let mut next: Option<u32> = None;
            for &(start, end) in cursors.iter() {
                if start < end {
                    let region = self.regions[start];
                    if next.is_none_or(|best| region < best) {
                        next = Some(region);
                    }
                }
            }
            let Some(region) = next else {
                return true;
            };
            for (start, end) in cursors.iter_mut() {
                if *start < *end && self.regions[*start] == region {
                    *start += 1;
                }
            }
            if last != Some(region) {
                visit(region as usize);
                last = Some(region);
            }
        }
    }
}

fn ordered(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}
