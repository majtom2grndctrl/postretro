//! The one per-drain install budget and admission order for every resource.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::cmp::Reverse;

use thiserror::Error;

use super::request::{ReadTier, StreamResource};

/// Bytes handed to the renderer per drain, across every resource: SH
/// decoded bytes, and lightmap block upload bytes (blocks upload as stored,
/// so their decoded size is their stored size). The first item is always
/// admitted, so one oversized item still installs.
pub(crate) const MAX_INSTALL_DECODED_BYTES_PER_DRAIN: u64 = 8 * 1024 * 1024;

/// The per-drain budget while a level entry is Settling. No world frame is
/// drawn, so a drain costs only loading-tree cadence; a larger budget shortens
/// the hold.
pub(crate) const SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN: u64 = 32 * 1024 * 1024;

/// A ready item's class on the one scale every resource shares. Declaration
/// order is admission order; `Visible` through `Lead` form the mandatory
/// tier, the rest the optional tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DrainClass {
    Visible,
    Pinned,
    /// A lightmap block mandatory only through the camera cell's baked set
    /// within lead L. SH has no counterpart.
    Lead,
    SeamWarm,
    /// SH warm-set prefetch, and the lightmap prefetch band.
    Prefetch,
    Hysteresis,
}

impl DrainClass {
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
/// (nonzero only in the prefetch band), then lead, then block id.
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
}

/// The leading `admitted` items of the ordered list install this drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrainAdmission {
    pub(crate) admitted: usize,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("drain install bytes overflow u64")]
pub(crate) struct DrainBytesOverflow;

/// Orders a merged ready list for admission, then admits the leading items
/// that fit `budget` (normally [`MAX_INSTALL_DECODED_BYTES_PER_DRAIN`]). The first always fits
/// whatever its size, so an oversized item cannot stall residency. Admission
/// stops at the first item over budget rather than skipping ahead to
/// smaller, lower-ranked work, so frame cost tracks bytes, not item count.
///
/// Every rank is unique (resource plus key), so the unstable sort orders
/// exactly as a stable one would, without the stable sort's buffer.
pub(crate) fn admit_drain(
    items: &mut [DrainItem],
    budget: u64,
) -> Result<DrainAdmission, DrainBytesOverflow> {
    items.sort_unstable_by_key(|item| item.rank);
    let mut total = 0u64;
    for (admitted, item) in items.iter().enumerate() {
        let next = total.checked_add(item.bytes).ok_or(DrainBytesOverflow)?;
        if admitted > 0 && next > budget {
            return Ok(DrainAdmission {
                admitted,
                bytes: total,
            });
        }
        total = next;
    }
    Ok(DrainAdmission {
        admitted: items.len(),
        bytes: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

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
        admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN).unwrap();

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
        admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN).unwrap();
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
        let admission = admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN).unwrap();
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
        let admission = admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN).unwrap();
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
        let admission = admit_drain(&mut items, MAX_INSTALL_DECODED_BYTES_PER_DRAIN).unwrap();
        assert_eq!(
            admission,
            DrainAdmission {
                admitted: 2,
                bytes: MAX_INSTALL_DECODED_BYTES_PER_DRAIN
            }
        );
    }
}
