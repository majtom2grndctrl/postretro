// One mover's bounce and closed-pose depth states: its poses' gathers
// reduced, pose by pose, to the probes id 57 stores.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::mover_probe_channel::{MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::depth::{DEPTH_SIGNIFICANCE_FLOOR, MoverAbsentDepth};

/// One probe whose posed state leaves its id-34 tile's round-trip bar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChangedProbe {
    pub(crate) probe: u32,
    /// The posed state as id 27's compact RGB tile ([`compact_rgb`]).
    pub(crate) tile: Vec<u16>,
    /// Past the perceptual floor too: the probe joins the bounce domain.
    pub(crate) affected: bool,
}

/// What one terminus's gather over every valid probe leaves: the probes
/// whose state leaves the round-trip bar, with their tiles, and the depth
/// tile of every probe a gather ray of which met the pose.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PoseStates {
    /// Ascending by probe.
    pub(crate) changed: Vec<ChangedProbe>,
    /// `(dense probe, closed-pose tile)`, ascending.
    pub(crate) depth: Vec<(u32, Vec<[u16; 2]>)>,
}

impl PoseStates {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        let changed: usize = self
            .changed
            .iter()
            .map(|probe| probe.tile.capacity() * size_of::<u16>())
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|(_, tile)| tile.capacity() * size_of::<[u16; 2]>())
            .sum();
        (changed
            + depth
            + self.changed.capacity() * size_of::<ChangedProbe>()
            + self.depth.capacity() * size_of::<(u32, Vec<[u16; 2]>)>()) as u64
    }

    /// The changed state of `probe`, when this terminus's gather left the
    /// round-trip bar there.
    pub(crate) fn changed(&self, probe: u32) -> Option<&ChangedProbe> {
        self.changed
            .binary_search_by_key(&probe, |changed| changed.probe)
            .ok()
            .map(|at| &self.changed[at])
    }
}

/// What a mid pose holds for one probe of the mover's near-field domain:
/// a state of its own, or the probe's base. A probe a mid pose lists in
/// neither way blends across it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MidState<T> {
    Stored(T),
    Base,
}

/// What one mid pose's near-field gather leaves, each list ascending by
/// probe. Storage sparsity only: a listed base or an unlisted probe says
/// which state compose blends from, never a flag or a weight.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MidPoseStates {
    /// The domain probes whose bounce the pose stores, as id 27's compact
    /// RGB tile ([`compact_rgb`]), or reads from the base; any other blends
    /// across.
    pub(crate) bounce: Vec<(u32, MidState<Vec<u16>>)>,
    /// Every domain probe: its closed-pose tile where some gather ray met
    /// the pose, else its base.
    pub(crate) depth: Vec<(u32, MidState<Vec<[u16; 2]>>)>,
}

impl MidPoseStates {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        let bounce: usize = self
            .bounce
            .iter()
            .map(|(_, state)| match state {
                MidState::Stored(tile) => tile.capacity() * size_of::<u16>(),
                MidState::Base => 0,
            })
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|(_, state)| match state {
                MidState::Stored(tile) => tile.capacity() * size_of::<[u16; 2]>(),
                MidState::Base => 0,
            })
            .sum();
        (bounce
            + depth
            + self.bounce.capacity() * size_of::<(u32, MidState<Vec<u16>>)>()
            + self.depth.capacity() * size_of::<(u32, MidState<Vec<[u16; 2]>>)>()) as u64
    }
}

/// One pose's raw states, as its own bake leaves them and its memo keeps
/// them: a terminus's full gather or a mid pose's near-field one. Nothing
/// in it depends on another pose, so every cross-pose rule (which probes a
/// mover stores, skipping, the depth floor and range) runs after each pose
/// resolves.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PoseState {
    Terminus(PoseStates),
    Mid(MidPoseStates),
}

impl PoseState {
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> u64 {
        match self {
            Self::Terminus(states) => states.heap_bytes(),
            Self::Mid(states) => states.heap_bytes(),
        }
    }
}

/// One mover's poses, ready to reduce: its termini's full gathers and its
/// mid poses' near-field ones, in pose order.
#[derive(Default)]
pub(crate) struct MoverPoseStates {
    /// First, then last.
    pub(crate) termini: [PoseStates; 2],
    pub(crate) mids: Vec<MidPoseStates>,
}

impl MoverPoseStates {
    /// Every pose in path order, at least two: a terminus first and last
    /// and a mid pose between.
    pub(crate) fn from_poses(poses: Vec<PoseState>) -> Self {
        let last = poses.len() - 1;
        let mut states = Self::default();
        for (pose, state) in poses.into_iter().enumerate() {
            match state {
                PoseState::Terminus(terminus) if pose == 0 || pose == last => {
                    states.termini[usize::from(pose == last)] = terminus;
                }
                PoseState::Mid(mid) if pose != 0 && pose != last => states.mids.push(mid),
                _ => unreachable!("pose {pose} of {} has the wrong kind", last + 1),
            }
        }
        states
    }

    /// Every pose in path order, as [`Self::from_poses`] took them.
    pub(crate) fn into_poses(self) -> Vec<PoseState> {
        let [first, last] = self.termini;
        std::iter::once(PoseState::Terminus(first))
            .chain(self.mids.into_iter().map(PoseState::Mid))
            .chain(std::iter::once(PoseState::Terminus(last)))
            .collect()
    }

    fn pose_count(&self) -> usize {
        self.mids.len() + 2
    }

    /// What pose `pose` lists for `probe` in one block: a terminus through
    /// `terminus`, a mid pose from its list.
    fn lookup<'s, T>(
        &'s self,
        pose: usize,
        probe: u32,
        terminus: impl Fn(usize) -> Option<&'s [T]>,
        mid: impl Fn(&'s MidPoseStates) -> &'s [(u32, MidState<Vec<T>>)],
    ) -> Option<MidState<&'s [T]>> {
        let last = self.pose_count() - 1;
        if pose == 0 || pose == last {
            return terminus(usize::from(pose == last)).map(MidState::Stored);
        }
        let list = mid(&self.mids[pose - 1]);
        let at = list
            .binary_search_by_key(&probe, |(probe, _)| *probe)
            .ok()?;
        Some(match &list[at].1 {
            MidState::Stored(tile) => MidState::Stored(tile.as_slice()),
            MidState::Base => MidState::Base,
        })
    }
}

/// One affected probe: what each pose holds for it, and its stored states
/// in pose order, each id 27's compact RGB tile. A terminus whose state is
/// within the round-trip bar of id 34 stores none and composes id 34 (owner,
/// 2026-11-09); a mid pose reads id 34 where it encloses the probe or
/// leaves it within its bars, and blends across where it lists nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BounceProbe {
    pub(crate) probe: u32,
    /// One per pose; at least one stores, and no terminus reads the base.
    pub(crate) marks: Vec<PoseMark>,
    pub(crate) tiles: Vec<u16>,
}

/// One probe carrying the mover's depth: what each pose holds for it, and
/// its stored tiles in pose order, each id 59's bordered tile. A pose whose
/// mover meets none of the probe's gather rays stores none: a terminus
/// reads the probe's base there, as does a mid pose inside the near-field
/// domain; a mid pose outside it blends across.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DepthProbe {
    pub(crate) probe: u32,
    /// One per pose; at least one stores, and no terminus reads the base.
    pub(crate) marks: Vec<PoseMark>,
    pub(crate) tiles: Vec<[u16; 2]>,
}

/// One mover's states, as id 57 stores them: its poses reduced.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoverBounceStates {
    pub(crate) mover_id: u32,
    /// Poses along the mover's path, termini included: every probe's marks
    /// run this long.
    pub(crate) pose_count: usize,
    /// Affected probes, ascending.
    pub(crate) bounce: Vec<BounceProbe>,
    /// Probes carrying closed-pose depth, ascending.
    pub(crate) depth: Vec<DepthProbe>,
    /// Probes whose closed-pose depth passes the significance floor, before
    /// the tile range cuts `depth`: its length unless the mover alone passes
    /// the range.
    pub(crate) depth_carried: usize,
    #[cfg(test)]
    held: crate::live_bytes::Held,
}

impl MoverBounceStates {
    pub(crate) fn new(
        mover_id: u32,
        pose_count: usize,
        bounce: Vec<BounceProbe>,
        depth: Vec<DepthProbe>,
    ) -> Self {
        #[cfg_attr(
            not(test),
            expect(unused_mut, reason = "only the test ledger records it")
        )]
        let mut states = Self {
            mover_id,
            pose_count,
            depth_carried: depth.len(),
            bounce,
            depth,
            #[cfg(test)]
            held: crate::live_bytes::Held::default(),
        };
        #[cfg(test)]
        {
            states.held = crate::live_bytes::hold("mover bounce states", states.heap_bytes());
        }
        states
    }

    /// Whether the mover alone carries depth at more probes than the tile
    /// range `depth_cap`: the build then fails by name, so its poses are
    /// never memoized.
    pub(crate) fn depth_past_range(&self, depth_cap: u32) -> bool {
        self.depth.len() > depth_cap as usize
    }

    /// Replace the stored states with `bounce` and `depth`, the same probes
    /// less those left with no stored state; `dropped_depth` depth probes
    /// left that way no longer carry depth.
    pub(crate) fn replace(
        &mut self,
        bounce: Vec<BounceProbe>,
        depth: Vec<DepthProbe>,
        dropped_depth: usize,
    ) {
        self.bounce = bounce;
        self.depth = depth;
        self.depth_carried -= dropped_depth;
        #[cfg(test)]
        {
            self.held = crate::live_bytes::hold("mover bounce states", self.heap_bytes());
        }
    }

    #[cfg(test)]
    fn heap_bytes(&self) -> u64 {
        let bounce: usize = self
            .bounce
            .iter()
            .map(|probe| {
                probe.tiles.capacity() * size_of::<u16>()
                    + probe.marks.capacity() * size_of::<PoseMark>()
            })
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|probe| {
                probe.tiles.capacity() * size_of::<[u16; 2]>()
                    + probe.marks.capacity() * size_of::<PoseMark>()
            })
            .sum();
        (bounce
            + depth
            + self.bounce.capacity() * size_of::<BounceProbe>()
            + self.depth.capacity() * size_of::<DepthProbe>()) as u64
    }
}

/// A tile's RGB, texel by texel: id 27's compact tile, the form every
/// stored bounce state takes from its pose's bake on.
pub(crate) fn compact_rgb(tile: &[OctahedralAtlasTexel]) -> Vec<u16> {
    tile.iter()
        .flat_map(|texel| [texel.rgba[0], texel.rgba[1], texel.rgba[2]])
        .collect()
}

/// Which termini store a bounce state, per probe the termini affect: a
/// probe affected at either terminus joins the domain, and each terminus
/// stores its state where it leaves the round-trip bar of id 34. Decided
/// from the termini alone, so a mid pose never moves what a terminus
/// stores.
fn terminus_stores(termini: &[PoseStates; 2]) -> BTreeMap<u32, [bool; 2]> {
    let mut changed: BTreeMap<u32, [Option<&ChangedProbe>; 2]> = BTreeMap::new();
    for (terminus, pose) in termini.iter().enumerate() {
        for probe in &pose.changed {
            changed.entry(probe.probe).or_default()[terminus] = Some(probe);
        }
    }
    changed
        .into_iter()
        .filter(|(_, states)| states.iter().flatten().any(|state| state.affected))
        .map(|(probe, states)| (probe, states.map(|state| state.is_some())))
        .collect()
}

/// Push pose state `state` onto a probe's marks and its tile onto its tiles.
fn mark<T: Copy>(state: Option<MidState<&[T]>>, marks: &mut Vec<PoseMark>, tiles: &mut Vec<T>) {
    match state {
        None => marks.push(PoseMark::Unmarked),
        Some(MidState::Base) => marks.push(PoseMark::Base),
        Some(MidState::Stored(tile)) => {
            marks.push(PoseMark::Stored);
            tiles.extend_from_slice(tile);
        }
    }
}

/// Reduce one mover's poses to its states.
///
/// Bounce: the termini decide alone ([`terminus_stores`]), as with two
/// poses: a probe affected at either terminus joins the domain, and each
/// stores its state where it leaves the round-trip bar; one that encloses
/// the probe is never changed, so it stores none, and a probe enclosed at
/// both termini joins through a mid pose or not at all
/// ([`super::gather::PosedProbe::changed`]). A probe some mid pose stores
/// a state for joins too; its termini store nothing for it, so at rest it
/// reads id 34 as before. Each mid pose marks what
/// [`super::mid_pose::classify_mid_pose`] decided.
///
/// Depth: a probe carries the mover's depth when one of its closed-pose
/// tiles scores at least [`DEPTH_SIGNIFICANCE_FLOOR`] against its
/// mover-absent depth. Where a terminus tile scores so, the termini keep
/// their two-pose rule: each stores a tile where some gather ray of the
/// probe met it, and at a terminus no ray meets, the posed gather bins the
/// mover-absent tile exactly, so it stores none and the probe reads its base
/// there. An enclosing terminus meets every ray at distance 0, so it stores
/// that dark tile and Chebyshev still rejects the probe at rest. Where only
/// a mid pose's tile scores so, the termini store nothing: the probe's live
/// tile reads its base at rest. Each mid pose stores its tile where a ray
/// met it and reads the base elsewhere in the domain; outside the domain it
/// blends across.
///
/// The floor is per mover: it compares the mover's own closed-pose tile with
/// the mover-absent depth, so a probe below it never joins the states. Depth
/// keeps at most `depth_cap + 1` records, the cap being the tile range: one
/// mover carrying depth at more probes than the range already fails the
/// build by name ([`super::depth::MoverTileRangeExceeded`]), and the one
/// record past it keeps that failure. Every probe past the floor still
/// counts toward `depth_carried`, so the failure names the mover's reach.
pub(crate) fn reduce_poses(
    mover_id: u32,
    poses: &MoverPoseStates,
    absent: &MoverAbsentDepth<'_>,
    depth_cap: u32,
) -> MoverBounceStates {
    let pose_count = poses.pose_count();
    let stores = terminus_stores(&poses.termini);
    let mut bounce_probes: BTreeSet<u32> = stores.keys().copied().collect();
    for mid in &poses.mids {
        bounce_probes.extend(
            mid.bounce
                .iter()
                .filter(|(_, state)| matches!(state, MidState::Stored(_)))
                .map(|(probe, _)| *probe),
        );
    }
    let bounce = bounce_probes
        .into_iter()
        .map(|probe| {
            let stored = stores.get(&probe).copied().unwrap_or([false; 2]);
            let mut marks = Vec::with_capacity(pose_count);
            let mut tiles: Vec<u16> = Vec::new();
            for pose in 0..pose_count {
                let state = poses.lookup(
                    pose,
                    probe,
                    |terminus| {
                        stored[terminus].then(|| {
                            poses.termini[terminus]
                                .changed(probe)
                                .expect("a stored terminus changed the probe")
                                .tile
                                .as_slice()
                        })
                    },
                    |mid| &mid.bounce,
                );
                mark(state, &mut marks, &mut tiles);
            }
            debug_assert_eq!(
                tiles.len(),
                marks.iter().filter(|&&m| m == PoseMark::Stored).count()
                    * MOVER_IRRADIANCE_TILE_F16_COUNT
            );
            BounceProbe {
                probe,
                marks,
                tiles,
            }
        })
        .collect();

    let mut met: BTreeMap<u32, [Option<&[[u16; 2]]>; 2]> = BTreeMap::new();
    for (terminus, pose) in poses.termini.iter().enumerate() {
        for (probe, tile) in &pose.depth {
            met.entry(*probe).or_default()[terminus] = Some(tile);
        }
    }
    let mut depth_probes: BTreeSet<u32> = met.keys().copied().collect();
    for mid in &poses.mids {
        depth_probes.extend(
            mid.depth
                .iter()
                .filter(|(_, state)| matches!(state, MidState::Stored(_)))
                .map(|(probe, _)| *probe),
        );
    }
    let significant =
        |probe: u32, tile: &[[u16; 2]]| absent.score(probe, tile) >= DEPTH_SIGNIFICANCE_FLOOR;
    let mut depth = Vec::new();
    let mut carried = 0;
    for probe in depth_probes {
        let termini = met.get(&probe).copied().unwrap_or([None; 2]);
        let termini_carry = termini
            .iter()
            .flatten()
            .any(|tile| significant(probe, tile));
        let carries = termini_carry
            || (1..pose_count - 1).any(|pose| {
                matches!(
                    poses.lookup(pose, probe, |_| None, |mid| &mid.depth),
                    Some(MidState::Stored(tile)) if significant(probe, tile)
                )
            });
        if !carries {
            continue;
        }
        carried += 1;
        if depth.len() > depth_cap as usize {
            continue;
        }
        let mut marks = Vec::with_capacity(pose_count);
        let mut tiles = Vec::new();
        for pose in 0..pose_count {
            let state = poses.lookup(
                pose,
                probe,
                |terminus| termini[terminus].filter(|_| termini_carry),
                |mid| &mid.depth,
            );
            mark(state, &mut marks, &mut tiles);
        }
        depth.push(DepthProbe {
            probe,
            marks,
            tiles,
        });
    }
    let mut states = MoverBounceStates::new(mover_id, pose_count, bounce, depth);
    states.depth_carried = carried;
    states
}
