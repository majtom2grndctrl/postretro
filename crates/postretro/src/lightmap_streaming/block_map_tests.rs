//! Block facts over a real manifest: a block reads and charges only the id-42
//! planes it stores. See: context/lib/testing_guide.md

use std::sync::Arc;

use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, group_plane_len};
use postretro_level_format::{SectionBlob, SectionId};

use super::*;
use crate::lightmap_streaming::prl_test_fixture::{FIXTURE_CELLS, StreamedLightmapPrl};
use crate::lightmap_streaming::source::ManifestBlockSource;

/// Block 0 stores both planes, 1 only plane B, 2 only plane A, and the rest
/// neither.
fn with_mixed_planes(sections: &mut Vec<SectionBlob>) {
    let lightmap = sections
        .iter()
        .find(|blob| blob.section_id == SectionId::Lightmap as u32)
        .unwrap();
    let index = postretro_level_format::lightmap::LightmapBlockIndex::from_prefix(
        &lightmap.data,
        lightmap.data.len() as u64,
    )
    .unwrap();
    let blocks = index
        .records
        .iter()
        .enumerate()
        .map(|(block, record)| {
            let len = group_plane_len(u32::from(record.width), u32::from(record.height)).unwrap();
            let plane = |keep: bool, byte: u8| match keep {
                true => vec![byte; len as usize],
                false => Vec::new(),
            };
            [
                plane(block == 0 || block == 2, 7),
                plane(block == 0 || block == 1, 9),
            ]
        })
        .collect();
    let section = ShadowmaskAtlasSection {
        owners: Vec::new(),
        blocks,
    };
    sections.push(SectionBlob {
        section_id: SectionId::ShadowmaskAtlas as u32,
        version: 1,
        data: section.to_bytes(),
    });
}

#[test]
fn a_block_charges_and_reads_only_its_stored_planes() {
    let prl = StreamedLightmapPrl::write_edited(with_mixed_planes);
    let world = prl.load();
    let manifest = Arc::clone(world.lightmap_stream_manifest().unwrap());
    let source = ManifestBlockSource::new(Arc::clone(&manifest));
    let map = LevelBlockMap::build(&source, FIXTURE_CELLS as usize, None).unwrap();
    let plane_of = |block: u32| {
        let record = manifest.lightmap_index().records[block as usize];
        group_plane_len(u32::from(record.width), u32::from(record.height)).unwrap()
    };

    for (block, stored_planes, reads) in [(0, 2, 2), (1, 1, 2), (2, 1, 2), (3, 0, 1)] {
        let facts = map.facts(block);
        assert_eq!(
            facts.shadowmask_bytes,
            stored_planes * plane_of(block),
            "block {block}"
        );
        assert_eq!(facts.ranges.len(), reads, "block {block}");
        assert_eq!(
            facts.texels.mask,
            stored_planes * facts.texels.lightmap,
            "block {block}: a slot per stored plane"
        );
    }

    let planes = |block| {
        source
            .read_block_pair(block)
            .unwrap()
            .shadowmask
            .expect("the level keeps id 42")
    };
    let filled = |block: u32, byte| vec![byte; plane_of(block) as usize];
    assert_eq!(planes(0), [filled(0, 7), filled(0, 9)]);
    assert_eq!(planes(1), [Vec::new(), filled(1, 9)]);
    assert_eq!(planes(2), [filled(2, 7), Vec::new()]);
    assert_eq!(planes(3), [Vec::<u8>::new(), Vec::new()]);
}
