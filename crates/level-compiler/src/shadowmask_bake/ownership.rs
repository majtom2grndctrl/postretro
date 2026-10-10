// Per-face mask ownership: candidate ranking, the four channels of one face,
// and the drop report.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use std::cmp::Ordering;

use postretro_level_format::shadowmask_atlas::{
    FaceOwners, NO_FACE_OWNERS, SHADOWMASK_OWNER_CHANNELS,
};

/// Lit chart-interior texels a light needs on a face to rank in the upper
/// tier. The stress-map census cleared every overflow at this value.
pub(crate) const LIT_TEXEL_FLOOR: u32 = 16;

/// One light's shadowed presence on one face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) spec_index: u16,
    /// Chart-interior texels whose quantized baked visibility is non-zero.
    pub(crate) lit_texels: u32,
    /// Summed `contribution × quantized visibility` luminance over those texels.
    pub(crate) contribution: f64,
    pub(crate) promoted: bool,
}

impl Candidate {
    pub(crate) fn above_floor(&self) -> bool {
        self.lit_texels >= LIT_TEXEL_FLOOR
    }

    /// The ranking's total order: above-floor first; above the floor, id-40
    /// lights first; then larger contribution; then the lower spec index. It
    /// never depends on arrival order.
    pub(crate) fn rank_cmp(&self, other: &Self) -> Ordering {
        let tier = |c: &Self| (c.above_floor(), c.above_floor() && c.promoted);
        tier(self)
            .cmp(&tier(other))
            .then(self.contribution.total_cmp(&other.contribution))
            .then(other.spec_index.cmp(&self.spec_index))
    }

    pub(crate) fn outranks(&self, other: &Self) -> bool {
        self.rank_cmp(other) == Ordering::Greater
    }
}

/// What offering a candidate to a full or partly full face did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Admission {
    /// The candidate holds `channel`; `evicted` lost it.
    Admitted {
        channel: usize,
        evicted: Option<Candidate>,
    },
    Rejected,
}

/// One face's four channels, in the order owners took them. Channel order is
/// put right by [`FaceChannels::ascending_order`] before encode.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FaceChannels {
    channels: [Option<Candidate>; SHADOWMASK_OWNER_CHANNELS],
    /// Every candidate offered, kept or not: the face's demand.
    candidates: u32,
}

impl FaceChannels {
    /// Admit `candidate` to a free channel, or in place of the lowest-ranked
    /// owner it outranks. The kept set is the top four by rank whatever the
    /// order candidates arrive in.
    pub(crate) fn offer(&mut self, candidate: Candidate) -> Admission {
        self.candidates += 1;
        if let Some(channel) = self.channels.iter().position(Option::is_none) {
            self.channels[channel] = Some(candidate);
            return Admission::Admitted {
                channel,
                evicted: None,
            };
        }
        let (weakest, owner) = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(channel, owner)| owner.map(|owner| (channel, owner)))
            .min_by(|(_, a), (_, b)| a.rank_cmp(b))
            .expect("a full face has four owners");
        if candidate.outranks(&owner) {
            self.channels[weakest] = Some(candidate);
            Admission::Admitted {
                channel: weakest,
                evicted: Some(owner),
            }
        } else {
            Admission::Rejected
        }
    }

    /// Fix an owner into `channel` without ranking: a cut face's owners come
    /// from its estimate before the walk.
    pub(crate) fn fix(&mut self, channel: usize, owner: Candidate) {
        self.channels[channel] = Some(owner);
    }

    /// Release a channel whose owner baked nothing.
    pub(crate) fn clear(&mut self, channel: usize) {
        self.channels[channel] = None;
    }

    pub(crate) fn channel_of(&self, spec_index: u16) -> Option<usize> {
        self.channels
            .iter()
            .position(|owner| owner.is_some_and(|owner| owner.spec_index == spec_index))
    }

    pub(crate) fn owner(&self, channel: usize) -> Option<Candidate> {
        self.channels[channel]
    }

    pub(crate) fn note_candidates(&mut self, count: u32) {
        self.candidates += count;
    }

    pub(crate) fn candidates(&self) -> u32 {
        self.candidates
    }

    pub(crate) fn has_owner(&self) -> bool {
        self.channels.iter().any(Option::is_some)
    }

    /// The owner entry in ascending spec index, empties trailing, and for each
    /// entry position the channel its mask values sit in now.
    pub(crate) fn ascending_order(
        &self,
    ) -> (FaceOwners, [Option<usize>; SHADOWMASK_OWNER_CHANNELS]) {
        let mut held: Vec<(u16, usize)> = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(channel, owner)| owner.map(|owner| (owner.spec_index, channel)))
            .collect();
        held.sort_unstable();
        let mut owners = NO_FACE_OWNERS;
        let mut source = [None; SHADOWMASK_OWNER_CHANNELS];
        for (position, (spec_index, channel)) in held.into_iter().enumerate() {
            owners[position] = spec_index;
            source[position] = Some(channel);
        }
        (owners, source)
    }
}

/// Where a dropped light lost its channel. A cut face is named once, by its
/// pre-cut face and the id-17 faces it became.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DropSite {
    Face(u32),
    CutFace {
        parent: u32,
        first_face: u32,
        last_face: u32,
    },
}

/// An above-floor light that holds no channel on a face it lights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OwnerDrop {
    pub(crate) site: DropSite,
    pub(crate) spec_index: u16,
    pub(crate) lit_texels: u32,
}

/// The drop report: complete on every build, and identical cold and warm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OwnerReport {
    /// Most candidate lights seen on one face (one cut face counts once).
    pub(crate) peak_face_demand: u32,
    /// Sorted by site, then spec index.
    pub(crate) drops: Vec<OwnerDrop>,
}

/// Warn once per dropped light, naming the light by its source label and the
/// face it lost. `--verbose` adds the peak per-face demand.
pub(crate) fn log_owner_report(
    report: &OwnerReport,
    label_of: impl Fn(u16) -> String,
    verbose: bool,
) {
    for drop in &report.drops {
        let site = match drop.site {
            DropSite::Face(face) => format!("face {face}"),
            DropSite::CutFace {
                parent,
                first_face,
                last_face,
            } => format!("cut face {parent} (faces {first_face}..={last_face})"),
        };
        log::warn!(
            "[ShadowmaskAtlas] {} lost its specular mask channel on {site} to four \
             higher-ranked lights ({} lit texels): no world highlight there",
            label_of(drop.spec_index),
            drop.lit_texels,
        );
    }
    if verbose {
        log::info!(
            "[ShadowmaskAtlas] peak per-face owner demand: {} candidate light(s) on one face \
             (capacity {SHADOWMASK_OWNER_CHANNELS}); {} light(s) dropped above the floor",
            report.peak_face_demand,
            report.drops.len(),
        );
    }
}
