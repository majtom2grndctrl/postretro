//! Test only: the SH sections the mover channel's priced windows hold,
//! routed through the live-bytes ledger by capacity.
//! See: context/lib/testing_guide.md §3 (Resource bounds)

use std::collections::BTreeMap;

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;

use crate::live_bytes::{Held, hold};

fn bytes<T>(vec: &Vec<T>) -> u64 {
    (vec.capacity() * size_of::<T>()) as u64
}

/// The ledger guards a run holds, by name: holding again under a name
/// drops the guard before it, so a section that shrinks or is replaced is
/// counted at its current capacity.
#[derive(Default)]
pub(super) struct Windows(BTreeMap<&'static str, Held>);

impl Windows {
    pub(super) fn hold(&mut self, kind: &'static str, bytes: u64) {
        self.0.remove(kind);
        self.0.insert(kind, hold(kind, bytes));
    }

    pub(super) fn release(&mut self, kind: &'static str) {
        self.0.remove(kind);
    }

    pub(super) fn delta(&mut self, kind: &'static str, section: Option<&DeltaShVolumesSection>) {
        self.hold(
            kind,
            section.map_or(0, |section| {
                bytes(&section.animation_descriptor_indices)
                    + bytes(&section.valid_probe_masks)
                    + bytes(&section.cell_levels)
                    + bytes(&section.affinity_offsets)
                    + bytes(&section.affinity_lights)
                    + bytes(&section.delta_subblocks)
            }),
        );
    }

    pub(super) fn direct_delta(
        &mut self,
        kind: &'static str,
        section: Option<&DirectShDeltaVolumesSection>,
    ) {
        self.hold(
            kind,
            section.map_or(0, |section| {
                bytes(&section.valid_probe_masks)
                    + bytes(&section.cell_levels)
                    + bytes(&section.affinity_offsets)
                    + bytes(&section.affinity_lights)
                    + bytes(&section.delta_subblocks)
            }),
        );
    }

    pub(super) fn animated_delta(
        &mut self,
        kind: &'static str,
        section: Option<&AnimatedDirectShDeltaVolumesSection>,
    ) {
        self.hold(
            kind,
            section.map_or(0, |section| {
                bytes(&section.animation_descriptor_indices)
                    + bytes(&section.valid_probe_masks)
                    + bytes(&section.cell_levels)
                    + bytes(&section.affinity_offsets)
                    + bytes(&section.affinity_lights)
                    + bytes(&section.delta_subblocks)
            }),
        );
    }

    pub(super) fn direct_volume(
        &mut self,
        kind: &'static str,
        section: Option<&DirectShVolumeSection>,
    ) {
        self.hold(kind, section.map_or(0, |section| bytes(&section.atlas)));
    }

    pub(super) fn entity_shadow(&mut self, section: Option<&EntityShadowLightsSection>) {
        self.hold(
            "entity-shadow section",
            section.map_or(0, |section| bytes(&section.light_indices)),
        );
    }

    pub(super) fn mover_channel(
        &mut self,
        kind: &'static str,
        section: Option<&MoverProbeChannelSection>,
    ) {
        self.hold(kind, section.map_or(0, channel_bytes));
    }
}

/// Id 57's heap, by capacity.
pub(super) fn channel_bytes(section: &MoverProbeChannelSection) -> u64 {
    bytes(&section.cells)
        + bytes(&section.mover_records)
        + section
            .mover_records
            .iter()
            .map(|record| bytes(&record.knots))
            .sum::<u64>()
        + bytes(&section.direct_offsets)
        + bytes(&section.direct_records)
        + bytes(&section.mover_offsets)
        + bytes(&section.direct_movers)
        + bytes(&section.visibility)
        + bytes(&section.own_tiles)
        + bytes(&section.bounce_offsets)
        + bytes(&section.bounce_entries)
        + section
            .bounce_entries
            .iter()
            .map(|entry| bytes(&entry.poses))
            .sum::<u64>()
        + bytes(&section.bounce_tiles)
        + bytes(&section.depth_records)
        + section
            .depth_records
            .iter()
            .map(|record| bytes(&record.marks))
            .sum::<u64>()
        + bytes(&section.depth_tiles)
}
