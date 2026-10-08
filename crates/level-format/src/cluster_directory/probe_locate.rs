//! Exact batched cell-locator descent over the whole probe grid.
//! See: context/lib/build_pipeline.md §PRL section IDs

use crate::{
    cell_locator::{CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection},
    sh_volume::OctahedralShVolumeSection,
};

/// A valid probe reaches a locator fault: a non-terminating walk, a missing
/// node, or a terminal cell outside the runtime cells. The caller falls back
/// to the per-probe walk, which reports the first fault in probe order.
pub(super) struct LocateFault;

/// Inclusive probe-coordinate box.
#[derive(Clone, Copy)]
struct ProbeBox {
    min: [u32; 3],
    max: [u32; 3],
}

/// Locates boxes of probes at once, with the same answer per probe as the
/// per-point walk in `locate_cell`.
///
/// Per point, the walk evaluates `((n0·x + n1·y) + n2·z) − d` in `f32` and
/// goes front when it is `>= 0`. With the three products and `d` finite, that
/// expression is non-decreasing in each product: every step is one rounded
/// addition of a finite operand, and rounding is monotone. Each product is a
/// function of one axis coordinate only. So over a box, the expression at the
/// per-axis minimum products bounds every point's value from below, and at
/// the maxima from above, exactly in `f32`, with no error analysis. A box
/// whose lower bound is `>= 0` goes front whole; one whose upper bound is
/// `< 0` goes back whole; any other box, or one with a non-finite product,
/// splits, down to single points that use the per-point expression verbatim.
///
/// A straddled axis-aligned plane splits a box exactly where it crosses: the
/// other two products are zero, so a point's side depends only on, and is
/// monotone in, its coordinate on that axis. Other planes halve the box.
pub(super) struct ProbeLocator<'a> {
    locator: &'a CellLocatorSection,
    runtime_cell_count: u32,
    /// Probe position per axis coordinate: `probe_position`'s expression.
    positions: [Vec<f32>; 3],
    /// Per axis: positions are finite and non-decreasing, so a product's
    /// extremes over a coordinate range sit at the range's ends.
    monotone: [bool; 3],
    grid: [u32; 3],
    /// Summed-volume table of valid probes, padded by one on each axis.
    valid_probes: Vec<u32>,
}

impl<'a> ProbeLocator<'a> {
    pub(super) fn new(
        locator: &'a CellLocatorSection,
        runtime_cell_count: u32,
        base: &OctahedralShVolumeSection,
    ) -> Self {
        let grid = base.grid_dimensions;
        let positions: [Vec<f32>; 3] = std::array::from_fn(|axis| {
            (0..grid[axis])
                .map(|coord| base.grid_origin[axis] + coord as f32 * base.cell_size[axis])
                .collect()
        });
        let monotone = std::array::from_fn(|axis| {
            positions[axis].iter().all(|position| position.is_finite())
                && positions[axis].windows(2).all(|pair| pair[0] <= pair[1])
        });
        // Prefix sums along x, then y, then z. Counts fit `u32` because the
        // probe count does (checked by the caller).
        let [gx, gy, gz] = grid.map(|dim| dim as usize);
        let (row, plane) = (gx + 1, (gx + 1) * (gy + 1));
        let mut valid_probes = vec![0u32; plane * (gz + 1)];
        let mut probe = 0usize;
        for z in 0..gz {
            for y in 0..gy {
                let start = (z + 1) * plane + (y + 1) * row;
                for x in 0..gx {
                    valid_probes[start + x + 1] =
                        valid_probes[start + x] + u32::from(base.probes[probe].validity != 0);
                    probe += 1;
                }
            }
        }
        for z in 1..=gz {
            for y in 2..=gy {
                for x in 1..=gx {
                    let index = z * plane + y * row + x;
                    valid_probes[index] += valid_probes[index - row];
                }
            }
        }
        for z in 2..=gz {
            for index in z * plane..(z + 1) * plane {
                valid_probes[index] += valid_probes[index - plane];
            }
        }
        Self {
            locator,
            runtime_cell_count,
            positions,
            monotone,
            grid,
            valid_probes,
        }
    }

    /// Push `(brick, cell)` for every brick holding a valid probe that
    /// locates to `cell`; at least one pair per such brick. Unordered, may
    /// repeat. Err when any valid probe's walk faults.
    pub(super) fn locate_grid(
        &self,
        affinity_dims: [u32; 3],
        located: &mut Vec<(u32, u32)>,
    ) -> Result<(), LocateFault> {
        let whole = ProbeBox {
            min: [0; 3],
            max: self.grid.map(|dim| dim - 1),
        };
        if !self.any_valid(whole) {
            return Ok(());
        }
        // Pending boxes live on the heap, not the call stack: a valid but
        // near-linear locator chain straddles one node per level, as deep as
        // the grid is wide. Box order cannot change the result.
        let mut pending = vec![(self.locator.root, self.locator.nodes.len() + 1, whole)];
        while let Some((child, remaining, probes)) = pending.pop() {
            self.descend(
                child,
                remaining,
                probes,
                affinity_dims,
                located,
                &mut pending,
            )?;
        }
        Ok(())
    }

    /// Walk from `child` with `remaining` steps, as `locate_cell` does, for a
    /// box holding at least one valid probe. A box that straddles a node
    /// queues its parts in `pending`.
    fn descend(
        &self,
        mut child: CellLocatorChild,
        mut remaining: usize,
        probes: ProbeBox,
        affinity_dims: [u32; 3],
        located: &mut Vec<(u32, u32)>,
        pending: &mut Vec<(CellLocatorChild, usize, ProbeBox)>,
    ) -> Result<(), LocateFault> {
        loop {
            if remaining == 0 {
                return Err(LocateFault);
            }
            remaining -= 1;
            let index = match child {
                CellLocatorChild::Cell(cell) => {
                    if cell >= self.runtime_cell_count {
                        return Err(LocateFault);
                    }
                    self.record(probes, cell, affinity_dims, located);
                    return Ok(());
                }
                CellLocatorChild::Node(index) => index,
            };
            let node = self.locator.nodes.get(index as usize).ok_or(LocateFault)?;
            match self.side(node, probes) {
                Some(true) => child = node.front,
                Some(false) => child = node.back,
                None => {
                    // Both parts re-enter this node with the step count it
                    // was reached with.
                    for part in self.split(node, probes) {
                        if self.any_valid(part) {
                            pending.push((child, remaining + 1, part));
                        }
                    }
                    return Ok(());
                }
            }
        }
    }

    /// Two boxes covering a straddling box: cut where an axis-aligned plane
    /// changes side, otherwise halved along the widest axis.
    fn split(&self, node: &CellLocatorNodeRecord, probes: ProbeBox) -> [ProbeBox; 2] {
        if let Some(cut) = self.axis_plane_cut(node, probes) {
            return cut;
        }
        let extent = |axis: usize| probes.max[axis] - probes.min[axis];
        let axis = (0..3).max_by_key(|&axis| extent(axis)).expect("three axes");
        halves(probes, axis, probes.min[axis] + extent(axis) / 2)
    }

    /// For a plane with one nonzero normal component, the box cut after the
    /// last coordinate on the first side. `None` when the plane is not
    /// axis-aligned, or an end slab does not resolve to one side.
    fn axis_plane_cut(
        &self,
        node: &CellLocatorNodeRecord,
        probes: ProbeBox,
    ) -> Option<[ProbeBox; 2]> {
        let mut nonzero = (0..3).filter(|&axis| node.plane_normal[axis] != 0.0);
        let axis = nonzero.next()?;
        if nonzero.next().is_some() {
            return None;
        }
        let slab_side = |coord: u32| {
            let mut slab = probes;
            slab.min[axis] = coord;
            slab.max[axis] = coord;
            self.side(node, slab)
        };
        let first = slab_side(probes.min[axis])?;
        if slab_side(probes.max[axis])? == first {
            return None;
        }
        // Side is monotone along the axis, so the last coordinate on the
        // first side lies in [min, max).
        let (mut low, mut high) = (probes.min[axis], probes.max[axis]);
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if slab_side(middle)? == first {
                low = middle;
            } else {
                high = middle;
            }
        }
        Some(halves(probes, axis, low))
    }

    /// Record `cell` for each brick of the box that holds a valid probe.
    fn record(
        &self,
        probes: ProbeBox,
        cell: u32,
        affinity_dims: [u32; 3],
        located: &mut Vec<(u32, u32)>,
    ) {
        let [ax, ay, _] = affinity_dims;
        let low = probes.min.map(|coord| coord / 4);
        let high = probes.max.map(|coord| coord / 4);
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let brick = [x, y, z];
                    let part = ProbeBox {
                        min: std::array::from_fn(|axis| probes.min[axis].max(brick[axis] * 4)),
                        max: std::array::from_fn(|axis| probes.max[axis].min(brick[axis] * 4 + 3)),
                    };
                    // A box inside one brick holds a valid probe already.
                    if low == high || self.any_valid(part) {
                        located.push((x + ax * (y + ay * z), cell));
                    }
                }
            }
        }
    }

    /// `Some(true)` when every probe in the box goes front, `Some(false)`
    /// when every one goes back, `None` when the box must split.
    fn side(&self, node: &CellLocatorNodeRecord, probes: ProbeBox) -> Option<bool> {
        if probes.min == probes.max {
            let point: [f32; 3] =
                std::array::from_fn(|axis| self.positions[axis][probes.min[axis] as usize]);
            let signed = node.plane_normal[0] * point[0]
                + node.plane_normal[1] * point[1]
                + node.plane_normal[2] * point[2]
                - node.plane_distance;
            return Some(signed >= 0.0);
        }
        if !node.plane_distance.is_finite() {
            return None;
        }
        let mut lowest = [0.0f32; 3];
        let mut highest = [0.0f32; 3];
        for axis in 0..3 {
            let (low, high) = self.product_extent(node.plane_normal[axis], axis, probes)?;
            lowest[axis] = low;
            highest[axis] = high;
        }
        if lowest[0] + lowest[1] + lowest[2] - node.plane_distance >= 0.0 {
            return Some(true);
        }
        if highest[0] + highest[1] + highest[2] - node.plane_distance < 0.0 {
            return Some(false);
        }
        None
    }

    /// Least and greatest `normal × position` over the box's coordinates on
    /// `axis`, or `None` when any product is non-finite.
    fn product_extent(&self, normal: f32, axis: usize, probes: ProbeBox) -> Option<(f32, f32)> {
        let range = &self.positions[axis][probes.min[axis] as usize..=probes.max[axis] as usize];
        let ends;
        // Rounded multiplication by one factor is monotone, so on monotone
        // positions the ends hold the extremes.
        let values = if self.monotone[axis] {
            ends = [range[0], range[range.len() - 1]];
            &ends[..]
        } else {
            range
        };
        let mut low = f32::INFINITY;
        let mut high = f32::NEG_INFINITY;
        for &position in values {
            let product = normal * position;
            if !product.is_finite() {
                return None;
            }
            low = low.min(product);
            high = high.max(product);
        }
        Some((low, high))
    }

    /// Whether the box holds a valid probe, by inclusion-exclusion over the
    /// summed-volume table. Wrapping keeps the differences exact.
    fn any_valid(&self, probes: ProbeBox) -> bool {
        let [gx, gy, _] = self.grid.map(|dim| dim as usize);
        let at = |x: u32, y: u32, z: u32| {
            self.valid_probes[x as usize + (gx + 1) * (y as usize + (gy + 1) * z as usize)]
        };
        let [x0, y0, z0] = probes.min;
        let [x1, y1, z1] = probes.max.map(|coord| coord + 1);
        let total = at(x1, y1, z1)
            .wrapping_sub(at(x0, y1, z1))
            .wrapping_sub(at(x1, y0, z1))
            .wrapping_sub(at(x1, y1, z0))
            .wrapping_add(at(x0, y0, z1))
            .wrapping_add(at(x0, y1, z0))
            .wrapping_add(at(x1, y0, z0))
            .wrapping_sub(at(x0, y0, z0));
        total != 0
    }
}

/// The box split after coordinate `last_low` on `axis`.
fn halves(probes: ProbeBox, axis: usize, last_low: u32) -> [ProbeBox; 2] {
    let mut low = probes;
    low.max[axis] = last_low;
    let mut high = probes;
    high.min[axis] = last_low + 1;
    [low, high]
}
