//! The play drain cap levers: the dev-tools slider and the walk measurement
//! override set one seam on the level's drain step. Play drains admit against
//! it; settling drains and the lightmap in-hand caps never read it. The walk's
//! test-only in-hand lever moves the in-hand caps alone.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/testing_guide.md §3

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use postretro_level_loader::LightmapDrainOutcome;
use postretro_visibility::VisibleCells;

use super::tests::is_test_source;
use super::*;
use crate::lightmap_streaming::controller::{MAX_BAND_IN_HAND_PAIR_BYTES, MAX_IN_HAND_PAIR_BYTES};
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::lightmap_streaming::test_fixtures::*;
use crate::session::lightmap_residency::walk_measurement::levers::{
    parse_drain_cap, parse_in_hand_cap,
};
use crate::streaming::drain_budget::{
    DrainClass, DrainItem, DrainRank, MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
    SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN,
};
use crate::streaming::request::StreamResource;

const PRODUCTION: u64 = MAX_INSTALL_DECODED_BYTES_PER_DRAIN;
const SETTLING: u64 = SETTLING_INSTALL_DECODED_BYTES_PER_DRAIN;

/// One lever's own path to the shared seam, setting the play cap to `bytes`.
struct Lever {
    name: &'static str,
    set: fn(&mut LevelStreaming, u64),
}

/// The walk override through its env parser, the slider's launch seed
/// through its own, and with dev-tools the Streaming tab's slider through
/// its lever.
fn levers() -> Vec<Lever> {
    #[cfg_attr(not(feature = "dev-tools"), allow(unused_mut))]
    let mut levers = vec![
        Lever {
            name: "walk override",
            set: |level, bytes| {
                let cap = parse_drain_cap(&(bytes as f64 / MIB as f64).to_string()).unwrap();
                assert_eq!(cap.bytes.get(), bytes);
                cap.apply(level);
            },
        },
        Lever {
            name: "launch seed",
            set: |level, bytes| {
                level.apply_play_drain_cap_seed(Ok((bytes as f64 / MIB as f64).to_string()));
            },
        },
    ];
    #[cfg(feature = "dev-tools")]
    levers.push(Lever {
        name: "dev-tools slider",
        set: |level, bytes| {
            let mut lever = level.drain_cap_lever();
            assert_eq!(lever.production_bytes, PRODUCTION);
            lever.set_cap_mib(bytes as f64 / MIB as f64);
            level.apply_drain_cap_lever(&lever);
        },
    });
    levers
}

/// A fresh level owner with `lever` at `bytes`, checked at the seam.
fn level_at(lever: &Lever, bytes: u64) -> LevelStreaming {
    let mut level = LevelStreaming::default();
    (lever.set)(&mut level, bytes);
    assert_eq!(
        level.play_drain_cap_override(),
        NonZeroU64::new(bytes),
        "{}",
        lever.name
    );
    level
}

fn sh(cluster: u32, bytes: u64) -> DrainItem {
    DrainItem {
        rank: DrainRank::sh(DrainClass::Visible, 0, cluster),
        bytes,
    }
}

fn pair(block: u32, bytes: u64) -> DrainItem {
    DrainItem::pair(
        DrainRank::lightmap_block(DrainClass::Visible, 0, 0, block),
        bytes / 2,
        bytes - bytes / 2,
    )
    .unwrap()
}

/// Offers `items` to the level's one drain and admits them through the step
/// `prepare_drains` runs; returns the items and bytes admitted, both
/// resources together.
fn admit(level: &mut LevelStreaming, settling: bool, items: &[DrainItem]) -> (usize, u64) {
    level.drain.begin();
    for &item in items {
        level.drain.offer(item);
    }
    level.admit_offered(settling).unwrap();
    let sh = level.drain.admission(StreamResource::Sh);
    let lightmap = level.drain.admission(StreamResource::LightmapBlock);
    (sh.admitted + lightmap.admitted, sh.bytes + lightmap.bytes)
}

/// `count` 2 MiB items, SH and lightmap alternating.
fn two_mib_items(count: u32) -> Vec<DrainItem> {
    (0..count)
        .map(|index| {
            if index % 2 == 0 {
                sh(index, 2 * MIB)
            } else {
                pair(index, 2 * MIB)
            }
        })
        .collect()
}

// With every lever unset, play and settling drains admit against today's
// budgets.
#[test]
fn unset_levers_admit_against_the_production_budgets() {
    let mut level = LevelStreaming::default();
    assert_eq!(level.play_drain_cap_override(), None);
    let play = two_mib_items(5);
    assert_eq!(admit(&mut level, false, &play), (4, PRODUCTION));
    let settling = two_mib_items(17);
    assert_eq!(admit(&mut level, true, &settling), (16, SETTLING));
}

// From either lever, a play drain admits against the lever-set cap. At
// half production, a drain production admits whole stops at the lever cap;
// at twice production, a drain production would cut admits whole.
#[test]
fn a_play_drain_admits_against_the_lever_set_cap_from_either_lever() {
    let fits_production = [sh(0, 3 * MIB), pair(0, 3 * MIB), sh(1, 3 * MIB / 2)];
    let over_production = [sh(0, 5 * MIB), pair(0, 5 * MIB), sh(1, 5 * MIB)];
    let mut unset = LevelStreaming::default();
    assert_eq!(
        admit(&mut unset, false, &fits_production),
        (3, 15 * MIB / 2)
    );
    assert_eq!(admit(&mut unset, false, &over_production), (1, 5 * MIB));

    for lever in levers() {
        let mut half = level_at(&lever, PRODUCTION / 2);
        let (admitted, bytes) = admit(&mut half, false, &fits_production);
        assert_eq!((admitted, bytes), (1, 3 * MIB), "{}", lever.name);
        assert!(bytes <= PRODUCTION / 2, "{}", lever.name);

        let mut twice = level_at(&lever, 2 * PRODUCTION);
        assert_eq!(
            admit(&mut twice, false, &over_production),
            (3, 15 * MIB),
            "{}",
            lever.name
        );
    }
}

// At a lever-set cap, a drain whose first item exceeds the cap admits
// that item and nothing after it, though the rest would fit.
#[test]
fn a_first_item_over_the_lever_cap_admits_alone() {
    for lever in levers() {
        for cap in [PRODUCTION / 2, 2 * PRODUCTION] {
            let mut level = level_at(&lever, cap);
            let items = [sh(0, cap + MIB), pair(0, 1024), sh(1, 1024)];
            assert_eq!(
                admit(&mut level, false, &items),
                (1, cap + MIB),
                "{} at {cap}",
                lever.name
            );
        }
    }
}

// With either lever set, a settling drain still admits against
// the settling budget, while a play drain of the same items reads the lever.
#[test]
fn settling_drains_admit_against_the_settling_budget_with_either_lever_set() {
    let items = two_mib_items(17);
    for lever in levers() {
        for cap in [PRODUCTION / 2, 2 * PRODUCTION] {
            let mut level = level_at(&lever, cap);
            assert_eq!(
                admit(&mut level, true, &items),
                (16, SETTLING),
                "{} at {cap}",
                lever.name
            );
            assert_eq!(
                admit(&mut level, false, &items),
                ((cap / (2 * MIB)) as usize, cap),
                "{} at {cap}",
                lever.name
            );
        }
    }
}

/// A lightmap-only level owner over the corridor, every block a pair of
/// `half` bytes per half, at lead `lead_metres`; `cap` set through `lever`.
fn corridor_level(
    half: u64,
    lead_metres: f32,
    cap: Option<(&Lever, u64)>,
) -> (LevelStreaming, Arc<TestBlockSource>) {
    let source = TestBlockSource::new(corridor_blocks(half, true));
    let mut level = match cap {
        Some((lever, bytes)) => level_at(lever, bytes),
        None => LevelStreaming::default(),
    };
    let mut stage = CellDemand::new(corridor_set().max_lead);
    stage.set_lead_metres(lead_metres);
    level.install_cell_demand(stage);
    level.install_lightmap(
        LightmapStreamingSession::with_source(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            &corridor_set(),
            None,
        )
        .unwrap(),
    );
    (level, source)
}

/// One play frame at camera cell 0, drawing nothing.
fn play_frame_at_cell_0(level: &mut LevelStreaming) {
    let set = corridor_set();
    let visible = VisibleCells::Culled(Vec::new());
    let cpu = StageFrame::default();
    let frame = StreamingFrame {
        visible_cells: &visible,
        sh_visible_cells: &visible,
        camera_cell: Some(0),
        path: PORTAL,
        monotonic_seconds: 0.0,
        settling: false,
        cpu: &cpu,
    };
    level.prepare_drains(&mut None, Some(&set), frame).unwrap();
}

/// The renderer returns the parked batch, installing every pair, with room
/// for eight blocks of band headroom.
fn renderer_returns_batch(level: &mut LevelStreaming) {
    let lightmap = level.lightmap_mut().unwrap();
    if let Some(batch) = lightmap.take_drain_batch_for_renderer() {
        lightmap
            .apply_outcome(LightmapDrainOutcome {
                installed: batch.ready.iter().map(|prepared| prepared.block).collect(),
                pool: headroom(8),
                ..LightmapDrainOutcome::default()
            })
            .unwrap();
    }
}

/// (in-hand bytes, band in-hand bytes, permits in use) after the latest
/// request pass.
fn in_hand(level: &LevelStreaming) -> (u64, u64, usize) {
    let controller = level.lightmap().unwrap().controller();
    (
        controller.in_hand_bytes(),
        controller.band_in_hand_bytes(),
        controller.permits_in_use(),
    )
}

/// Each run's cap: unset, then half and twice production through every lever.
fn caps(levers: &[Lever]) -> Vec<Option<(&Lever, u64)>> {
    let mut caps = vec![None];
    for lever in levers {
        caps.push(Some((lever, PRODUCTION / 2)));
        caps.push(Some((lever, 2 * PRODUCTION)));
    }
    caps
}

fn cap_name(cap: Option<(&Lever, u64)>) -> String {
    cap.map_or_else(
        || "unset".to_string(),
        |(lever, bytes)| format!("{} at {bytes}", lever.name),
    )
}

// The in-hand cap stays at its production total at every lever value.
// Five 10 MiB mandatory pairs at lead 32 m: three submit (30 MiB), and the
// fourth waits at the 32 MiB production total. A cap coupled to the lever
// would stop after one at half production and submit all five at twice.
#[test]
fn a_read_waits_at_the_production_in_hand_total_at_every_lever_value() {
    let levers = levers();
    for cap in caps(&levers) {
        let (mut level, _source) = corridor_level(5 * MIB, 32.0, cap);
        play_frame_at_cell_0(&mut level);
        let (bytes, _, permits) = in_hand(&level);
        assert_eq!((bytes, permits), (30 * MIB, 3), "{}", cap_name(cap));
        assert!(bytes + 10 * MIB > MAX_IN_HAND_PAIR_BYTES);
        level.retire(&mut None);
    }
}

// Band half: at lead 0 m the two mandatory 6 MiB pairs submit first;
// with pool room, two of the three band pairs follow (12 MiB band in hand)
// and the third waits at the 16 MiB production band total, with the full
// cap's 32 MiB not reached. A band cap coupled to the lever would admit no
// band pair at half production and all three at twice.
#[test]
fn a_band_read_waits_at_the_production_band_total_at_every_lever_value() {
    let levers = levers();
    for cap in caps(&levers) {
        let (mut level, _source) = corridor_level(3 * MIB, 0.0, cap);
        play_frame_at_cell_0(&mut level);
        assert_eq!(in_hand(&level), (12 * MIB, 0, 2), "{}", cap_name(cap));
        renderer_returns_batch(&mut level);
        play_frame_at_cell_0(&mut level);
        let (bytes, band, permits) = in_hand(&level);
        assert_eq!(
            (bytes, band, permits),
            (24 * MIB, 12 * MIB, 4),
            "{}",
            cap_name(cap)
        );
        assert!(band + 6 * MIB > MAX_BAND_IN_HAND_PAIR_BYTES);
        assert!(bytes + 6 * MIB <= MAX_IN_HAND_PAIR_BYTES);
        level.retire(&mut None);
    }
}

/// Sets the walk's in-hand lever on `level`'s controller, through its parser.
fn set_walk_in_hand_cap(level: &mut LevelStreaming, mib: &str) {
    let cap = parse_in_hand_cap(mib).unwrap();
    cap.apply(level.lightmap_mut().unwrap().controller_mut());
}

// The walk's in-hand lever moves the full total. Five 8 MiB mandatory pairs
// at lead 32 m: unset, four submit and the fifth waits at the 32 MiB
// production total; at 16 MiB, two submit and the third waits.
#[test]
fn a_read_waits_at_the_walk_in_hand_cap_and_at_production_unset() {
    for (lever, cap) in [(None, MAX_IN_HAND_PAIR_BYTES), (Some("16"), 16 * MIB)] {
        let (mut level, _source) = corridor_level(4 * MIB, 32.0, None);
        if let Some(mib) = lever {
            set_walk_in_hand_cap(&mut level, mib);
        }
        play_frame_at_cell_0(&mut level);
        let (bytes, _, permits) = in_hand(&level);
        assert_eq!(bytes, cap, "{lever:?}");
        assert_eq!(permits as u64, cap / (8 * MIB), "{lever:?}");
        level.retire(&mut None);
    }
}

// The band takes half the walk's total. At lead 0 m the two mandatory 3 MiB
// pairs submit first; with pool room, unset, all three band pairs follow
// (9 MiB, under the 16 MiB production band total). At 16 MiB the band cap is
// 8: two band pairs submit (6 MiB) and the third waits on the band half, with
// the full 16 MiB total not reached.
#[test]
fn a_band_read_waits_at_half_the_walk_in_hand_cap_and_at_production_unset() {
    for (lever, band_cap, band_pairs) in [
        (None, MAX_BAND_IN_HAND_PAIR_BYTES, 3),
        (Some("16"), 8 * MIB, 2),
    ] {
        let (mut level, _source) = corridor_level(3 * MIB / 2, 0.0, None);
        if let Some(mib) = lever {
            set_walk_in_hand_cap(&mut level, mib);
        }
        play_frame_at_cell_0(&mut level);
        assert_eq!(in_hand(&level), (6 * MIB, 0, 2), "{lever:?}");
        renderer_returns_batch(&mut level);
        play_frame_at_cell_0(&mut level);
        let (bytes, band, permits) = in_hand(&level);
        let band_bytes = band_pairs * 3 * MIB;
        assert_eq!(
            (bytes, band, permits),
            (6 * MIB + band_bytes, band_bytes, 2 + band_pairs as usize),
            "{lever:?}"
        );
        assert!(band <= band_cap);
        if band_pairs < 3 {
            assert!(band + 3 * MIB > band_cap);
            assert!(bytes + 3 * MIB <= 16 * MIB);
        }
        level.retire(&mut None);
    }
}

// --- Grep gate --------------------------------------------------------

pub(super) const DEV_TOOLS: &str = r#"#[cfg(feature = "dev-tools")]"#;
pub(super) const TEST: &str = "#[cfg(test)]";
pub(super) const TEST_OR_DEV_TOOLS: &str = r#"#[cfg(any(test, feature = "dev-tools"))]"#;
const TEST_WITH_DEV_TOOLS: &str = r#"#[cfg(all(test, feature = "dev-tools"))]"#;

/// What a scan is inside at a line break.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    Code,
    /// A `"..."` string (or byte string).
    Str,
    /// A raw string closed by `"` and this many `#`.
    Raw(usize),
    /// A (nestable) block comment at this depth.
    Block(u32),
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Bracket depth change over `line`, and the last code character on it.
/// Brackets inside strings, raw strings (`r"..."`, `r#"..."#`, any hash
/// count), char and byte literals, line comments and block comments do not
/// count, and comments and whitespace are not code. `open` carries an
/// unfinished string or block comment to the next line.
fn scan_line(line: &str, open: &mut Open) -> (i32, Option<char>) {
    let chars: Vec<char> = line.chars().collect();
    let (mut delta, mut last, mut index) = (0, None, 0);
    while index < chars.len() {
        let c = chars[index];
        match *open {
            Open::Str => match c {
                '\\' => index += 1,
                '"' => *open = Open::Code,
                _ => {}
            },
            Open::Raw(hashes) => {
                if c == '"' && (1..=hashes).all(|n| chars.get(index + n) == Some(&'#')) {
                    *open = Open::Code;
                    index += hashes;
                }
            }
            Open::Block(depth) => {
                if c == '*' && chars.get(index + 1) == Some(&'/') {
                    *open = if depth == 1 {
                        Open::Code
                    } else {
                        Open::Block(depth - 1)
                    };
                    index += 1;
                } else if c == '/' && chars.get(index + 1) == Some(&'*') {
                    *open = Open::Block(depth + 1);
                    index += 1;
                }
            }
            Open::Code => {
                if c == '/' && chars.get(index + 1) == Some(&'/') {
                    break;
                }
                if c == '/' && chars.get(index + 1) == Some(&'*') {
                    *open = Open::Block(1);
                    index += 2;
                    continue;
                }
                if !c.is_whitespace() {
                    last = Some(c);
                }
                match c {
                    '"' => *open = Open::Str,
                    'r' if index == 0
                        || !is_ident(chars[index - 1])
                        || (chars[index - 1] == 'b'
                            && (index < 2 || !is_ident(chars[index - 2]))) =>
                    {
                        let hashes = chars[index + 1..].iter().take_while(|&&h| h == '#').count();
                        if chars.get(index + 1 + hashes) == Some(&'"') {
                            *open = Open::Raw(hashes);
                            index += 1 + hashes;
                        }
                    }
                    '\'' if chars.get(index + 1) == Some(&'\\') => {
                        index += 3;
                        while index < chars.len() && chars[index] != '\'' {
                            index += 1;
                        }
                    }
                    '\'' if chars.get(index + 2) == Some(&'\'') => index += 2,
                    '(' | '[' | '{' => delta += 1,
                    ')' | ']' | '}' => delta -= 1,
                    _ => {}
                }
            }
        }
        index += 1;
    }
    (delta, last)
}

/// `source` without each item gated by one of `gates`: the attribute, any
/// attributes after it (multi-line ones included), and the item through the
/// first line whose code ends in `;`, `,` or `}` at bracket depth zero, with
/// comments, strings and char literals ignored. Formatted source keeps one
/// gate per line. Panics naming `label` when depth goes negative or the file
/// ends inside a gated item, rather than silently dropping production lines.
pub(super) fn strip_gated(label: &str, source: &str, gates: &[&str]) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut kept = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if !gates.contains(&line.trim()) {
            kept.push(line);
            continue;
        }
        let gate_line = index;
        let (mut depth, mut open, mut ended) = (0, Open::Code, false);
        while index < lines.len() {
            let (delta, last) = scan_line(lines[index], &mut open);
            index += 1;
            depth += delta;
            assert!(
                depth >= 0,
                "{label}:{index}: bracket depth went negative inside the item gated at line {gate_line}"
            );
            if depth == 0 && open == Open::Code && matches!(last, Some(';' | ',' | '}')) {
                ended = true;
                break;
            }
        }
        assert!(
            ended,
            "{label}: the file ends inside the item gated at line {gate_line}"
        );
    }
    kept.join("\n")
}

#[test]
fn strip_gated_drops_each_gated_item_whole_and_keeps_the_rest() {
    let source = [
        "fn kept() {}",
        DEV_TOOLS,
        "#[allow(dead_code)]",
        "pub use debug_ui::{",
        "    Lever,",
        "};",
        "struct S {",
        "    #[cfg(any(test, feature = \"dev-tools\"))]",
        "    seam: Option<u64>,",
        "    kept: u8,",
        "}",
        DEV_TOOLS,
        "let ui = {",
        "    let text = \"{ unbalanced\";",
        "    call(",
        "        seam,",
        "    );",
        "};",
        "fn after() {}",
    ]
    .join("\n");
    assert_eq!(
        strip_gated("fixture", &source, &[DEV_TOOLS, TEST_OR_DEV_TOOLS]),
        [
            "fn kept() {}",
            "struct S {",
            "    kept: u8,",
            "}",
            "fn after() {}"
        ]
        .join("\n")
    );
}

#[test]
fn strip_gated_keeps_the_production_line_after_trailing_comments_attributes_and_literals() {
    let leak = "pub(crate) fn leak(&self) -> Option<NonZeroU64> { self.play_drain_cap_override }";
    let cases: [&[&str]; 7] = [
        // A trailing comment after the terminator.
        &[TEST, "const X: u64 = 1; // test only", leak],
        // A multi-line attribute after the gate.
        &[
            TEST,
            "#[allow(",
            "    long_lint_name",
            ")]",
            "fn x() {}",
            leak,
        ],
        // A bracket inside a block comment, then a multi-line one.
        &[TEST, "fn a() { /* { */ }", leak],
        &[TEST, "fn b() { /* {", "   { */ }", leak],
        // Raw strings with a trailing backslash, and with hashes.
        &[TEST, "const P: &str = r\"C:\\\";", leak],
        &[TEST, "const Q: &str = r#\"a \" { \"#;", leak],
        // Char and byte literals holding brackets and quotes.
        &[TEST, "fn c() { let _ = ('{', b'}', '\\'', '\"'); }", leak],
    ];
    for lines in cases {
        let source = lines.join("\n");
        assert_eq!(strip_gated("fixture", &source, &[TEST]), leak, "{lines:?}");
    }
}

#[test]
#[should_panic(expected = "the file ends inside the item gated")]
fn strip_gated_panics_when_the_file_ends_inside_a_gated_item() {
    strip_gated("fixture", "#[cfg(test)]\nfn open() {\n    body();", &[TEST]);
}

#[test]
#[should_panic(expected = "bracket depth went negative")]
fn strip_gated_panics_when_depth_goes_negative() {
    strip_gated(
        "fixture",
        "struct S {\n    #[cfg(test)]\n    seam: u8\n}",
        &[TEST],
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every file under `dir` whose name passes `keep`, skipping build output.
fn files_under(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !path.ends_with("target") && !path.ends_with("node_modules") {
                    stack.push(path);
                }
            } else if keep(&path) {
                files.push(path);
            }
        }
    }
    files
}

fn rust_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    files_under(dir, &|path| {
        path.extension().is_some_and(|extension| extension == "rs")
    })
    .into_iter()
    .map(|path| {
        let source = std::fs::read_to_string(&path).unwrap();
        (path, source)
    })
    .collect()
}

/// Every module `sources` declare out of line behind `#[cfg(test)]`, as the
/// file and the directory its source may live in.
fn test_only_modules(sources: &[(PathBuf, String)]) -> Vec<PathBuf> {
    let mut modules = Vec::new();
    for (path, source) in sources {
        let stem = path.file_stem().unwrap();
        let children = if stem == "mod" || stem == "main" || stem == "lib" {
            path.parent().unwrap().to_path_buf()
        } else {
            path.with_extension("")
        };
        let lines: Vec<&str> = source.lines().collect();
        for pair in lines.windows(2) {
            let declaration = pair[1].trim();
            let declaration = ["pub(crate) ", "pub(super) ", "pub "]
                .iter()
                .find_map(|visibility| declaration.strip_prefix(visibility))
                .unwrap_or(declaration);
            if pair[0].trim() == TEST
                && let Some(name) = declaration
                    .strip_prefix("mod ")
                    .and_then(|rest| rest.strip_suffix(';'))
            {
                modules.push(children.join(name));
            }
        }
    }
    modules
}

/// Production files under `dir` that still name any of `needles` once every
/// item behind `gates` is stripped. Test sources and test-only modules are
/// skipped.
fn ungated_mentions(dir: &Path, gates: &[&str], needles: &[&str]) -> Vec<String> {
    let sources = rust_sources(dir);
    let test_only = test_only_modules(&sources);
    sources
        .into_iter()
        .filter(|(path, _)| !is_test_source(path))
        .filter(|(path, _)| {
            !test_only
                .iter()
                .any(|module| path.starts_with(module) || *path == module.with_extension("rs"))
        })
        .filter_map(|(path, source)| {
            let production = strip_gated(&path.display().to_string(), &source, gates);
            needles
                .iter()
                .find(|needle| production.contains(**needle))
                .map(|needle| format!("{}: {needle}", path.display()))
        })
        .collect()
}

// The cap slider builds only with dev-tools. The renderer's whole
// debug UI, the lever's module included, is behind the feature; outside it,
// and across the engine binary, the lever is named only in gated items.
#[test]
fn drain_cap_slider_builds_only_with_dev_tools() {
    let renderer = workspace_root().join("crates/renderer/src");
    let render_mod = std::fs::read_to_string(renderer.join("render/mod.rs")).unwrap();
    assert!(render_mod.contains("mod debug_ui;"));
    assert!(!strip_gated("render/mod.rs", &render_mod, &[DEV_TOOLS]).contains("mod debug_ui;"));
    let debug_ui = std::fs::read_to_string(renderer.join("render/debug_ui/mod.rs")).unwrap();
    assert!(debug_ui.contains("mod drain_cap_lever;"));

    let lever = ["StreamingDrainCapLever", "drain_cap_lever"];
    let outside_debug_ui: Vec<String> = ungated_mentions(&renderer, &[DEV_TOOLS], &lever)
        .into_iter()
        // Windows joins with `\`; compare on `/` so the filter holds there.
        .filter(|mention| !mention.replace('\\', "/").contains("render/debug_ui/"))
        .collect();
    assert!(outside_debug_ui.is_empty(), "{outside_debug_ui:#?}");

    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let gates = [DEV_TOOLS, TEST, TEST_WITH_DEV_TOOLS];
    let in_engine = ungated_mentions(&engine, &gates, &lever);
    assert!(in_engine.is_empty(), "{in_engine:#?}");
}

// The walk overrides, the drain cap and the test-only in-hand lever,
// live only in the walk harness, which is test code.
#[test]
fn walk_drain_cap_override_lives_only_in_the_test_walk_harness() {
    let harness = Path::new("lightmap_residency").join("walk_measurement");
    let crates = workspace_root().join("crates");
    let sources = rust_sources(&crates);
    for env in [
        concat!("POSTRETRO_LIGHTMAP_WALK_", "DRAIN_CAP"),
        concat!("POSTRETRO_LIGHTMAP_WALK_", "IN_HAND_CAP"),
    ] {
        let named: Vec<&PathBuf> = sources
            .iter()
            .filter(|(_, source)| source.contains(env))
            .map(|(path, _)| path)
            .collect();
        assert!(!named.is_empty(), "the walk harness names {env}");
        let outside: Vec<&&PathBuf> = named
            .iter()
            .filter(|path| !path.parent().is_some_and(|dir| dir.ends_with(&harness)))
            .collect();
        assert!(outside.is_empty(), "{env}: {outside:#?}");
    }

    let residency = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session/lightmap_residency/mod.rs"),
    )
    .unwrap();
    assert!(residency.contains("mod walk_measurement;"));
    assert!(
        !strip_gated("lightmap_residency/mod.rs", &residency, &[TEST])
            .contains("mod walk_measurement")
    );
}

// Both walk tallies, the same-class cut counter and SH's per-frame
// late tally, and the drain views they read build only for tests.
#[test]
fn walk_tallies_and_their_seams_are_test_only() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let seams = [
        "same_class_cut",
        "latest_update_miss_frames",
        "ordered_items",
        "fn shared_drain",
    ];
    let named: Vec<&str> = seams
        .iter()
        .copied()
        .filter(|seam| {
            rust_sources(&engine)
                .iter()
                .any(|(_, source)| source.contains(seam))
        })
        .collect();
    assert_eq!(named, seams, "every seam exists to be gated");
    let ungated = ungated_mentions(&engine, &[TEST], &seams);
    assert!(ungated.is_empty(), "{ungated:#?}");
}

// The override seam builds only for tests and dev-tools; a production
// build admits play drains against the constant alone.
#[test]
fn play_drain_cap_override_seam_is_test_or_dev_tools_only() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let seam = "play_drain_cap_override";
    let level_streaming =
        std::fs::read_to_string(engine.join("session/level_streaming/mod.rs")).unwrap();
    assert!(level_streaming.contains(seam));
    let gates = [DEV_TOOLS, TEST, TEST_OR_DEV_TOOLS, TEST_WITH_DEV_TOOLS];
    let ungated = ungated_mentions(&engine, &gates, &[seam]);
    assert!(ungated.is_empty(), "{ungated:#?}");
}

// The in-hand lever's controller override, its setter and the
// session's mutable controller view build only for tests; a production
// build waits at the in-hand constants alone.
#[test]
fn in_hand_cap_override_seam_is_test_only() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let seams = ["in_hand_cap_override", "fn controller_mut"];
    let controller =
        std::fs::read_to_string(engine.join("lightmap_streaming/controller.rs")).unwrap();
    assert!(controller.contains(seams[0]));
    let ungated = ungated_mentions(&engine, &[TEST], &seams);
    assert!(ungated.is_empty(), "{ungated:#?}");
    let production = strip_gated("lightmap_streaming/controller.rs", &controller, &[TEST]);
    for (accessor, constant) in [
        ("in_hand_cap", "MAX_IN_HAND_PAIR_BYTES"),
        ("band_in_hand_cap", "MAX_BAND_IN_HAND_PAIR_BYTES"),
    ] {
        let body = format!("fn {accessor}(&self) -> u64 {{\n        {constant}\n    }}");
        assert!(
            production.contains(&body),
            "production {accessor} is {constant}"
        );
    }
}

// No player setting names the drain cap: nothing in the options,
// settings, persistence or save code, nor in the engine-owned UI and SDK
// trees a player-facing menu is built from.
#[test]
fn no_player_setting_names_the_drain_cap() {
    let needles = [
        "drain_cap",
        "draincap",
        "drain cap",
        "drain-cap",
        "drain_budget",
        "drain budget",
        "drainbudget",
        "drain-budget",
        "install_decoded_bytes",
    ];
    let root = workspace_root();
    let player_facing = |path: &Path| {
        let relative = path.strip_prefix(&root).unwrap_or(path);
        let lower = relative.to_string_lossy().to_lowercase();
        ["option", "setting", "persist", "save"]
            .iter()
            .any(|word| lower.contains(word))
    };
    let mut scanned = files_under(&root.join("crates"), &|path| {
        player_facing(path) && path.extension().is_some_and(|extension| extension == "rs")
    });
    for tree in ["core/ui", "sdk"] {
        scanned.extend(files_under(&root.join(tree), &|_| true));
    }
    assert!(
        scanned
            .iter()
            .any(|path| path.ends_with("options/persist.rs")),
        "the scan reaches the options persistence"
    );
    let offenders: Vec<String> = scanned
        .iter()
        .filter_map(|path| {
            let text = std::fs::read(path).ok()?;
            let text = String::from_utf8_lossy(&text).to_lowercase();
            needles
                .iter()
                .find(|needle| text.contains(**needle))
                .map(|needle| format!("{}: {needle}", path.display()))
        })
        .collect();
    assert!(offenders.is_empty(), "{offenders:#?}");
}
