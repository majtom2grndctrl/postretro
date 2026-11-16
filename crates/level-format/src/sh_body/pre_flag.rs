// Test support: converts a PRL between the pre-flag SH section format and the current one.
// See: context/lib/testing_guide.md §3 (Slow / cold-bake suites)

use std::collections::HashSet;
use std::io::Cursor;

use super::{FlagLayout, ShBodyPresence, flag_layout, invalid_data, read_field};
use crate::cluster_sh_payloads::ClusterShPayloadsSection;
use crate::{SectionBlob, SectionId, read_container, section_data_from_bytes, write_prl};

/// A PRL packed before ids 27, 34, 35, 41 and 45 carried a body-presence
/// field, rewritten in the current format with every body present.
///
/// For each of the five sections in the table: its version goes up by one and
/// a present flag (1) is inserted where its old fixed header ended. Id 50's
/// source records for those sections take the bumped versions. Every other
/// section, the table order and every table version are kept, and the
/// container is rewritten with [`write_prl`]. Fails when one of the five
/// sections (or an id-50 source naming one) is not at its pre-flag version,
/// when the table lists a section id twice, or when id 50 lists id 57 as a
/// source (see [`downgrade_to_pre_flag_prl`]).
pub fn upgrade_pre_flag_prl(prl: &[u8]) -> crate::Result<Vec<u8>> {
    rewrite(prl, Direction::Upgrade)
}

/// The inverse of [`upgrade_pre_flag_prl`]: a current PRL whose five SH
/// sections all carry their bodies, rewritten in the pre-flag format. Fails
/// on an omitted body, which the pre-flag format cannot express.
///
/// Both directions refuse a PRL whose id 50 lists id 57 as a source. Callers
/// comparing against an older compiler relabel id 57's epoch, and id 50's
/// record of that epoch would not follow. The dev maps keep id 57 resident.
pub fn downgrade_to_pre_flag_prl(prl: &[u8]) -> crate::Result<Vec<u8>> {
    rewrite(prl, Direction::Downgrade)
}

#[derive(Clone, Copy)]
enum Direction {
    Upgrade,
    Downgrade,
}

impl Direction {
    /// The version a section must carry on input, and the one it leaves with.
    fn versions(self, layout: &FlagLayout) -> (u32, u32) {
        let current = layout.expected_version;
        match self {
            Self::Upgrade => (current - 1, current),
            Self::Downgrade => (current, current - 1),
        }
    }
}

fn rewrite(prl: &[u8], direction: Direction) -> crate::Result<Vec<u8>> {
    let meta = read_container(&mut Cursor::new(prl))?;
    let mut blobs = Vec::with_capacity(meta.sections.len());
    let mut seen = HashSet::with_capacity(meta.sections.len());
    for entry in &meta.sections {
        // Sections resolve by id (first match), so a repeated id would copy
        // the first one's bytes into both entries.
        if !seen.insert(entry.section_id) {
            return Err(invalid_data(format!(
                "section {} is listed twice in the section table",
                entry.section_id
            )));
        }
        let data = section_data_from_bytes(prl, &meta, entry.section_id)?
            .expect("a table entry resolves to its own section");
        let section = SectionId::from_u32(entry.section_id);
        let data = match section {
            Some(SectionId::ClusterShPayloads) => rewrite_cluster_sources(data, direction)?,
            Some(id) => match flag_layout(id) {
                Some(layout) => rewrite_flagged(id, &layout, data, direction)?,
                None => data.to_vec(),
            },
            None => data.to_vec(),
        };
        blobs.push(SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data,
        });
    }
    let mut out = Vec::with_capacity(prl.len());
    write_prl(&mut out, &blobs)?;
    Ok(out)
}

/// One of the five sections, its version moved and its flag inserted or
/// removed at the end of the fixed header.
fn rewrite_flagged(
    section: SectionId,
    layout: &FlagLayout,
    data: &[u8],
    direction: Direction,
) -> crate::Result<Vec<u8>> {
    let width = layout.field_width;
    let flag_at = layout.header_len - width;
    let (from, to) = direction.versions(layout);
    let header_len = match direction {
        Direction::Upgrade => flag_at,
        Direction::Downgrade => layout.header_len,
    };
    if data.len() < header_len {
        return Err(invalid_data(format!(
            "section {} is {} bytes, shorter than its {header_len}-byte fixed header",
            section as u32,
            data.len()
        )));
    }
    let version = read_field(data, 0, width);
    if version != from {
        return Err(invalid_data(format!(
            "section {} internal version {version}, expected {from}",
            section as u32
        )));
    }
    let to_bytes = to.to_le_bytes();
    let mut out = Vec::with_capacity(data.len() + width);
    out.extend_from_slice(&to_bytes[..width]);
    match direction {
        Direction::Upgrade => {
            out.extend_from_slice(&data[width..flag_at]);
            let flag = u32::from(ShBodyPresence::Present.flag()).to_le_bytes();
            out.extend_from_slice(&flag[..width]);
            out.extend_from_slice(&data[flag_at..]);
        }
        Direction::Downgrade => {
            let flag = read_field(data, flag_at, width);
            if flag != u32::from(ShBodyPresence::Present.flag()) {
                return Err(invalid_data(format!(
                    "section {} body-presence flag {flag}: the pre-flag format holds bodies only",
                    section as u32
                )));
            }
            out.extend_from_slice(&data[width..flag_at]);
            out.extend_from_slice(&data[layout.header_len..]);
        }
    }
    Ok(out)
}

/// Id 50 with the source records of the five sections moved to the versions
/// those sections now carry. The payload is kept as it is.
fn rewrite_cluster_sources(data: &[u8], direction: Direction) -> crate::Result<Vec<u8>> {
    let id50_error = |error: crate::cluster_sh_payloads::ClusterShPayloadsError| {
        invalid_data(format!("section 50: {error}"))
    };
    let metadata_len =
        ClusterShPayloadsSection::metadata_len_from_header(data).map_err(id50_error)?;
    if metadata_len > data.len() {
        return Err(invalid_data(format!(
            "section 50 is {} bytes, shorter than its {metadata_len}-byte metadata",
            data.len()
        )));
    }
    let mut section =
        ClusterShPayloadsSection::from_metadata_bytes(&data[..metadata_len], data.len() as u64)
            .map_err(id50_error)?;
    // Callers relabel id 57's epoch when comparing across compilers; id 50's
    // record of that epoch would keep the old one.
    let mover_channel = SectionId::MoverProbeChannel as u32;
    if section
        .sources
        .iter()
        .any(|s| s.section_id == mover_channel)
    {
        let subject = match direction {
            Direction::Upgrade => "baseline",
            Direction::Downgrade => "PRL",
        };
        return Err(invalid_data(format!(
            "{subject} streams id 57 through id 50; the epoch relabel cannot follow it"
        )));
    }
    for source in &mut section.sources {
        let Some(layout) = SectionId::from_u32(source.section_id).and_then(flag_layout) else {
            continue;
        };
        let (from, to) = direction.versions(&layout);
        if source.internal_version != from {
            return Err(invalid_data(format!(
                "section 50 records source {} at version {}, expected {from}",
                source.section_id, source.internal_version
            )));
        }
        source.internal_version = to;
    }
    let mut out = section.metadata_bytes().map_err(id50_error)?;
    out.extend_from_slice(&data[metadata_len..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_sh_payloads::{ClusterShPayloadsSourceKind, ClusterShPayloadsSourceRecord};
    use crate::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
    use crate::sh_body::SH_BODY_SECTIONS;

    /// A section with the field at its current version and flag, then a body.
    fn flagged(section: SectionId, body: ShBodyPresence) -> Vec<u8> {
        let layout = flag_layout(section).unwrap();
        let mut bytes: Vec<u8> = (0..layout.header_len as u8).map(|b| b ^ 0xA5).collect();
        bytes[..layout.field_width]
            .copy_from_slice(&layout.expected_version.to_le_bytes()[..layout.field_width]);
        let flag_at = layout.header_len - layout.field_width;
        bytes[flag_at..layout.header_len]
            .copy_from_slice(&u32::from(body.flag()).to_le_bytes()[..layout.field_width]);
        bytes.extend_from_slice(&[7, 8, 9, 10, 11]);
        bytes
    }

    fn prl(sections: &[(u32, u16, Vec<u8>)]) -> Vec<u8> {
        let blobs: Vec<_> = sections
            .iter()
            .map(|(id, version, data)| SectionBlob {
                section_id: *id,
                version: *version,
                data: data.clone(),
            })
            .collect();
        let mut out = Vec::new();
        write_prl(&mut out, &blobs).unwrap();
        out
    }

    fn sections(prl: &[u8]) -> Vec<(u32, u16, Vec<u8>)> {
        let meta = read_container(&mut Cursor::new(prl)).unwrap();
        meta.sections
            .iter()
            .map(|entry| {
                let data = section_data_from_bytes(prl, &meta, entry.section_id)
                    .unwrap()
                    .unwrap();
                (entry.section_id, entry.version, data.to_vec())
            })
            .collect()
    }

    fn id50(sources: &[u32]) -> Vec<u8> {
        let mut section = ClusterShPayloadsSection {
            header: crate::cluster_sh_payloads::ClusterShPayloadsHeader {
                cluster_count: 0,
                source_count: sources.len() as u32,
                grid_dimensions: [1, 1, 1],
                affinity_dimensions: [1, 1, 1],
                payload_bytes: 0,
            },
            sources: Vec::new(),
            index: Vec::new(),
        };
        for &id in sources {
            let layout = flag_layout(SectionId::from_u32(id).unwrap()).unwrap();
            let kind = if id == SectionId::OctahedralShVolume as u32
                || id == SectionId::DirectShVolume as u32
            {
                ClusterShPayloadsSourceKind::DenseBaseAtlas
            } else {
                ClusterShPayloadsSourceKind::SparseAffinity
            };
            section.sources.push(ClusterShPayloadsSourceRecord {
                section_id: id,
                internal_version: layout.expected_version,
                kind,
            });
        }
        section.metadata_bytes().unwrap()
    }

    fn current_prl() -> Vec<u8> {
        let mut table = vec![(1u32, 3u16, vec![1, 2, 3])];
        for &section in &SH_BODY_SECTIONS {
            table.push((section as u32, 1, flagged(section, ShBodyPresence::Present)));
        }
        let mut ids: Vec<u32> = SH_BODY_SECTIONS.iter().map(|&id| id as u32).collect();
        ids.sort_unstable();
        table.push((SectionId::ClusterShPayloads as u32, 1, id50(&ids)));
        prl(&table)
    }

    #[test]
    fn downgrade_drops_each_flag_and_upgrade_restores_the_prl_byte_for_byte() {
        let current = current_prl();
        let pre_flag = downgrade_to_pre_flag_prl(&current).unwrap();
        let (now, before) = (sections(&current), sections(&pre_flag));
        assert_eq!(now.len(), before.len());
        for ((id, version, data), (old_id, old_version, old)) in now.iter().zip(&before) {
            assert_eq!(
                (id, version),
                (old_id, old_version),
                "table order and versions"
            );
            let Some(layout) = SectionId::from_u32(*id).and_then(flag_layout) else {
                continue;
            };
            let width = layout.field_width;
            assert_eq!(old.len() + width, data.len(), "section {id} loses its flag");
            assert_eq!(
                read_field(old, 0, width),
                layout.expected_version - 1,
                "section {id} at its pre-flag version"
            );
            assert_eq!(
                &old[width..layout.header_len - width],
                &data[width..layout.header_len - width]
            );
            assert_eq!(
                &old[layout.header_len - width..],
                &data[layout.header_len..]
            );
        }
        let id50 = &before
            .iter()
            .find(|(id, ..)| *id == SectionId::ClusterShPayloads as u32)
            .unwrap()
            .2;
        let parsed =
            ClusterShPayloadsSection::from_metadata_bytes(id50, id50.len() as u64).unwrap();
        for source in parsed.sources {
            let layout = flag_layout(SectionId::from_u32(source.section_id).unwrap()).unwrap();
            assert_eq!(source.internal_version, layout.expected_version - 1);
        }
        assert_eq!(upgrade_pre_flag_prl(&pre_flag).unwrap(), current);
    }

    #[test]
    fn upgrade_refuses_a_section_already_at_the_current_version() {
        let err = upgrade_pre_flag_prl(&current_prl()).unwrap_err();
        assert!(err.to_string().contains("internal version"), "{err}");
    }

    #[test]
    fn downgrade_refuses_an_omitted_body() {
        let section = SectionId::DirectShDeltaVolumes;
        let current = prl(&[(section as u32, 1, flagged(section, ShBodyPresence::Omitted))]);
        let err = downgrade_to_pre_flag_prl(&current).unwrap_err();
        assert!(err.to_string().contains("bodies only"), "{err}");
    }

    #[test]
    fn both_directions_refuse_a_section_id_listed_twice() {
        let section = SectionId::DirectShDeltaVolumes;
        let current = flagged(section, ShBodyPresence::Present);
        let twice = prl(&[
            (section as u32, 1, current.clone()),
            (section as u32, 1, current),
        ]);
        let err = downgrade_to_pre_flag_prl(&twice).unwrap_err();
        assert!(err.to_string().contains("listed twice"), "{err}");
        let pre_flag = prl(&[(1, 3, vec![1, 2, 3]), (1, 3, vec![4, 5, 6])]);
        let err = upgrade_pre_flag_prl(&pre_flag).unwrap_err();
        assert!(err.to_string().contains("listed twice"), "{err}");
    }

    #[test]
    fn both_directions_refuse_id_57_streamed_through_id_50() {
        let mut ids: Vec<u32> = SH_BODY_SECTIONS.iter().map(|&id| id as u32).collect();
        ids.sort_unstable();
        let mut bytes = id50(&ids);
        let mut section =
            ClusterShPayloadsSection::from_metadata_bytes(&bytes, bytes.len() as u64).unwrap();
        section.sources.push(ClusterShPayloadsSourceRecord {
            section_id: SectionId::MoverProbeChannel as u32,
            internal_version: u32::from(crate::mover_probe_channel::MOVER_PROBE_CHANNEL_EPOCH),
            kind: ClusterShPayloadsSourceKind::MoverChannel,
        });
        section.header.source_count += 1;
        bytes = section.metadata_bytes().unwrap();
        let current = prl(&[(SectionId::ClusterShPayloads as u32, 1, bytes)]);
        let err = downgrade_to_pre_flag_prl(&current).unwrap_err();
        assert!(
            err.to_string()
                .contains("streams id 57 through id 50; the epoch relabel cannot follow it"),
            "{err}"
        );
        // An upgrade refuses before it reads the five sources' versions.
        let err = upgrade_pre_flag_prl(&current).unwrap_err();
        assert!(
            err.to_string().contains(
                "baseline streams id 57 through id 50; the epoch relabel cannot follow it"
            ),
            "{err}"
        );
    }

    #[test]
    fn upgrade_decodes_a_real_section_whole() {
        let section = DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: crate::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: crate::octahedral::DEFAULT_IRRADIANCE_TILE_BORDER,
            valid_probe_masks: vec![0],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };
        let current = prl(&[(
            SectionId::DirectShDeltaVolumes as u32,
            1,
            section.to_bytes(),
        )]);
        let upgraded = upgrade_pre_flag_prl(&downgrade_to_pre_flag_prl(&current).unwrap()).unwrap();
        let (_, _, data) = &sections(&upgraded)[0];
        assert_eq!(
            DirectShDeltaVolumesSection::from_bytes(data).unwrap(),
            section
        );
    }
}
