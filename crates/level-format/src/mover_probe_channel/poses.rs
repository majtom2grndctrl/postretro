// MoverProbeChannel (id 57) per-pose records: each mover's ramp and pose
// knots, and the bounce entries and depth records that mark, pose by pose,
// which probes store a state, read their base, or blend across.
// See: context/lib/build_pipeline.md §PRL section IDs

/// What one pose holds for one probe in a bounce entry or depth record.
///
/// Storage sparsity only: no mark flags a probe occluded, zeroes a weight or
/// excludes a probe. Each says which state compose blends from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoseMark {
    /// The pose stores no state. At a terminus (the first or last pose) the
    /// probe reads its base there: id 34 for bounce, its id-59 tile or
    /// averaged moments for depth. At a mid pose it blends across, between
    /// the nearest poses that store a state or read the base.
    Unmarked,
    /// The pose stores a state: one tile in its block's payload.
    Stored,
    /// A mid pose reads the probe's base: the pose leaves the probe within
    /// its bars. An enclosing mid pose is never `Base`: it is `Unmarked` in
    /// bounce and marked across in direct, and stores its dark depth tile.
    /// Never at a terminus, where an unmarked probe already reads the base.
    Base,
}

/// Whether pose `pose` of `pose_count` is a terminus: the first or the last.
pub fn is_terminus(pose: usize, pose_count: usize) -> bool {
    pose == 0 || pose + 1 == pose_count
}

/// One mover's pose table: its author ramp and where along its path each
/// pose sits. Every block stores a mover's states in this pose order.
#[derive(Debug, Clone, PartialEq)]
pub struct MoverRecord {
    pub mover_id: u32,
    /// The author ramp: the mover's light state reads
    /// `clamp((t - ramp_start) / (ramp_end - ramp_start), 0, 1)`, with
    /// `0 <= ramp_start < ramp_end <= 1`. Game-side; knots map through it.
    pub ramp_start: f32,
    pub ramp_end: f32,
    /// Each pose's arc-length fraction t along the path: at least two,
    /// finite, strictly ascending, the first exactly 0 and the last exactly
    /// 1, so the termini are the first and last poses.
    pub knots: Vec<f32>,
}

impl MoverRecord {
    /// A mover that stores its two termini alone.
    pub fn termini(mover_id: u32, ramp_start: f32, ramp_end: f32) -> Self {
        Self {
            mover_id,
            ramp_start,
            ramp_end,
            knots: vec![0.0, 1.0],
        }
    }

    pub fn pose_count(&self) -> usize {
        self.knots.len()
    }
}

/// One pose's masks in a bounce entry: local x-fastest probe bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BouncePoseMasks {
    /// Probes storing `S^b_k` at this pose: their state leaves id 34's
    /// round-trip bar.
    pub stored: u64,
    /// Probes reading id 34 at this mid pose. Always 0 at a terminus.
    pub base: u64,
}

impl BouncePoseMasks {
    pub fn mark(&self, local: u32) -> PoseMark {
        let bit = 1u64.checked_shl(local).unwrap_or(0);
        if self.stored & bit != 0 {
            PoseMark::Stored
        } else if self.base & bit != 0 {
            PoseMark::Base
        } else {
            PoseMark::Unmarked
        }
    }
}

/// One mover at one listed cell: the probes whose static bounce it changes,
/// and what each pose holds for each.
///
/// The union of the poses' `stored` masks is the affected-probe mask, a
/// non-empty subset of the cell's valid probes; any one pose's may be empty.
/// The masks are storage sparsity; none ever flags, zeroes or excludes a
/// probe. Tiles run pose by pose, then probe by `stored` rank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoverBounceEntry {
    pub mover_id: u32,
    /// One per pose of the mover, in pose order.
    pub poses: Vec<BouncePoseMasks>,
}

impl MoverBounceEntry {
    /// An entry of a two-pose mover storing `stored[k]` at terminus k.
    pub fn termini(mover_id: u32, stored: [u64; 2]) -> Self {
        Self {
            mover_id,
            poses: stored
                .into_iter()
                .map(|stored| BouncePoseMasks { stored, base: 0 })
                .collect(),
        }
    }

    pub fn pose_count(&self) -> usize {
        self.poses.len()
    }

    /// Probes the mover affects: those storing a state at some pose.
    pub fn affected_probe_mask(&self) -> u64 {
        self.poses.iter().fold(0, |mask, pose| mask | pose.stored)
    }

    /// Tiles the entry's payload holds: one per stored (pose, probe).
    pub fn stored_tile_count(&self) -> usize {
        self.poses
            .iter()
            .map(|pose| pose.stored.count_ones() as usize)
            .sum()
    }

    /// What pose `pose` holds for local probe `local`; [`PoseMark::Unmarked`]
    /// past the entry's poses.
    pub fn mark(&self, pose: usize, local: u32) -> PoseMark {
        self.poses
            .get(pose)
            .map_or(PoseMark::Unmarked, |masks| masks.mark(local))
    }

    /// Local probe `local`'s tile at pose `pose`, as a tile index into the
    /// entry's payload (pose after pose, probe by `stored` rank): `None`
    /// where that pose stores no state for it.
    pub fn tile_index(&self, pose: usize, local: u32) -> Option<usize> {
        let bit = 1u64.checked_shl(local)?;
        let masks = self.poses.get(pose)?;
        if masks.stored & bit == 0 {
            return None;
        }
        let before: usize = self.poses[..pose]
            .iter()
            .map(|earlier| earlier.stored.count_ones() as usize)
            .sum();
        Some(before + (masks.stored & (bit - 1)).count_ones() as usize)
    }
}

/// One mover's closed-pose depth at one probe: what each pose holds. At
/// least one pose stores a tile, and the tiles follow in pose order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoverDepthRecord {
    pub mover_id: u32,
    /// Dense id-34 lattice index of a valid probe.
    pub probe_index: u32,
    /// One per pose of the mover, in pose order. A pose stores `D_k` where
    /// the mover posed there meets some gather ray of the probe.
    pub marks: Vec<PoseMark>,
}

impl MoverDepthRecord {
    /// A record of a two-pose mover storing `D_k` where `stored[k]`.
    pub fn termini(mover_id: u32, probe_index: u32, stored: [bool; 2]) -> Self {
        Self {
            mover_id,
            probe_index,
            marks: stored
                .into_iter()
                .map(|stored| {
                    if stored {
                        PoseMark::Stored
                    } else {
                        PoseMark::Unmarked
                    }
                })
                .collect(),
        }
    }

    /// Records ascend by this key, with no repeats.
    pub fn order_key(&self) -> (u32, u32) {
        (self.mover_id, self.probe_index)
    }

    pub fn pose_count(&self) -> usize {
        self.marks.len()
    }

    /// Tiles the record stores: one per stored pose.
    pub fn stored_tile_count(&self) -> usize {
        self.marks
            .iter()
            .filter(|&&mark| mark == PoseMark::Stored)
            .count()
    }

    /// Pose `pose`'s tile, as a tile index into the record's tiles: `None`
    /// where the pose stores none.
    pub fn tile_index(&self, pose: usize) -> Option<usize> {
        (self.marks.get(pose) == Some(&PoseMark::Stored)).then(|| {
            self.marks[..pose]
                .iter()
                .filter(|&&mark| mark == PoseMark::Stored)
                .count()
        })
    }
}

/// One mover's across marks at one listed cell: per mid pose, the local
/// probes the pose's brushes enclose. Enclosure is geometric, a function of
/// (mover, probe, pose) alone, so it is recorded once per (cell, mover)
/// rather than per direct record. Direct compose blends across a marked
/// pose, between the nearest poses not marked across around t′, exactly as
/// bounce blends across an unmarked mid pose; the `q` stored there is 255 and
/// never read. Termini are never marked: an enclosing terminus reads the base.
///
/// Storage sparsity only: a mark never flags a probe occluded, zeroes a
/// weight or excludes a probe. Compose still blends between stored states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoverAcrossEntry {
    /// Position of the entry's cell in the section's listed cells.
    pub listed: u32,
    pub mover_id: u32,
    /// One mask per mid pose, poses 1 to N − 2 in order: local x-fastest
    /// bits of the probes that pose encloses.
    pub mid_poses: Vec<u64>,
}

impl MoverAcrossEntry {
    /// Entries ascend by this key, with no repeats.
    pub fn order_key(&self) -> (u32, u32) {
        (self.listed, self.mover_id)
    }

    /// Probes marked at pose `pose`; 0 at a terminus or past the poses.
    pub fn marks(&self, pose: usize) -> u64 {
        pose.checked_sub(1)
            .and_then(|mid| self.mid_poses.get(mid))
            .copied()
            .unwrap_or(0)
    }

    /// Whether pose `pose` is marked across for local probe `local`.
    pub fn is_across(&self, pose: usize, local: u32) -> bool {
        self.marks(pose) & 1u64.checked_shl(local).unwrap_or(0) != 0
    }

    /// Probes marked at some pose.
    pub fn marked_probes(&self) -> u64 {
        self.mid_poses.iter().fold(0, |mask, &pose| mask | pose)
    }
}
