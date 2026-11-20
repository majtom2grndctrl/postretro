//! The `content/dev` seam pair compiles through `prl-build`, each map's seam targeting the other.
//! See: context/lib/build_pipeline.md §PRL section IDs (Seams, id 61)

mod prl_build_support;

use std::path::{Path, PathBuf};

use postretro_level_format::seams::{SeamActivation, SeamMode, SeamsSection};
use postretro_level_format::{SectionId, read_container, read_section_data};

use prl_build_support::prl_build;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` into `out` and return its id-61
/// section, which a map with a seam always packs.
fn compile(map: &str, out: &Path) -> SeamsSection {
    let prl = out.join(format!("{map}.prl"));
    let output = prl_build()
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
    let payload = read_section_data(&mut cursor, &metadata, SectionId::Seams as u32)
        .expect("read seams")
        .unwrap_or_else(|| panic!("`{map}` packs a Seams section"));
    SeamsSection::from_bytes(&payload).expect("seams decode")
}

#[test]
fn seam_pair_dev_fixtures_compile_with_prl_build() {
    let out = tempfile::tempdir().expect("temp output directory");
    let a = compile("seam-pair-a", out.path());
    let b = compile("seam-pair-b", out.path());
    for (section, target) in [(&a, "seam-pair-b"), (&b, "seam-pair-a")] {
        assert_eq!(section.seams.len(), 1, "one seam per map");
        let seam = &section.seams[0];
        assert_eq!(seam.name, "pair_door", "one shared name");
        assert_eq!(seam.target, target, "each seam targets the other map");
        assert_eq!(seam.activation, SeamActivation::Touch);
        assert_eq!(seam.mode, SeamMode::Classic);
        assert!(seam.enabled_on_spawn);
    }
    assert_eq!(a.seams[0].landmark_yaw, 0.0);
    assert!(
        (b.seams[0].landmark_yaw - 90.0f32.to_radians()).abs() < 1e-6,
        "map B's landmark turns a crossing a quarter turn"
    );
    // The seam stands at the same place relative to its landmark in both
    // maps: B's brush, rotated back a quarter turn about B's landmark, is A's.
    let extent = |section: &SeamsSection| {
        let seam = &section.seams[0];
        let brush = &seam.brushes[0];
        let offset = |axis: usize| {
            (
                brush.aabb_min[axis] - seam.landmark_origin[axis],
                brush.aabb_max[axis] - seam.landmark_origin[axis],
            )
        };
        [offset(0), offset(1), offset(2)]
    };
    let [ax, ay, az] = extent(&a);
    let [bx, by, bz] = extent(&b);
    let close = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4;
    assert!(close(ay, by), "height above the landmark: {ay:?} vs {by:?}");
    assert!(
        close(ax, bz) && close(az, bx),
        "a quarter turn swaps the horizontal extents: A {ax:?} {az:?}, B {bx:?} {bz:?}"
    );
}
