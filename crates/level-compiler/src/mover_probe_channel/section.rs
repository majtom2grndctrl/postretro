// Assembling id 57 from the channel's bakes: one term record per (cell,
// light) with its probe mask and, where ids 41 and 45 as emitted hold no
// per-probe entry, its own tile once; each gating mover's visibility under
// it; the bake-only roster; and the per-mover ramp records.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::mover_probe_channel::{
    DirectRecordParts, MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE,
    MoverChannelBlock, MoverDirectRecord, MoverLightNamespace, MoverLightRampRecord,
    MoverPayloadPlacement, MoverProbeChannelParts, MoverProbeChannelSection,
};
use postretro_level_format::sh_reconstruct::Level;

use super::blocks::BounceBlocks;
use super::direct::{DirectEntry, MoverDirectBake, ProbeCells};
use crate::map_data::MapKinematicMover;

/// The resident byte bar each payload block is held against: level-format's
/// bars by default, or one lowered bar for both (the
/// `--mover-payload-resident-bar` test seam).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoverPayloadBars {
    pub(crate) direct: u64,
    pub(crate) bounce: u64,
}

impl Default for MoverPayloadBars {
    fn default() -> Self {
        Self {
            direct: MoverChannelBlock::Direct.resident_byte_bar(),
            bounce: MoverChannelBlock::Bounce.resident_byte_bar(),
        }
    }
}

impl MoverPayloadBars {
    /// Level-format's bars, or `bar` for both blocks.
    pub(crate) fn with_override(bar: Option<u64>) -> Self {
        bar.map_or_else(Self::default, |bar| Self {
            direct: bar,
            bounce: bar,
        })
    }

    fn of(&self, block: MoverChannelBlock) -> u64 {
        match block {
            MoverChannelBlock::Direct => self.direct,
            MoverChannelBlock::Bounce => self.bounce,
        }
    }

    /// Place each of `section`'s payloads: one past its bar streams with id
    /// 50's cells, the rest stay resident. Residency only: the payload's
    /// bytes are the same wherever they are placed.
    pub(crate) fn place(&self, section: &mut MoverProbeChannelSection) {
        for block in MoverChannelBlock::BOTH {
            let placement = if section.payload_bytes(block) > self.of(block) {
                MoverPayloadPlacement::Streamed
            } else {
                MoverPayloadPlacement::Resident
            };
            match block {
                MoverChannelBlock::Direct => section.direct_placement = placement,
                MoverChannelBlock::Bounce => section.bounce_placement = placement,
            }
        }
    }
}

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
/// its cell.
pub(crate) struct EmittedDirectEntries<'a> {
    pub(crate) selection: Option<&'a EntityShadowLightsSection>,
    pub(crate) direct: Option<&'a DirectShDeltaVolumesSection>,
    pub(crate) animated: Option<&'a AnimatedDirectShDeltaVolumesSection>,
}

impl EmittedDirectEntries<'_> {
    /// Whether the light's term at `cell` is stored per probe: an id-41
    /// entry at a dense cell for a static light (id 41's selection slots
    /// name AlphaLights indices), an id-45 entry for an animated one (id 45
    /// stays dense). Never for a bake-only light, which neither holds.
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
            MoverLightNamespace::BakeOnlyStaticLight => false,
        }
    }

    /// Whether records of `namespace` gate anything at load. With no id 45
    /// the renderer composes animated-baked records ungated: the light's id
    /// 45 term they gate was never added. So the compiler writes none, as
    /// they would be dead bytes. Static and bake-only records gate through
    /// ids 35 and 41 and their own tiles, so they always do.
    pub(crate) fn composes_gated(&self, namespace: MoverLightNamespace) -> bool {
        namespace != MoverLightNamespace::AnimatedBaked || self.animated.is_some()
    }
}

/// Id 57 holding `direct`'s entries and `bounce`'s blocks, or `None` when
/// there are none: the section is never written empty. Each light's entries
/// at one cell become one term record, masked to the probes where some
/// mover's `q₀` or `q₁` is below 255. `own_tile(cell, namespace, light)`
/// says whether the record carries its light's tile at those probes; a
/// namespace that requires one always does. `gated(namespace)` says whether
/// that namespace's records gate anything once loaded; a light in one that
/// does not writes no record. Bake-only lights, which `direct` names by
/// static-baked index, are written by rank among the bake-only lights it
/// holds. The listed cells are the union of the direct and bounce blocks'
/// cells. Each payload past its bar in `bars` streams with id 50's cells;
/// the section still holds it, for id 50 to carry.
pub(crate) fn channel_section(
    direct: &MoverDirectBake,
    bounce: &BounceBlocks,
    cells: &ProbeCells,
    ramps: &MoverRamps,
    bars: &MoverPayloadBars,
    own_tile: impl Fn(usize, MoverLightNamespace, u32) -> bool,
    gated: impl Fn(MoverLightNamespace) -> bool,
) -> anyhow::Result<Option<MoverProbeChannelSection>> {
    let mut records = Vec::new();
    for light in direct.lights.iter().filter(|light| gated(light.namespace)) {
        // Entries ascend by (cell, mover): one run per cell.
        for run in light.entries.chunk_by(|a, b| a.cell == b.cell) {
            let cell = run[0].cell;
            let carries_own_tile = light.namespace.requires_own_tile()
                || own_tile(cell as usize, light.namespace, light.light_index);
            records.extend(term_record(
                run,
                cells.valid_masks[cell as usize],
                MoverDirectRecord {
                    namespace: light.namespace,
                    carries_own_tile,
                    light_index: light.light_index,
                    probe_mask: 0,
                },
            ));
        }
    }
    let bake_only_roster = BakeOnlyRoster::new(&records);
    for part in &mut records {
        part.record.light_index =
            bake_only_roster.wire_index(part.record.namespace, part.record.light_index);
    }
    let mut section = MoverProbeChannelSection::assemble(
        MoverProbeChannelParts {
            affinity_dims: cells.affinity_dims,
            direct: records,
            bounce: bounce.bounce.clone(),
            depth: bounce.depth.clone(),
        },
        |cell| cells.valid_masks[cell as usize],
        |mover_id| ramps.of(mover_id),
    )?;
    if let Some(section) = &mut section {
        bars.place(section);
    }
    Ok(section)
}

/// One light's term record at one cell from its movers' entries there, each
/// holding every valid probe. The mask keeps the probes where some mover's
/// `q` is below 255 at either terminus; every other probe composes exactly
/// as with no section, so it stores nothing. The tile is the light's, the
/// same in every mover's entry, so it is stored once. `None` when no mover
/// changes the term at any probe.
fn term_record(
    run: &[DirectEntry],
    valid_mask: u64,
    mut record: MoverDirectRecord,
) -> Option<DirectRecordParts> {
    fn pair(entry: &DirectEntry, slot: usize) -> &[u8] {
        &entry.visibility[slot * MOVER_VISIBILITY_BYTES_PER_PROBE..]
            [..MOVER_VISIBILITY_BYTES_PER_PROBE]
    }
    let gates = |entry: &DirectEntry, slot: usize| pair(entry, slot).iter().any(|&q| q < u8::MAX);
    let mut kept_slots = Vec::new();
    let mut bits = valid_mask;
    let mut slot = 0;
    while bits != 0 {
        let local = bits.trailing_zeros();
        bits &= bits - 1;
        if run.iter().any(|entry| gates(entry, slot)) {
            record.probe_mask |= 1 << local;
            kept_slots.push(slot);
        }
        slot += 1;
    }
    if kept_slots.is_empty() {
        return None;
    }
    debug_assert!(
        run.windows(2).all(|pair| pair[0].tiles == pair[1].tiles),
        "one light's tile at one cell is the same for every mover"
    );
    let movers = run
        .iter()
        .filter(|entry| kept_slots.iter().any(|&slot| gates(entry, slot)))
        .map(|entry| {
            let visibility = kept_slots
                .iter()
                .flat_map(|&slot| pair(entry, slot).iter().copied())
                .collect();
            (entry.mover_id, visibility)
        })
        .collect();
    let own_tiles = if record.carries_own_tile {
        kept_slots
            .iter()
            .flat_map(|&slot| {
                run[0].tiles[slot * MOVER_IRRADIANCE_TILE_F16_COUNT..]
                    [..MOVER_IRRADIANCE_TILE_F16_COUNT]
                    .iter()
                    .copied()
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(DirectRecordParts {
        cell: run[0].cell,
        record,
        movers,
        own_tiles,
    })
}

/// The bake-only lights the records name, by static-baked index ascending:
/// the section's namespace-2 roster, whose rank is each light's wire index.
/// A light with no record is absent, so the indices the section names run
/// densely from 0, as the loader requires.
struct BakeOnlyRoster(Vec<u32>);

impl BakeOnlyRoster {
    fn new(records: &[DirectRecordParts]) -> Self {
        let mut lights: Vec<u32> = records
            .iter()
            .filter(|part| part.record.namespace == MoverLightNamespace::BakeOnlyStaticLight)
            .map(|part| part.record.light_index)
            .collect();
        lights.sort_unstable();
        lights.dedup();
        Self(lights)
    }

    /// `light_index` as the section writes it.
    fn wire_index(&self, namespace: MoverLightNamespace, light_index: u32) -> u32 {
        if namespace != MoverLightNamespace::BakeOnlyStaticLight {
            return light_index;
        }
        let rank = self
            .0
            .binary_search(&light_index)
            .expect("every bake-only light with a record is on the roster");
        rank as u32
    }
}
