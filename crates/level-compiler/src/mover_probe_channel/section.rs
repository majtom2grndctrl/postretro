// Assembling id 57 from the channel's bakes: the direct block in wire order,
// each entry's own-tile flag against ids 41 and 45 as emitted, and the
// per-mover ramp records.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::mover_probe_channel::{
    MoverBouncePlacement, MoverDirectEntry, MoverLightNamespace, MoverLightRampRecord,
    MoverProbeChannelSection,
};
use postretro_level_format::sh_reconstruct::Level;

use super::direct::{DirectEntry, MoverDirectBake, ProbeCells};
use crate::map_data::MapKinematicMover;

/// Every mover's author ramp, ascending by id.
pub(crate) struct MoverRamps(Vec<MoverLightRampRecord>);

impl MoverRamps {
    pub(crate) fn new(movers: &[MapKinematicMover]) -> Self {
        let mut ramps: Vec<MoverLightRampRecord> = movers
            .iter()
            .map(|mover| MoverLightRampRecord {
                mover_id: mover.mover_id,
                start: mover.light_ramp_start,
                end: mover.light_ramp_end,
            })
            .collect();
        ramps.sort_by_key(|ramp| ramp.mover_id);
        Self(ramps)
    }

    fn of(&self, mover_id: u32) -> anyhow::Result<MoverLightRampRecord> {
        self.0
            .binary_search_by_key(&mover_id, |ramp| ramp.mover_id)
            .map(|at| self.0[at])
            .map_err(|_| {
                anyhow::anyhow!("mover probe channel names mover {mover_id}, which the map lacks")
            })
    }
}

/// Ids 41 and 45 as they will be emitted: after the exact-zero drop, id
/// 41's classification and the valid-probe compaction. An entry carries its
/// light's own tile unless one of these holds the light's term per probe at
/// its cell (P22).
pub(crate) struct EmittedDirectEntries<'a> {
    pub(crate) selection: Option<&'a EntityShadowLightsSection>,
    pub(crate) direct: Option<&'a DirectShDeltaVolumesSection>,
    pub(crate) animated: Option<&'a AnimatedDirectShDeltaVolumesSection>,
}

impl EmittedDirectEntries<'_> {
    /// Whether the light's term at `cell` is stored per probe: an id-41
    /// entry at a dense cell for a static light (id 41's selection slots
    /// name AlphaLights indices), an id-45 entry for an animated one (id 45
    /// stays dense).
    pub(crate) fn holds_per_probe(
        &self,
        cell: usize,
        namespace: MoverLightNamespace,
        light_index: u32,
    ) -> bool {
        let entries_at = |offsets: &[u32], lights: &[u32], wanted: u32| {
            offsets
                .get(cell..=cell + 1)
                .is_some_and(|range| lights[range[0] as usize..range[1] as usize].contains(&wanted))
        };
        match namespace {
            MoverLightNamespace::StaticLight => {
                let (Some(selection), Some(direct)) = (self.selection, self.direct) else {
                    return false;
                };
                let Some(slot) = selection
                    .light_indices
                    .iter()
                    .position(|&alpha| alpha == light_index)
                else {
                    return false;
                };
                direct.cell_levels.get(cell) == Some(&Level::L0.to_u8())
                    && entries_at(
                        &direct.affinity_offsets,
                        &direct.affinity_lights,
                        slot as u32,
                    )
            }
            MoverLightNamespace::AnimatedBaked => self.animated.is_some_and(|animated| {
                animated.cell_levels.get(cell) == Some(&Level::L0.to_u8())
                    && entries_at(
                        &animated.affinity_offsets,
                        &animated.affinity_lights,
                        light_index,
                    )
            }),
        }
    }
}

/// Id 57 holding `direct`'s entries, or `None` when there are none: the
/// section is never written empty. `own_tile(cell, namespace, light)` says
/// whether an entry carries its light's tile. The bounce block is empty and
/// resident and there is no depth block, until those halves bake.
pub(crate) fn channel_section(
    direct: &MoverDirectBake,
    cells: &ProbeCells,
    ramps: &MoverRamps,
    own_tile: impl Fn(usize, MoverLightNamespace, u32) -> bool,
) -> anyhow::Result<Option<MoverProbeChannelSection>> {
    let mut entries: Vec<(MoverDirectEntry, &DirectEntry)> = direct
        .lights
        .iter()
        .flat_map(|light| {
            light.entries.iter().map(|entry| {
                (
                    MoverDirectEntry {
                        mover_id: entry.mover_id,
                        namespace: light.namespace,
                        carries_own_tile: own_tile(
                            entry.cell as usize,
                            light.namespace,
                            light.light_index,
                        ),
                        light_index: light.light_index,
                    },
                    entry,
                )
            })
        })
        .collect();
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort_by_key(|(record, entry)| (entry.cell, record.order_key()));

    let cell_count = cells.cell_count();
    let mut direct_offsets = vec![0u32; cell_count + 1];
    for (_, entry) in &entries {
        direct_offsets[entry.cell as usize + 1] += 1;
    }
    for cell in 0..cell_count {
        direct_offsets[cell + 1] += direct_offsets[cell];
    }
    let mut visibility = Vec::new();
    let mut own_tiles = Vec::new();
    for (record, entry) in &entries {
        visibility.extend_from_slice(&entry.visibility);
        if record.carries_own_tile {
            own_tiles.extend_from_slice(&entry.tiles);
        }
    }
    let mut mover_ids: Vec<u32> = entries.iter().map(|(record, _)| record.mover_id).collect();
    mover_ids.sort_unstable();
    mover_ids.dedup();
    let ramps = mover_ids
        .into_iter()
        .map(|mover_id| ramps.of(mover_id))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(MoverProbeChannelSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: cells.affinity_dims,
        bounce_placement: MoverBouncePlacement::Resident,
        valid_probe_masks: cells.valid_masks.clone(),
        direct_offsets,
        direct_entries: entries.iter().map(|(record, _)| *record).collect(),
        visibility,
        own_tiles,
        bounce_offsets: vec![0; cell_count + 1],
        bounce_entries: Vec::new(),
        bounce_tiles: Vec::new(),
        depth_records: Vec::new(),
        depth_tiles: Vec::new(),
        ramps,
    }))
}
