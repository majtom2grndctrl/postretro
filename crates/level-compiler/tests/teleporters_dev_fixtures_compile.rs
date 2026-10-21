//! The shipped `content/dev` teleporter fixtures — the acceptance level, its
//! Luau twin, the eight-source stress map and both matched baselines — compile
//! through the real `prl-build`, data scripts included; the level's baked id 51
//! holds a classic arrival its baseline cannot reach.
//! See: context/lib/build_pipeline.md §Compiler pipeline, §PRL section IDs

use std::path::{Path, PathBuf};
use std::process::Command;

use glam::Vec3;
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::teleporters::{TeleportMode, TeleportersSection};
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_level_loader::LevelWorld;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` into `out` and return its id-55 section,
/// `None` when the PRL carries none.
fn compile(map: &str, out: &Path) -> Option<TeleportersSection> {
    let prl = out.join(format!("{map}.prl"));
    compile_prl(map, &prl);
    let mut cursor = std::io::Cursor::new(std::fs::read(&prl).expect("read PRL"));
    let metadata = read_container(&mut cursor).expect("container");
    let has_script = read_section_data(&mut cursor, &metadata, SectionId::DataScript as u32)
        .expect("read data script")
        .is_some();
    assert!(has_script, "`{map}` embeds its data script");
    read_section_data(&mut cursor, &metadata, SectionId::Teleporters as u32)
        .expect("read teleporters")
        .map(|payload| TeleportersSection::from_bytes(&payload).expect("teleporters decode"))
}

fn compile_prl(map: &str, prl: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .arg(workspace().join(format!("content/dev/maps/{map}.map")))
        .arg("-o")
        .arg(prl)
        .arg("--no-tui")
        .arg("--no-cache")
        .arg("--baked-root")
        .arg(prl.parent().expect("PRL has a directory").join("baked"))
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "`{map}` failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// The PRL's runtime cell locator and its baked id 51.
fn residency(prl: &Path) -> (LevelWorld, CellResidencySetSection) {
    let mut cursor = std::io::Cursor::new(std::fs::read(prl).expect("read PRL"));
    let metadata = read_container(&mut cursor).expect("container");
    let mut section = |id: SectionId| {
        read_section_data(&mut cursor, &metadata, id as u32)
            .expect("read section")
            .unwrap_or_else(|| panic!("{id:?} present"))
    };
    let cells = CellsSection::from_bytes(&section(SectionId::Cells)).expect("cells decode");
    let portals = PortalsSection::from_bytes(&section(SectionId::Portals)).expect("portals decode");
    let locator =
        CellLocatorSection::from_bytes(&section(SectionId::CellLocator), cells.cells.len() as u32)
            .expect("locator decodes");
    let set = CellResidencySetSection::from_bytes(
        &section(SectionId::CellResidencySet),
        cells.cells.len(),
    )
    .expect("residency set decodes");
    let world = LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
        .expect("visibility world");
    (world, set)
}

/// Quake map units to engine metres, as the compiler converts them.
fn engine(quake: [f32; 3]) -> Vec3 {
    Vec3::new(-quake[1], quake[2], -quake[0]) * 0.0254
}

/// The gate's classic arrival cell, baked at lead 0 for the camera cell the
/// gate pad stands in, and its lead without teleporters in the baseline.
fn teleport_gate_arrival_lead(prl: &Path) -> Option<u32> {
    let (world, set) = residency(prl);
    // Gate pad centre and the `vault_in` destination, from teleporters.map.
    let source = world.locate_cell(engine([-40.0, -184.0, 48.0]));
    let arrival = world.locate_cell(engine([3920.0, 112.0, 40.0])) as u32;
    assert_ne!(source as u32, arrival, "the vault is a separate room");
    set.entries_for(source)
        .iter()
        .find(|entry| entry.cell_id == arrival)
        .map(|entry| entry.lead)
}

#[test]
fn teleport_dev_fixtures_compile_with_prl_build() {
    let out = tempfile::tempdir().expect("temp output directory");
    let level = compile("teleporters", out.path()).expect("the level has teleporters");
    let modes: Vec<(TeleportMode, bool)> = level
        .teleporters
        .iter()
        .map(|record| (record.mode, record.enabled_on_spawn))
        .collect();
    assert_eq!(
        modes,
        [
            (TeleportMode::Classic, false),
            (TeleportMode::Seamless, false),
            (TeleportMode::Seamless, false),
            (TeleportMode::Classic, true),
            (TeleportMode::Classic, true),
        ],
        "gate, both era directions, both return gates, in map order"
    );
    assert_eq!(level.teleporters[0].arrival_sound, "fixtures/jump");
    assert!(
        level
            .teleporters
            .iter()
            .all(|record| record.on_teleport.is_empty()),
        "the one-way gate disarms through its script binding, not a brush KVP"
    );

    // The gate pad's camera cell holds the far vault's arrival cell from lead
    // 0, which the matched baseline (same static geometry, no teleporters)
    // never bakes for that camera.
    assert_eq!(
        teleport_gate_arrival_lead(&out.path().join("teleporters.prl")),
        Some(0)
    );

    let twin = compile("teleporters-luau", out.path()).expect("the twin has teleporters");
    assert_eq!(
        level.to_bytes(),
        twin.to_bytes(),
        "the Luau twin compiles a byte-identical teleporter section"
    );

    let stress = compile("teleporters-stress", out.path()).expect("the stress map has teleporters");
    assert_eq!(stress.teleporters.len(), 8);
    for baseline in ["teleporters-baseline", "teleporters-stress-baseline"] {
        assert!(
            compile(baseline, out.path()).is_none(),
            "`{baseline}` removes every teleporter"
        );
    }
    assert_eq!(
        teleport_gate_arrival_lead(&out.path().join("teleporters-baseline.prl")),
        None,
        "without the gate the vault is out of reach and view of the hub"
    );
}
