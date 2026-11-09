// MoverProbeChannel (id 57) load: structural decode, the checks against ids
// 34/35, 43, 45 and 59, a streamed payload resolved against id 50 (and read
// whole for a whole-load), and the ramp hand-off into mover runtime data.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

use std::collections::BTreeMap;

use postretro_level_format::SectionId;
use postretro_level_format::delta_sh_volumes::AFFINITY_FACTOR;
use postretro_level_format::mover_probe_channel::{
    MoverChannelBlock, MoverLightNamespace, MoverPayloadPlacement, MoverProbeChannelError,
    MoverProbeChannelSection, MoverStreamedRow, parse_streamed_rows,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::probe_depth_tiles::MAX_PROBE_DEPTH_TILES;
use postretro_level_format::sh_volume::{OctahedralShProbe, validate_storage_levels_against_delta};

use super::lighting::LightingDecode;
use crate::prl_container::PrlContainer;
use crate::prl_lighting::{expected_affinity_dims, valid_probe_mask_for_grid_cell};
use crate::sh_stream::ShStreamManifest;
use crate::{KinematicGeometry, MapLight, MoverLightRamp, PrlLoadError, ShadowType};

type Error = MoverProbeChannelError;

/// What id 57 is checked against, from whichever id-34 metadata the load
/// holds: the streamed base in a streaming load, the id-34 body otherwise.
pub(super) struct MoverProbeChannelInputs<'a> {
    /// Id 34's probe grid, which id 35 shares by validation.
    pub(super) grid_dimensions: [u32; 3],
    pub(super) tile_dimension: u32,
    pub(super) tile_border: u32,
    pub(super) probes: &'a [OctahedralShProbe],
    /// Id 18, with each light's animated slot resolved.
    pub(super) lights: &'a [MapLight],
    /// Id 34's animation descriptor count: the roster id 45 numbers its
    /// lights by.
    pub(super) animated_roster_len: usize,
    pub(super) id59_tile_count: usize,
    /// Id 43's movers.
    pub(super) mover_ids: &'a [u32],
    /// Whether this load's id 50 lists this section as a source: where a
    /// streamed payload is placed.
    pub(super) id50_carries_payload: bool,
}

/// Read optional id 57, check it, and hand each mover it names its author
/// ramp. Absent is `None` and leaves every mover on the identity ramp;
/// malformed or unresolved refuses the level by name. A streamed payload
/// must resolve against id 50 (`id50`, validated whatever the SH mode).
/// When SH streams it stays with id 50's cells, read as they become
/// resident; a whole-load (`streaming` is `None`, as under
/// `POSTRETRO_SH_STREAMING=off`) reads it whole now, so the section holds
/// the same payload a resident placement would.
pub(super) fn read_mover_probe_channel(
    container: &PrlContainer,
    streaming: Option<&ShStreamManifest>,
    id50: Option<&ShStreamManifest>,
    lighting: &LightingDecode,
    kinematic_geometry: &mut KinematicGeometry,
) -> Result<Option<MoverProbeChannelSection>, PrlLoadError> {
    let Some(data) = container.read_section(SectionId::MoverProbeChannel as u32)? else {
        return Ok(None);
    };
    let (grid_dimensions, tile_dimension, tile_border, probes, animated_roster_len) =
        match (streaming, lighting.sh_volume.as_ref()) {
            (Some(manifest), _) => {
                let base = manifest.base();
                (
                    base.grid_dimensions,
                    base.tile_dimension,
                    base.tile_border,
                    &base.probes[..],
                    base.animation_descriptors.len(),
                )
            }
            (None, Some(base)) => (
                base.grid_dimensions,
                base.tile_dimension,
                base.tile_border,
                &base.probes[..],
                base.animation_descriptors.len(),
            ),
            // A whole load refuses a level without id 34 before this point.
            (None, None) => ([0; 3], 0, 0, &[][..], 0),
        };
    let mover_ids: Vec<u32> = kinematic_geometry
        .movers
        .iter()
        .map(|mover| mover.mover_id)
        .collect();
    let id50_carries_payload =
        id50.is_some_and(|manifest| manifest.carries_source(SectionId::MoverProbeChannel));
    let mut section = decode_mover_probe_channel(
        &data,
        &MoverProbeChannelInputs {
            grid_dimensions,
            tile_dimension,
            tile_border,
            probes,
            lights: &lighting.lights,
            animated_roster_len,
            id59_tile_count: lighting
                .probe_depth_tiles
                .as_ref()
                .map_or(0, |tiles| tiles.tile_count()),
            mover_ids: &mover_ids,
            id50_carries_payload,
        },
    )?;
    if streaming.is_none()
        && section.streams()
        && let Some(manifest) = id50
    {
        read_streamed_payload_whole(manifest, &mut section)?;
    }
    apply_light_ramps(&section, kinematic_geometry);
    log::info!(
        "[PRL] MoverProbeChannel: {} mover(s) over {} cell(s), {} direct record(s) with {} mover entr(y/ies) ({:?}, {} B), {} bounce entr(y/ies) ({:?}, {} B), {} depth record(s) ({} live tile(s))",
        section.ramps.len(),
        section.cells.len(),
        section.direct_records.len(),
        section.direct_movers.len(),
        section.direct_placement,
        section.direct_payload_bytes(),
        section.bounce_entries.len(),
        section.bounce_placement,
        section.bounce_payload_bytes(),
        section.depth_records.len(),
        section.live_tile_count(),
    );
    Ok(Some(section))
}

/// A whole-load's read of a streamed payload: every id-50 chunk decoded and
/// verified through the manifest, and its id-57 rows gathered into the
/// section. A cell several clusters cover is carried by each, and the
/// copies must agree. Only the rows' bytes are held, never every chunk.
fn read_streamed_payload_whole(
    manifest: &ShStreamManifest,
    section: &mut MoverProbeChannelSection,
) -> Result<(), PrlLoadError> {
    let mut rows: BTreeMap<u32, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
    for cluster_id in 0..manifest.cluster_count() {
        let chunk = manifest.read_and_decode_cluster(cluster_id)?;
        let Some(block) = chunk.mover_channel_rows() else {
            continue;
        };
        for row in parse_streamed_rows(block)? {
            match rows.get(&row.cell) {
                Some((direct, bounce)) if *direct != row.direct || *bounce != row.bounce => {
                    return Err(Error::StreamedPayloadMismatch {
                        block: if *direct != row.direct {
                            MoverChannelBlock::Direct
                        } else {
                            MoverChannelBlock::Bounce
                        },
                        cell: row.cell,
                        detail: "differs between two clusters that carry it".into(),
                    }
                    .into());
                }
                Some(_) => {}
                None => {
                    rows.insert(row.cell, (row.direct.to_vec(), row.bounce.to_vec()));
                }
            }
        }
    }
    section.attach_streamed_payload(rows.iter().map(|(&cell, (direct, bounce))| {
        MoverStreamedRow {
            cell,
            direct,
            bounce,
        }
    }))?;
    Ok(())
}

/// Decode id 57's bytes and check them against the rest of the level.
pub(super) fn decode_mover_probe_channel(
    data: &[u8],
    inputs: &MoverProbeChannelInputs<'_>,
) -> Result<MoverProbeChannelSection, Error> {
    let section = MoverProbeChannelSection::from_bytes(data)?;
    validate_mover_probe_channel(&section, inputs)?;
    Ok(section)
}

/// The checks id 57 needs from other sections. The decode already pinned its
/// own structure.
fn validate_mover_probe_channel(
    section: &MoverProbeChannelSection,
    inputs: &MoverProbeChannelInputs<'_>,
) -> Result<(), Error> {
    let expected = expected_affinity_dims(inputs.grid_dimensions, AFFINITY_FACTOR);
    if section.affinity_dims != expected {
        return Err(Error::GridMismatch {
            found: section.affinity_dims,
            grid: inputs.grid_dimensions,
            expected,
        });
    }
    if inputs.tile_dimension != DEFAULT_IRRADIANCE_TILE_DIMENSION
        || inputs.tile_border != DEFAULT_IRRADIANCE_TILE_BORDER
    {
        return Err(Error::TileGeometry {
            dimension: inputs.tile_dimension,
            border: inputs.tile_border,
            expected_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            expected_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        });
    }
    for listed in &section.cells {
        let cell = listed.cell as usize;
        let expected = valid_probe_mask_for_grid_cell(
            inputs.grid_dimensions,
            inputs.probes,
            section.affinity_dims,
            cell,
        );
        if listed.valid_probe_mask != expected {
            return Err(Error::ValidityMismatch {
                cell,
                found: listed.valid_probe_mask,
                expected,
            });
        }
    }

    // Listed cells hold direct or bounce entries, so they compose at L0, as a
    // grid-matched delta entry's cells do: the base may never be stored
    // coarser there. The shared check reads a dense CSR, one entry per
    // listed cell.
    let cell_count = section.affinity_cell_count();
    let mut offsets = vec![0u32; cell_count + 1];
    for cell in section.listed_cells() {
        offsets[cell + 1] = 1;
    }
    for cell in 0..cell_count {
        offsets[cell + 1] += offsets[cell];
    }
    validate_storage_levels_against_delta(
        inputs.grid_dimensions,
        inputs.probes,
        &vec![0u8; cell_count],
        &offsets,
    )
    .map_err(|error| Error::StorageCeiling {
        message: error.to_string(),
    })?;

    // Ramp records cover exactly the movers every block names.
    if let Some(ramp) = section
        .ramps
        .iter()
        .find(|ramp| !inputs.mover_ids.contains(&ramp.mover_id))
    {
        return Err(Error::UnresolvedMover {
            mover_id: ramp.mover_id,
        });
    }

    let bake_only_roster_len = section.bake_only_light_count();
    for (index, record) in section.direct_records.iter().enumerate() {
        check_light(
            index,
            record.namespace,
            record.light_index,
            inputs,
            bake_only_roster_len,
        )?;
    }

    let lattice: u64 = inputs
        .grid_dimensions
        .iter()
        .map(|&dim| u64::from(dim))
        .product();
    for (record, depth) in section.depth_records.iter().enumerate() {
        let probe = depth.probe_index;
        if u64::from(probe) >= lattice {
            return Err(Error::DepthProbeOutOfRange {
                record,
                probe,
                lattice,
            });
        }
        if inputs
            .probes
            .get(probe as usize)
            .is_none_or(|metadata| metadata.validity == 0)
        {
            return Err(Error::DepthProbeNotValid { record, probe });
        }
    }
    // Closed-pose tiles stay on the CPU; the tile texture holds id 59's tiles
    // and one live tile per probe carrying mover depth.
    let live = section.live_tile_count();
    if inputs.id59_tile_count + live > MAX_PROBE_DEPTH_TILES as usize {
        return Err(Error::TileRange {
            id59: inputs.id59_tile_count,
            live,
            max: MAX_PROBE_DEPTH_TILES,
        });
    }

    // A streamed payload is placed with id 50's cells: id 50 must list this
    // section, or the payload is nowhere this load can read.
    if let Some(block) = MoverChannelBlock::BOTH
        .into_iter()
        .find(|&block| section.placement(block) == MoverPayloadPlacement::Streamed)
        && !inputs.id50_carries_payload
    {
        return Err(Error::StreamedPayloadUnresolved { block });
    }
    Ok(())
}

/// A term record's light must exist in its namespace: a static lightmap
/// light of id 18 (id 35's set), an animated-baked light of the roster, or a
/// bake-only light of the section's own roster, whose indices run densely
/// below `bake_only_roster_len`.
fn check_light(
    record: usize,
    namespace: MoverLightNamespace,
    light_index: u32,
    inputs: &MoverProbeChannelInputs<'_>,
    bake_only_roster_len: usize,
) -> Result<(), Error> {
    let refusal = |detail: String| Error::LightOutOfNamespace {
        record,
        namespace,
        light_index,
        detail,
    };
    match namespace {
        MoverLightNamespace::StaticLight => {
            let Some(light) = inputs.lights.get(light_index as usize) else {
                return Err(refusal(format!(
                    "past AlphaLights (id 18)'s {} light(s)",
                    inputs.lights.len()
                )));
            };
            if light.is_dynamic
                || light.shadow_type != ShadowType::StaticLightMap
                || light.animated_slot.is_some()
            {
                return Err(refusal(
                    "which is not a static lightmap light of DirectShVolume (id 35)".into(),
                ));
            }
        }
        MoverLightNamespace::AnimatedBaked => {
            if light_index as usize >= inputs.animated_roster_len {
                return Err(refusal(format!(
                    "past the {}-light animated-baked roster",
                    inputs.animated_roster_len
                )));
            }
        }
        MoverLightNamespace::BakeOnlyStaticLight => {
            if light_index as usize >= bake_only_roster_len {
                return Err(refusal(format!(
                    "past the section's {bake_only_roster_len}-light bake-only roster, whose \
                     indices run densely from 0"
                )));
            }
        }
    }
    Ok(())
}

/// Each mover the section names takes its author ramp; every other mover
/// keeps the identity ramp it loaded with. Only plain numbers cross: no
/// section type reaches the game.
fn apply_light_ramps(
    section: &MoverProbeChannelSection,
    kinematic_geometry: &mut KinematicGeometry,
) {
    for mover in &mut kinematic_geometry.movers {
        if let Some(ramp) = section.ramp_of(mover.mover_id) {
            mover.light_ramp = MoverLightRamp {
                start: ramp.start,
                end: ramp.end,
            };
        }
    }
}

#[cfg(test)]
#[path = "mover_probe_channel_tests.rs"]
mod tests;
