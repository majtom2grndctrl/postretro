//! Id 57's payload placement, through the binary: each block whose
//! payload is over its resident bar streams with id 50's cells, one at or under
//! it stays in id 57, and a streamed build reads back the resident build's
//! payload, from id 50's clusters while SH streams and whole under `off`.
//!
//! Streaming is residency: a streamed payload is the same data placed with id
//! 50's cells. `--mover-payload-resident-bar` lowers both bars so a small map
//! crosses them.
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod mover_bounce_support;
mod prl_build_support;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use postretro_level_format::SectionId;
use postretro_level_format::mover_probe_channel::{
    MoverChannelBlock, MoverPayloadPlacement, MoverProbeChannelSection, parse_streamed_rows,
};
use postretro_level_loader::{LevelWorld, ShStorage, load_prl};

use mover_bounce_support::{Built, Cache, build, channel, dev_map};

const BAR_FLAG: &str = "--mover-payload-resident-bar";

/// The two PRLs the `off` probe compares, resident first, in one variable.
const OFF_PROBE_ENV: &str = "POSTRETRO_TEST_MOVER_PLACEMENT_PRLS";
const OFF_PROBE: &str = "a_streamed_build_loaded_under_off_holds_the_resident_payload";

/// The setpiece: both of its blocks carry a payload, of different sizes.
fn setpiece(flags: &[&str]) -> Built {
    build(
        &dev_map("mover-setpiece.map"),
        "mover-setpiece",
        Cache::Cold,
        flags,
    )
}

/// `section` with both blocks marked resident: the payload it carries,
/// whatever its placement said.
fn as_resident(section: &MoverProbeChannelSection) -> MoverProbeChannelSection {
    let mut section = section.clone();
    section.direct_placement = MoverPayloadPlacement::Resident;
    section.bounce_placement = MoverPayloadPlacement::Resident;
    section
}

fn load(prl: &Path) -> LevelWorld {
    load_prl(prl.to_str().expect("UTF-8 path")).expect("the PRL loads")
}

/// Every listed cell's payload of `loaded` equals `resident`'s, each streamed
/// block resolved from the id-50 row of any cluster carrying the cell; every
/// cluster carrying it carries the same row.
fn assert_payload_resolves_from_id_50(world: &LevelWorld, resident: &MoverProbeChannelSection) {
    let loaded = world.mover_probe_channel().expect("id 57 loads");
    let ShStorage::Streaming(manifest) = world.sh_storage() else {
        panic!("the setpiece streams SH under the default mode");
    };
    assert!(manifest.carries_source(SectionId::MoverProbeChannel));
    let chunks: Vec<_> = (0..manifest.cluster_count())
        .map(|cluster| {
            manifest
                .read_and_decode_cluster(cluster)
                .expect("decode the cluster")
        })
        .collect();
    let mut rows = BTreeMap::new();
    // A cluster covering no listed cell with streamed bytes carries no block.
    for block in chunks.iter().filter_map(|chunk| chunk.mover_channel_rows()) {
        for row in parse_streamed_rows(block).expect("the row block parses") {
            if let Some(seen) = rows.insert(row.cell, row) {
                assert_eq!(
                    (seen.direct, seen.bounce),
                    (row.direct, row.bounce),
                    "cell {}: every cluster carrying it carries the same row",
                    row.cell
                );
            }
        }
    }
    let slices = loaded.cell_slices();
    assert_eq!(slices, resident.cell_slices(), "the same payload slices");
    for (position, cell) in loaded.listed_cells().enumerate() {
        let row = rows.get(&(cell as u32)).copied();
        let payload = loaded
            .cell_payload(position, &slices[position], row)
            .unwrap_or_else(|error| panic!("cell {cell}: {error}"));
        let expected = resident
            .cell_payload(position, &slices[position], None)
            .expect("the resident cell");
        assert_eq!(payload.visibility, expected.visibility, "cell {cell}");
        assert_eq!(payload.own_tiles, expected.own_tiles, "cell {cell}");
        assert_eq!(payload.bounce_tiles, expected.bounce_tiles, "cell {cell}");
    }
}

/// Run [`OFF_PROBE`] in a child of this test binary with SH streaming `off`,
/// which the loader reads from the environment only.
fn assert_off_load_holds_the_resident_payload(resident: &Path, streamed: &Path) {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([OFF_PROBE, "--exact", "--ignored", "--nocapture"])
        .env("POSTRETRO_SH_STREAMING", "off")
        .env(
            OFF_PROBE_ENV,
            format!("{}\n{}", resident.display(), streamed.display()),
        )
        .output()
        .expect("spawn the off probe");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && logs.contains("1 passed"),
        "the off probe passes\n{logs}"
    );
}

// Under the shipped bars the setpiece
// keeps both payloads in id 57 and id 50 lists no id-57 source. With the bar
// at the smaller payload, the larger block streams and the smaller one,
// exactly at the bar, stays resident; at zero both stream. Each streamed build
// keeps id 57's cells and records, and reads back the resident build's payload
// from id 50 while SH streams and whole under `off`.
#[test]
fn a_block_over_its_bar_streams_with_id_50_and_one_at_or_under_it_stays_resident() {
    let resident_build = setpiece(&[]);
    let resident = channel(&resident_build.prl).expect("the setpiece packs id 57");
    for block in MoverChannelBlock::BOTH {
        assert_eq!(resident.placement(block), MoverPayloadPlacement::Resident);
        assert!(resident.payload_held(block));
    }
    let world = load(&resident_build.prl);
    let ShStorage::Streaming(manifest) = world.sh_storage() else {
        panic!("the setpiece streams SH under the default mode");
    };
    assert!(
        !manifest.carries_source(SectionId::MoverProbeChannel),
        "a channel under both bars adds no id-50 source"
    );
    assert_eq!(world.mover_probe_channel(), Some(&resident));

    let direct = resident.payload_bytes(MoverChannelBlock::Direct);
    let bounce = resident.payload_bytes(MoverChannelBlock::Bounce);
    assert!(
        direct > 0 && bounce > 0 && direct != bounce,
        "the setpiece's payloads differ in size: direct {direct} B, bounce {bounce} B"
    );
    let (smaller, larger) = if direct < bounce {
        (MoverChannelBlock::Direct, MoverChannelBlock::Bounce)
    } else {
        (MoverChannelBlock::Bounce, MoverChannelBlock::Direct)
    };
    let at_smaller = direct.min(bounce).to_string();
    for (bar, streams) in [
        (at_smaller.as_str(), vec![larger]),
        ("0", MoverChannelBlock::BOTH.to_vec()),
    ] {
        let streamed_build = setpiece(&[BAR_FLAG, bar]);
        let streamed = channel(&streamed_build.prl).expect("id 57 decodes");
        for block in MoverChannelBlock::BOTH {
            let expected = if streams.contains(&block) {
                MoverPayloadPlacement::Streamed
            } else {
                MoverPayloadPlacement::Resident
            };
            assert_eq!(streamed.placement(block), expected, "bar {bar}: {block}");
            assert_eq!(
                streamed.payload_held(block),
                expected == MoverPayloadPlacement::Resident,
                "bar {bar}: {block}: the decoded section holds only a resident payload"
            );
        }
        assert!(
            streams.contains(&larger) && (bar == "0") == streams.contains(&smaller),
            "the cases cover one and both blocks"
        );
        let summary = format!(
            "{larger} payload {} B streams with id 50's cells",
            resident.payload_bytes(larger)
        );
        assert!(
            streamed_build.logs.contains(&summary),
            "bar {bar}: the log reports `{summary}`\n{}",
            streamed_build.logs
        );

        let world = load(&streamed_build.prl);
        assert_payload_resolves_from_id_50(&world, &resident);
        assert_off_load_holds_the_resident_payload(&resident_build.prl, &streamed_build.prl);
    }
}

// The child half of the test above: under `off`, the streamed PRL's id 57
// holds the resident PRL's payload with its own placements. Run alone it has
// nothing to compare and passes.
#[test]
#[ignore = "run by a_block_over_its_bar_streams_with_id_50_and_one_at_or_under_it_stays_resident"]
fn a_streamed_build_loaded_under_off_holds_the_resident_payload() {
    let Ok(paths) = std::env::var(OFF_PROBE_ENV) else {
        return;
    };
    assert_eq!(
        std::env::var("POSTRETRO_SH_STREAMING").as_deref(),
        Ok("off")
    );
    let (resident, streamed) = paths.split_once('\n').expect("two PRL paths");
    let resident = load(Path::new(resident));
    let streamed = load(Path::new(streamed));
    let resident = resident.mover_probe_channel().expect("id 57 loads");
    let streamed = streamed.mover_probe_channel().expect("id 57 loads");
    assert!(
        MoverChannelBlock::BOTH
            .iter()
            .any(|&block| streamed.placement(block) == MoverPayloadPlacement::Streamed),
        "the streamed PRL streams a block"
    );
    for block in MoverChannelBlock::BOTH {
        assert!(
            streamed.payload_held(block),
            "{block}: held whole under off"
        );
    }
    assert_eq!(&as_resident(streamed), resident);
}
