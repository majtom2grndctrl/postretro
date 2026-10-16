//! The shipped `content/dev` teleporter fixtures — the acceptance level, its
//! Luau twin, the eight-source stress map and both matched baselines — compile
//! through the real `prl-build`, data scripts included.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use std::path::{Path, PathBuf};
use std::process::Command;

use postretro_level_format::teleporters::{TeleportMode, TeleportersSection};
use postretro_level_format::{SectionId, read_container, read_section_data};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` into `out` and return its id-55 section,
/// `None` when the PRL carries none.
fn compile(map: &str, out: &Path) -> Option<TeleportersSection> {
    let prl = out.join(format!("{map}.prl"));
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .arg(workspace().join(format!("content/dev/maps/{map}.map")))
        .arg("-o")
        .arg(&prl)
        .arg("--no-tui")
        .arg("--no-cache")
        .arg("--baked-root")
        .arg(out.join("baked"))
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "`{map}` failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
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
}
