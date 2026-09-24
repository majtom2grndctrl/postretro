use std::fs::File;
use std::path::PathBuf;

use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

#[test]
#[ignore = "manual capture helper; set POSTRETRO_CLUSTER_SOURCE_PRL and POSTRETRO_CLUSTER_STRIPPED_PRL"]
fn rewrites_prl_without_cluster_directory_through_container_apis() {
    let source = PathBuf::from(
        std::env::var_os("POSTRETRO_CLUSTER_SOURCE_PRL")
            .expect("POSTRETRO_CLUSTER_SOURCE_PRL must name the directory-present PRL"),
    );
    let stripped = PathBuf::from(
        std::env::var_os("POSTRETRO_CLUSTER_STRIPPED_PRL")
            .expect("POSTRETRO_CLUSTER_STRIPPED_PRL must name the output PRL"),
    );
    assert_ne!(
        source, stripped,
        "capture helper must not overwrite its directory-present source",
    );

    let mut input = File::open(&source).expect("open directory-present PRL");
    let metadata = read_container(&mut input).expect("read source PRL metadata");
    assert_eq!(
        metadata
            .sections
            .iter()
            .filter(|entry| entry.section_id == SectionId::ClusterDirectory as u32)
            .count(),
        1,
        "manual comparison source must contain exactly one cluster directory",
    );

    let sections: Vec<_> = metadata
        .sections
        .iter()
        .filter(|entry| entry.section_id != SectionId::ClusterDirectory as u32)
        .map(|entry| SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data: read_section_data(&mut input, &metadata, entry.section_id)
                .expect("read source section")
                .expect("section table entry must resolve"),
        })
        .collect();

    let mut output = File::create(&stripped).expect("create directory-removed PRL");
    write_prl(&mut output, &sections).expect("rewrite PRL through the shared container writer");

    let mut rewritten = File::open(&stripped).expect("open directory-removed PRL");
    let rewritten_metadata = read_container(&mut rewritten).expect("read rewritten PRL metadata");
    assert_eq!(
        rewritten_metadata.sections.len() + 1,
        metadata.sections.len()
    );
    assert!(
        rewritten_metadata
            .sections
            .iter()
            .all(|entry| entry.section_id != SectionId::ClusterDirectory as u32),
    );
}
