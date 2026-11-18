//! On-demand check of the canonical cell partition against a real PRL.
//! See: context/lib/build_pipeline.md §PRL section IDs

use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

use crate::bvh::BvhSection;
use crate::cells::CellsSection;
use crate::container::{read_container, read_section_data};
use crate::portals::PortalsSection;
use crate::section_registry::SectionId;

use super::{ClusterDirectorySection, canonical_cell_partition};

fn section(
    reader: &mut BufReader<File>,
    meta: &crate::container::ContainerMeta,
    id: SectionId,
) -> Vec<u8> {
    read_section_data(reader, meta, id as u32)
        .expect("section read")
        .unwrap_or_else(|| panic!("PRL has no section {id:?}"))
}

/// Rebuilds the partition from a baked PRL's cells, portals and BVH, times it,
/// and requires it to equal the compiler's id-49 clusters and members. Set
/// `POSTRETRO_PARTITION_PRL` to the PRL path.
#[test]
#[ignore = "needs POSTRETRO_PARTITION_PRL; run on demand"]
fn canonical_partition_matches_a_baked_directory() {
    let path = std::env::var("POSTRETRO_PARTITION_PRL").expect("set POSTRETRO_PARTITION_PRL");
    let mut reader = BufReader::new(File::open(&path).expect("open PRL"));
    let meta = read_container(&mut reader).expect("container");
    let cells = CellsSection::from_bytes(&section(&mut reader, &meta, SectionId::Cells)).unwrap();
    let portals =
        PortalsSection::from_bytes(&section(&mut reader, &meta, SectionId::Portals)).unwrap();
    let bvh = BvhSection::from_bytes(&section(&mut reader, &meta, SectionId::Bvh)).unwrap();
    let directory = ClusterDirectorySection::from_bytes(&section(
        &mut reader,
        &meta,
        SectionId::ClusterDirectory,
    ))
    .unwrap();

    let start = Instant::now();
    let canonical = canonical_cell_partition(
        &cells,
        &portals,
        &bvh,
        directory.primitive_limit,
        directory.cell_limit,
        &directory.seam_portal_ids,
    )
    .unwrap();
    let elapsed = start.elapsed();
    eprintln!(
        "partition: {} cells, {} portals, {} seam portals, {} clusters in {:.3} s",
        cells.cells.len(),
        portals.portals.len(),
        directory.seam_portal_ids.len(),
        canonical.clusters.len(),
        elapsed.as_secs_f64()
    );

    assert_eq!(canonical.members, directory.members);
    assert_eq!(canonical.clusters.len(), directory.clusters.len());
    for (index, (expected, baked)) in canonical
        .clusters
        .iter()
        .zip(&directory.clusters)
        .enumerate()
    {
        let mut baked = baked.clone();
        baked.range_start = 0;
        baked.range_count = 0;
        assert_eq!(*expected, baked, "cluster {index}");
    }
}
