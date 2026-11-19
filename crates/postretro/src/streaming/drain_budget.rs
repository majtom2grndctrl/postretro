//! The one per-drain install budget and admission order for every resource.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::cmp::Reverse;

use thiserror::Error;

use super::request::{ReadTier, StreamResource};

/// Bytes handed to the renderer per drain, across every resource: SH
/// chunks' and lightmap pairs' decoded bytes, never their stored ones. The
/// first item is always admitted, so one oversized item still installs.
pub(crate) const MAX_INSTALL_DECODED_BYTES_PER_DRAIN: u64 = 8 * 1024 * 1024;

/// The per-drain budget while a level entry is Settling. No world frame is
/// drawn, so a drain costs only loading-tree cadence; a larger budget shortens
/// the hold.
pub(crate) const SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN: u64 = 32 * 1024 * 1024;

const MIB: u64 = 1024 * 1024;

/// One resource's estimated frame-thread install CPU: a fixed cost per item
/// plus a cost per MiB of decoded bytes, in whole microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallCost {
    per_item_us: u64,
    per_mib_us: u64,
}

// Coefficients: the warm CPU fit (release, Radeon Pro 5300M Mac) in
// `context/plans/done/streaming-drain-budget-spike/findings.md` §Install cost,
// at the midpoint of each fitted range. A lightmap pair is 15 µs per pair and
// 0.19–0.20 ms/MiB, so 15 µs + 195 µs/MiB. An SH item is a whole cluster
// (`DrainRank::sh` keys on cluster id), so the fit's per-chunk term is per
// item; the "drain + compose" row is used, since compose is work an SH
// install triggers: 64–72 µs per chunk and 1.00–1.16 ms/MiB, so
// 68 µs + 1,080 µs/MiB. Forced drain work (SH eviction, repack, pool growth)
// is not admitted and is not charged.
const LIGHTMAP_PAIR_INSTALL_COST: InstallCost = InstallCost {
    per_item_us: 15,
    per_mib_us: 195,
};
const SH_CLUSTER_INSTALL_COST: InstallCost = InstallCost {
    per_item_us: 68,
    per_mib_us: 1_080,
};

/// Estimated frame-thread install CPU of one ready item of `resource`
/// holding `decoded_bytes`, in whole microseconds: the resource's fixed cost
/// plus its per-MiB cost, the byte term rounded up. Integer math only, so
/// admission against it is deterministic; saturates rather than overflows.
pub(crate) fn estimated_install_us(resource: StreamResource, decoded_bytes: u64) -> u64 {
    let cost = match resource {
        StreamResource::Sh => SH_CLUSTER_INSTALL_COST,
        StreamResource::LightmapBlock => LIGHTMAP_PAIR_INSTALL_COST,
    };
    let byte_term =
        (u128::from(decoded_bytes) * u128::from(cost.per_mib_us)).div_ceil(u128::from(MIB));
    cost.per_item_us
        .saturating_add(u64::try_from(byte_term).unwrap_or(u64::MAX))
}

/// What one drain admits against. Production charges decoded bytes; the
/// estimated-time unit is the walk measurement's sweep lever
/// (`plans/drafts/streaming-drain-time-budget`). Both run the same admission:
/// see [`admit_drain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainBudget {
    /// Each item is charged its decoded bytes.
    DecodedBytes(u64),
    /// Each item is charged [`estimated_install_us`]; the limit is in µs.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the time budget is a test-only walk lever until its play value is chosen"
        )
    )]
    EstimatedMicros(u64),
}

impl DrainBudget {
    /// The in-play budget.
    pub(crate) const PLAY: Self = Self::DecodedBytes(MAX_INSTALL_DECODED_BYTES_PER_DRAIN);
    /// The Settling budget.
    pub(crate) const SETTLING: Self = Self::DecodedBytes(SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN);

    const fn limit(self) -> u64 {
        match self {
            Self::DecodedBytes(limit) | Self::EstimatedMicros(limit) => limit,
        }
    }

    fn charge(self, item: &DrainItem) -> u64 {
        match self {
            Self::DecodedBytes(_) => item.bytes,
            Self::EstimatedMicros(_) => item.estimated_install_us(),
        }
    }
}

/// A ready item's class on the one scale every resource shares. Declaration
/// order is admission order; `Visible` through `Lead` form the mandatory
/// tier, the rest the optional tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DrainClass {
    Visible,
    Pinned,
    /// A lightmap block or SH cluster mandatory only through the camera
    /// cell's id-51 set within lead L.
    Lead,
    SeamWarm,
    /// The id-51 band past L: SH `Band` clusters and lightmap band blocks.
    /// Kept as `Prefetch` on this shared scale; each resource names its own.
    Prefetch,
    Hysteresis,
}

impl DrainClass {
    /// Every class, in declaration order: `ALL[class.index()] == class`.
    pub(crate) const ALL: [Self; 6] = [
        Self::Visible,
        Self::Pinned,
        Self::Lead,
        Self::SeamWarm,
        Self::Prefetch,
        Self::Hysteresis,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn tier(self) -> ReadTier {
        match self {
            Self::Visible | Self::Pinned | Self::Lead => ReadTier::Mandatory,
            Self::SeamWarm | Self::Prefetch | Self::Hysteresis => ReadTier::Optional,
        }
    }
}

/// Position of one ready item in the merged admission order, compared field
/// by field: class; at equal class, SH before lightmap blocks; then higher
/// priority, then lower lead, then lower key. SH items carry lead 0, so SH
/// keeps its class, priority, cluster-id order. Blocks order by priority
/// (nonzero only in the band), then lead, then block id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DrainRank {
    class: DrainClass,
    resource: StreamResource,
    priority: Reverse<u32>,
    lead: u32,
    key: u32,
}

impl DrainRank {
    pub(crate) const fn sh(class: DrainClass, effective_priority: u32, cluster_id: u32) -> Self {
        Self {
            class,
            resource: StreamResource::Sh,
            priority: Reverse(effective_priority),
            lead: 0,
            key: cluster_id,
        }
    }

    /// `lead` is the block's baked lead in id-46 fixed-point units; zero for
    /// the camera's own dilated visible set.
    pub(crate) const fn lightmap_block(
        class: DrainClass,
        priority: u32,
        lead: u32,
        block_id: u32,
    ) -> Self {
        Self {
            class,
            resource: StreamResource::LightmapBlock,
            priority: Reverse(priority),
            lead,
            key: block_id,
        }
    }

    pub(crate) const fn resource(&self) -> StreamResource {
        self.resource
    }

    /// The walk measurement's same-class cut tally reads the class at a cut.
    #[cfg(test)]
    pub(crate) const fn class(&self) -> DrainClass {
        self.class
    }

    pub(crate) const fn key(&self) -> u32 {
        self.key
    }
}

/// One ready unit the renderer installs whole. A lightmap block and its
/// shadowmask block are one item, so the pair is admitted or deferred whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrainItem {
    pub(crate) rank: DrainRank,
    pub(crate) bytes: u64,
}

impl DrainItem {
    /// A lightmap block pair: both halves' upload bytes, charged together.
    pub(crate) fn pair(
        rank: DrainRank,
        lightmap_bytes: u64,
        shadowmask_bytes: u64,
    ) -> Result<Self, DrainBytesOverflow> {
        Ok(Self {
            rank,
            bytes: lightmap_bytes
                .checked_add(shadowmask_bytes)
                .ok_or(DrainBytesOverflow)?,
        })
    }

    /// This item's [`estimated_install_us`].
    pub(crate) fn estimated_install_us(&self) -> u64 {
        estimated_install_us(self.rank.resource(), self.bytes)
    }
}

/// The leading `admitted` items of the ordered list install this drain, and
/// their decoded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrainAdmission {
    pub(crate) admitted: usize,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("drain install bytes overflow u64")]
pub(crate) struct DrainBytesOverflow;

/// Orders a merged ready list for admission, then admits the leading items
/// whose summed charge fits `budget` (normally [`DrainBudget::PLAY`]), each
/// charged in the budget's unit. The first always fits whatever its charge,
/// so an oversized item cannot stall residency. Admission stops at the first
/// item over budget rather than skipping ahead to smaller, lower-ranked work,
/// so frame cost tracks the charged unit, not item count.
///
/// Every rank is unique (resource plus key), so the unstable sort orders
/// exactly as a stable one would, without the stable sort's buffer.
pub(crate) fn admit_drain(
    items: &mut [DrainItem],
    budget: DrainBudget,
) -> Result<DrainAdmission, DrainBytesOverflow> {
    items.sort_unstable_by_key(|item| item.rank);
    let (mut charged, mut bytes) = (0u64, 0u64);
    for (admitted, item) in items.iter().enumerate() {
        let next = charged
            .checked_add(budget.charge(item))
            .ok_or(DrainBytesOverflow)?;
        if admitted > 0 && next > budget.limit() {
            return Ok(DrainAdmission { admitted, bytes });
        }
        charged = next;
        bytes = bytes.checked_add(item.bytes).ok_or(DrainBytesOverflow)?;
    }
    Ok(DrainAdmission {
        admitted: items.len(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(class: DrainClass, cluster_id: u32, bytes: u64) -> DrainItem {
        DrainItem {
            rank: DrainRank::sh(class, 0, cluster_id),
            bytes,
        }
    }

    fn pair(class: DrainClass, lead: u32, block_id: u32, half: u64) -> DrainItem {
        DrainItem::pair(
            DrainRank::lightmap_block(class, 0, lead, block_id),
            half,
            half,
        )
        .unwrap()
    }

    fn keys(items: &[DrainItem]) -> Vec<(StreamResource, u32)> {
        items
            .iter()
            .map(|item| (item.rank.resource(), item.rank.key()))
            .collect()
    }

    #[test]
    fn merged_drain_orders_mandatory_across_resources_before_optional() {
        let mut items = [
            pair(DrainClass::Prefetch, 0, 70, 1),
            sh(DrainClass::Prefetch, 9, 1),
            pair(DrainClass::Lead, 2048, 50, 1),
            pair(DrainClass::Lead, 1024, 51, 1),
            sh(DrainClass::Pinned, 4, 1),
            pair(DrainClass::Visible, 0, 60, 1),
            sh(DrainClass::Visible, 3, 1),
            sh(DrainClass::Hysteresis, 1, 1),
        ];
        admit_drain(&mut items, DrainBudget::PLAY).unwrap();

        use StreamResource::{LightmapBlock as Lm, Sh};
        assert_eq!(
            keys(&items),
            vec![
                (Sh, 3),
                (Lm, 60),
                (Sh, 4),
                (Lm, 51),
                (Lm, 50),
                (Sh, 9),
                (Lm, 70),
                (Sh, 1),
            ],
            "SH leads blocks at equal class; blocks order by lead"
        );
        assert!(
            items.iter().map(|item| item.rank.class.tier()).is_sorted(),
            "every mandatory item precedes every optional item"
        );
    }

    #[test]
    fn prefetch_priority_outranks_lead_within_the_band() {
        let near = DrainItem {
            rank: DrainRank::lightmap_block(DrainClass::Prefetch, 0, 1024, 1),
            bytes: 1,
        };
        let ranked = DrainItem {
            rank: DrainRank::lightmap_block(DrainClass::Prefetch, 3, 4096, 2),
            bytes: 1,
        };
        let mut items = [near, ranked];
        admit_drain(&mut items, DrainBudget::PLAY).unwrap();
        assert_eq!(items[0], ranked);
    }

    // P12 (CPU half): each drain admits at least one item, and a pair that
    // straddles the budget boundary is deferred whole.
    #[test]
    fn drain_budget_defers_a_straddling_block_pair_whole() {
        let mut items = [
            sh(DrainClass::Visible, 0, 4 * MIB),
            // Each half fits the 4 MiB left; the pair (6 MiB) does not.
            pair(DrainClass::Visible, 0, 7, 3 * MIB),
            sh(DrainClass::Pinned, 1, MIB),
        ];
        let admission = admit_drain(&mut items, DrainBudget::PLAY).unwrap();
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 1,
                bytes: 4 * MIB
            }
        );
        assert_eq!(
            items[1].rank.resource(),
            StreamResource::LightmapBlock,
            "the pair is next in line, not skipped past"
        );
    }

    #[test]
    fn drain_budget_admits_an_oversized_first_item_alone() {
        let mut items = [
            sh(DrainClass::Pinned, 0, 1),
            pair(
                DrainClass::Visible,
                0,
                7,
                MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
            ),
        ];
        let admission = admit_drain(&mut items, DrainBudget::PLAY).unwrap();
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 1,
                bytes: 2 * MAX_INSTALL_DECODED_BYTES_PER_DRAIN
            },
            "the whole oversized pair installs; nothing joins it"
        );
        assert_eq!(items[0].rank.resource(), StreamResource::LightmapBlock);
    }

    #[test]
    fn drain_budget_fills_to_the_limit_across_resources() {
        let mut items = [
            sh(DrainClass::Visible, 0, 4 * MIB),
            pair(DrainClass::Visible, 0, 7, 2 * MIB),
            sh(DrainClass::Prefetch, 1, 1),
        ];
        let admission = admit_drain(&mut items, DrainBudget::PLAY).unwrap();
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 2,
                bytes: MAX_INSTALL_DECODED_BYTES_PER_DRAIN
            }
        );
    }

    /// The time budget the sweep starts from: a quarter of a 60 Hz frame.
    const FOUR_MS: DrainBudget = DrainBudget::EstimatedMicros(4_000);

    #[test]
    fn install_estimate_is_each_resources_fixed_cost_plus_its_per_mib_cost() {
        use StreamResource::{LightmapBlock as Lm, Sh};
        for (resource, cost) in [
            (Lm, LIGHTMAP_PAIR_INSTALL_COST),
            (Sh, SH_CLUSTER_INSTALL_COST),
        ] {
            assert_eq!(estimated_install_us(resource, 0), cost.per_item_us);
            assert_eq!(
                estimated_install_us(resource, 4 * MIB),
                cost.per_item_us + 4 * cost.per_mib_us,
                "{resource:?}"
            );
            assert_eq!(
                estimated_install_us(resource, 1),
                cost.per_item_us + 1,
                "{resource:?}: a partial microsecond rounds up"
            );
        }
        assert_eq!(estimated_install_us(Lm, MIB), 15 + 195);
        assert_eq!(estimated_install_us(Sh, MIB), 68 + 1_080);
    }

    #[test]
    fn equal_decoded_bytes_cost_differently_per_resource() {
        let cluster = sh(DrainClass::Visible, 0, 2 * MIB);
        let block_pair = pair(DrainClass::Visible, 0, 7, MIB);
        assert_eq!(cluster.bytes, block_pair.bytes);
        assert_eq!(cluster.estimated_install_us(), 68 + 2 * 1_080);
        assert_eq!(block_pair.estimated_install_us(), 15 + 2 * 195);
    }

    // The time charge runs the byte path's admission: the same order, the
    // first item always, and a stop at the first item over budget even when
    // a later, smaller one would fit.
    #[test]
    fn time_admission_keeps_the_order_and_stops_at_the_first_item_over_budget() {
        let mut items = [
            pair(DrainClass::Prefetch, 0, 70, MIB / 2),
            sh(DrainClass::Visible, 1, 2 * MIB),
            pair(DrainClass::Visible, 0, 7, MIB / 2),
            sh(DrainClass::Visible, 0, 2 * MIB),
        ];
        let admission = admit_drain(&mut items, FOUR_MS).unwrap();
        use StreamResource::{LightmapBlock as Lm, Sh};
        assert_eq!(
            keys(&items),
            vec![(Sh, 0), (Sh, 1), (Lm, 7), (Lm, 70)],
            "the byte path's order"
        );
        // 2,228 µs, then 4,456 µs passes 4 ms: the 210 µs pair behind it
        // would fit but is not reached.
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 1,
                bytes: 2 * MIB
            }
        );
    }

    #[test]
    fn time_admission_admits_an_over_budget_first_item_alone() {
        let mut items = [
            sh(DrainClass::Visible, 0, 8 * MIB),
            pair(DrainClass::Visible, 0, 7, 1024),
        ];
        assert!(items[0].estimated_install_us() > 4_000);
        let admission = admit_drain(&mut items, FOUR_MS).unwrap();
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 1,
                bytes: 8 * MIB
            }
        );
    }

    // Lightmap pairs fit far more bytes under a time budget than the byte
    // cap lets in; SH fits fewer.
    #[test]
    fn a_lightmap_drain_past_8_mib_fits_4_ms_whole_and_an_sh_drain_over_it_is_cut() {
        let pairs = || (0..16).map(|block| pair(DrainClass::Visible, 0, block, MIB / 2));
        let mut lightmap: Vec<DrainItem> = pairs().collect();
        assert_eq!(
            admit_drain(&mut lightmap, FOUR_MS).unwrap(),
            DrainAdmission {
                admitted: 16,
                bytes: 16 * MIB
            },
            "16 × 210 µs = 3,360 µs"
        );
        let mut lightmap: Vec<DrainItem> = pairs().collect();
        assert_eq!(
            admit_drain(&mut lightmap, DrainBudget::PLAY)
                .unwrap()
                .admitted,
            8
        );

        let clusters = || (0..4).map(|cluster| sh(DrainClass::Visible, cluster, MIB));
        let mut sh_items: Vec<DrainItem> = clusters().collect();
        assert_eq!(
            admit_drain(&mut sh_items, FOUR_MS).unwrap(),
            DrainAdmission {
                admitted: 3,
                bytes: 3 * MIB
            },
            "3 × 1,148 µs fit; the fourth would reach 4,592 µs"
        );
        let mut sh_items: Vec<DrainItem> = clusters().collect();
        assert_eq!(
            admit_drain(&mut sh_items, DrainBudget::PLAY)
                .unwrap()
                .admitted,
            4
        );
    }
}
