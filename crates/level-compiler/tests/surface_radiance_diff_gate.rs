//! The surface radiance cache is bake-only: its branch changes no PRL
//! format, runtime crate or shader, and adds no section id. A cheap pin of
//! the `SectionId` registry runs every time; the diff against the branch's
//! merge base with `origin/main` shells out to git, so it runs on demand.
//! Both are the surface-radiance-cache plan's AC 29 and retire with it.
//! See: context/lib/build_pipeline.md §PRL section IDs

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/level-compiler sits two below the workspace")
        .to_path_buf()
}

/// Every `Name = id` the `SectionId` enum declares, in source order.
fn section_ids() -> Vec<(String, u32)> {
    let source = std::fs::read_to_string(
        workspace_root().join("crates/level-format/src/section_registry.rs"),
    )
    .expect("read the section registry");
    let start = source
        .find("pub enum SectionId {")
        .expect("SectionId is declared");
    let body = &source[start..start + source[start..].find("\n}").expect("SectionId closes")];
    body.lines()
        .map(|line| line.split("//").next().unwrap_or("").trim())
        .filter_map(|line| {
            let (name, id) = line.trim_end_matches(',').split_once(" = ")?;
            Some((name.to_owned(), id.parse().ok()?))
        })
        .collect()
}

/// The registry at the plan's base commit, d2bda6032.
const BASE_SECTION_IDS: [(&str, u32); 44] = [
    ("BspNodes", 12),
    ("BspLeaves", 13),
    ("Portals", 15),
    ("TextureNames", 16),
    ("Geometry", 17),
    ("AlphaLights", 18),
    ("Bvh", 19),
    ("ShVolume", 20),
    ("LightInfluence", 21),
    ("Lightmap", 22),
    ("ChunkLightList", 23),
    ("AnimatedLightChunks", 24),
    ("AnimatedLightWeightMaps", 25),
    ("LightTags", 26),
    ("DeltaShVolumes", 27),
    ("DataScript", 28),
    ("MapEntity", 29),
    ("FogVolumes", 30),
    ("FogCellMasks", 31),
    ("TextureCacheKeys", 32),
    ("SdfAtlas", 33),
    ("OctahedralShVolume", 34),
    ("DirectShVolume", 35),
    ("NavMesh", 36),
    ("CellDrawIndex", 37),
    ("Cells", 38),
    ("CellLocator", 39),
    ("EntityShadowLights", 40),
    ("DirectShDeltaVolumes", 41),
    ("ShadowmaskAtlas", 42),
    ("KinematicGeometry", 43),
    ("TriggerVolumes", 44),
    ("AnimatedDirectShDeltaVolumes", 45),
    ("CellVisibility", 46),
    ("BillboardDirectScatterVolume", 47),
    ("AnimatedBillboardDirectScatterDeltaVolumes", 48),
    ("ClusterDirectory", 49),
    ("ClusterShPayloads", 50),
    ("CellResidencySet", 51),
    ("EnvironmentVolumes", 52),
    ("PushForces", 53),
    ("ReflectionProbes", 54),
    ("Teleporters", 55),
    ("GhostLinks", 56),
];

// AC 29: the branch adds no `SectionId`: the registry still declares
// exactly the ids it declared at the base commit, by name and number.
#[test]
fn the_section_registry_lists_exactly_the_base_commits_ids() {
    let ids = section_ids();
    let expected: Vec<(String, u32)> = BASE_SECTION_IDS
        .iter()
        .map(|&(name, id)| (name.to_owned(), id))
        .collect();
    assert_eq!(ids, expected);
}

fn git(args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git prints UTF-8")
}

/// Workspace members that are build or test tooling, not runtime: the
/// compiler itself, the script compiler, the content tool, xtask and the
/// test log capture.
const TOOLING: [&str; 5] = [
    "crates/level-compiler",
    "crates/script-compiler",
    "crates/tool",
    "crates/xtask",
    "crates/test-log-capture",
];

/// Every workspace member directory the root manifest lists.
fn workspace_members() -> Vec<String> {
    let manifest = std::fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("read the workspace manifest");
    let start = manifest.find("members = [").expect("workspace members");
    let end = start + manifest[start..].find(']').expect("members close");
    manifest[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

// AC 29: against its merge base with `origin/main`, the branch (committed,
// staged, unstaged and untracked) touches no file under
// `crates/level-format`, any runtime crate (every workspace member but the
// tooling) or any shader. Shells out to git and needs `origin/main` fetched.
#[test]
#[ignore = "diffs the working tree against git merge-base HEAD origin/main; run on demand with -- --ignored"]
fn the_branch_touches_no_format_runtime_crate_or_shader() {
    let base = git(&["merge-base", "HEAD", "origin/main"]);
    let base = base.trim();
    let mut changed: Vec<String> = git(&["diff", "--name-only", base])
        .lines()
        .chain(git(&["ls-files", "--others", "--exclude-standard"]).lines())
        .map(str::to_owned)
        .collect();
    changed.sort();
    changed.dedup();
    assert!(!changed.is_empty(), "the branch changes something");

    let members = workspace_members();
    assert!(members.contains(&"crates/level-format".to_owned()));
    for tool in TOOLING {
        assert!(members.contains(&tool.to_owned()), "{tool} is a member");
    }
    let runtime: Vec<&String> = members
        .iter()
        .filter(|member| !TOOLING.contains(&member.as_str()))
        .collect();
    let forbidden: Vec<&String> = changed
        .iter()
        .filter(|path| {
            runtime
                .iter()
                .any(|member| path.starts_with(&format!("{member}/")))
                || path.ends_with(".wgsl")
                || path.contains("/shaders/")
        })
        .collect();
    assert!(
        forbidden.is_empty(),
        "the branch (base {base}) touches format, runtime or shader files: {forbidden:#?}"
    );
}
