// GPU capture proofs for direction-aware probe visibility: closet fixtures
// rendered with their baked id 59 against the same PRL without it.
// See: context/lib/rendering_pipeline.md §4 (Probe depth moments)
//
// "Today" is the tiled bake with id 59 stripped, so every other section is
// byte-identical and only the tiles differ. Means and pixel moves are Rec. 709
// luma of the post-tonemap sRGB capture (0–255), as the spike measured them;
// a "move" is |Δluma| > 2. Captures compare on one adapter; no golden images.

use std::fs;
use std::path::{Path, PathBuf};

use image::RgbaImage;
use postretro_level_format as prl_format;
use postretro_level_format::SectionId;
use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;

mod capture_support;

use capture_support::{capture_command, compile_dev_map, load_capture_rgba, workspace_root};

const CLOSET_WIDTH: u32 = 960;
const CLOSET_HEIGHT: u32 = 540;
/// TrenchBroom unit in metres.
const UNIT: f32 = 0.0254;

/// A capture pose in map units, yaw and pitch in degrees.
#[derive(Clone, Copy)]
struct Pose {
    map: [f32; 3],
    yaw: f32,
    pitch: f32,
}

/// Station 2: inside the closet, facing the door.
const STATION_2: Pose = Pose {
    map: [320.0, 720.0, 64.0],
    yaw: 270.0,
    pitch: -6.0,
};
/// Station 3: the closet floor at the door.
const STATION_3: Pose = Pose {
    map: [320.0, 610.0, 96.0],
    yaw: 270.0,
    pitch: -38.0,
};

fn camera(pose: Pose) -> serde_json::Value {
    let [x, y, z] = pose.map;
    serde_json::json!({
        "position": [-y * UNIT, z * UNIT, -x * UNIT],
        "yaw_deg": pose.yaw,
        "pitch_deg": pose.pitch,
        "fov_deg": 100.0
    })
}

fn read_sections(path: &Path) -> Vec<prl_format::SectionBlob> {
    let mut file = fs::File::open(path).expect("open compiled fixture");
    let meta = prl_format::read_container(&mut file).expect("read fixture container");
    meta.sections
        .iter()
        .map(|entry| prl_format::SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data: prl_format::read_section_data(&mut file, &meta, entry.section_id)
                .expect("read fixture section")
                .expect("listed section must be present"),
        })
        .collect()
}

fn tiles_of(sections: &[prl_format::SectionBlob]) -> ProbeDepthTilesSection {
    let blob = sections
        .iter()
        .find(|blob| blob.section_id == SectionId::ProbeDepthTiles as u32)
        .expect("a tiled bake ships id 59");
    ProbeDepthTilesSection::from_bytes(&blob.data).expect("id 59 must parse")
}

/// A PRL beside the compiled fixture, so capture still derives its materials
/// root from content/dev/maps. Deletes itself on drop.
fn sibling_prl(workspace: &Path, label: &str) -> tempfile::TempPath {
    tempfile::Builder::new()
        .prefix(&format!(".{label}-"))
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve variant PRL path")
        .into_temp_path()
}

/// Write `sections` with id 59 replaced by `id59` (`None` drops it).
fn write_with_id59(sections: &[prl_format::SectionBlob], id59: Option<Vec<u8>>, path: &Path) {
    let blobs: Vec<_> = sections
        .iter()
        .filter_map(|blob| {
            if blob.section_id != SectionId::ProbeDepthTiles as u32 {
                return Some(prl_format::SectionBlob {
                    section_id: blob.section_id,
                    version: blob.version,
                    data: blob.data.clone(),
                });
            }
            id59.clone().map(|data| prl_format::SectionBlob {
                section_id: blob.section_id,
                version: blob.version,
                data,
            })
        })
        .collect();
    let mut file = fs::File::create(path).expect("create variant PRL");
    prl_format::write_prl(&mut file, &blobs).expect("write variant PRL");
}

/// One fixture compiled with tiles, plus the same PRL with id 59 stripped.
struct TiledPair {
    _compiled: tempfile::TempPath,
    tiled: PathBuf,
    today: tempfile::TempPath,
    sections: Vec<prl_format::SectionBlob>,
}

fn tiled_pair(workspace: &Path, map_name: &str) -> TiledPair {
    let compiled = compile_dev_map(workspace, map_name);
    let sections = read_sections(&compiled);
    let today = sibling_prl(workspace, &format!("{map_name}-no-id59"));
    write_with_id59(&sections, None, &today);
    TiledPair {
        tiled: compiled.to_path_buf(),
        _compiled: compiled,
        today,
        sections,
    }
}

/// Run one capture and return its image. Fails rather than skips without an
/// adapter: this proof does not count unless it ran.
fn capture(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
    pose: Pose,
    resolution: [u32; 2],
    light_term_mask: Option<u32>,
) -> RgbaImage {
    let scene_path = scratch.join(format!("{label}.scene.json"));
    let output_path = scratch.join(format!("{label}.png"));
    let mut scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": camera(pose),
        "resolution": resolution,
        "output": output_path.display().to_string(),
        "force_full_resident_sh_compose": true
    });
    if let Some(mask) = light_term_mask {
        scene["light_term_mask"] = mask.into();
    }
    fs::write(&scene_path, serde_json::to_vec_pretty(&scene).unwrap()).unwrap();
    let result = capture_command(workspace, &scene_path)
        .env("RUST_LOG", "warn")
        .output()
        .expect("launch postretro capture");
    assert!(
        result.status.success(),
        "capture `{label}` failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    load_capture_rgba(&output_path)
}

fn luma([r, g, b, _]: [u8; 4]) -> f64 {
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}

fn frame_mean(image: &RgbaImage) -> f64 {
    image.pixels().map(|p| luma(p.0)).sum::<f64>() / f64::from(image.width() * image.height())
}

/// Pixels whose luma moves by more than 2 levels from `before` to `after`.
#[derive(Debug, Default)]
struct Moves {
    darker: usize,
    brighter: usize,
    total: usize,
}

fn pixel_moves(before: &RgbaImage, after: &RgbaImage) -> Moves {
    assert_eq!(before.dimensions(), after.dimensions());
    let mut moves = Moves {
        total: (before.width() * before.height()) as usize,
        ..Moves::default()
    };
    for (a, b) in before.pixels().zip(after.pixels()) {
        let delta = luma(b.0) - luma(a.0);
        if delta < -2.0 {
            moves.darker += 1;
        } else if delta > 2.0 {
            moves.brighter += 1;
        }
    }
    moves
}

fn closet_capture(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
    pose: Pose,
) -> RgbaImage {
    capture(
        workspace,
        scratch,
        map,
        label,
        pose,
        [CLOSET_WIDTH, CLOSET_HEIGHT],
        None,
    )
}

// A1, A2: the static-door reference reproduces the leak without tiles, and
// tiles take it out.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn static_door_closet_glows_without_tiles_and_reads_dark_with_them() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "mover-closet-static-door");
    assert!(
        tiles_of(&pair.sections).tile_count() > 0,
        "the closet bakes tiles"
    );

    let today_st3 = frame_mean(&closet_capture(
        &workspace,
        scratch.path(),
        &pair.today,
        "today-st3",
        STATION_3,
    ));
    let today_st2 = frame_mean(&closet_capture(
        &workspace,
        scratch.path(),
        &pair.today,
        "today-st2",
        STATION_2,
    ));
    let tiled_st3 = frame_mean(&closet_capture(
        &workspace,
        scratch.path(),
        &pair.tiled,
        "tiled-st3",
        STATION_3,
    ));
    let tiled_st2 = frame_mean(&closet_capture(
        &workspace,
        scratch.path(),
        &pair.tiled,
        "tiled-st2",
        STATION_2,
    ));
    eprintln!(
        "static door: today st3 {today_st3:.3} st2 {today_st2:.3}; tiled st3 {tiled_st3:.3} st2 {tiled_st2:.3}"
    );
    assert!(
        today_st3 >= 0.4,
        "the fixture must reproduce the leak: st3 {today_st3}"
    );
    assert!(tiled_st3 <= 0.05, "tiles must darken st3: {tiled_st3}");
    assert!(tiled_st2 <= 0.02, "tiles must darken st2: {tiled_st2}");
}

// A8: an absent id 59 and a zero-tile id 59 render identically.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn absent_and_zero_tile_id59_render_byte_identical() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "mover-closet-static-door");
    let zero = sibling_prl(&workspace, "mover-closet-static-door-zero-tiles");
    write_with_id59(
        &pair.sections,
        Some(ProbeDepthTilesSection::default().to_bytes()),
        &zero,
    );
    for (pose, name) in [(STATION_2, "st2"), (STATION_3, "st3")] {
        let absent = closet_capture(
            &workspace,
            scratch.path(),
            &pair.today,
            &format!("absent-{name}"),
            pose,
        );
        let empty = closet_capture(
            &workspace,
            scratch.path(),
            &zero,
            &format!("zero-{name}"),
            pose,
        );
        assert!(
            absent.as_raw() == empty.as_raw(),
            "{name}: absent and zero-tile id 59 must render byte-identical"
        );
    }
}

fn station_means(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
) -> [(RgbaImage, f64); 2] {
    [(STATION_2, "st2"), (STATION_3, "st3")].map(|(pose, name)| {
        let image = closet_capture(workspace, scratch, map, &format!("{label}-{name}"), pose);
        let mean = frame_mean(&image);
        (image, mean)
    })
}

// A5: an open doorway reads as it did; tiles touch only probes that need them.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn open_doorway_reads_as_today() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "mover-closet-no-door");
    let today = station_means(&workspace, scratch.path(), &pair.today, "today");
    let tiled = station_means(&workspace, scratch.path(), &pair.tiled, "tiled");
    for (name, (before, before_mean), (after, after_mean)) in
        [("st2", &today[0], &tiled[0]), ("st3", &today[1], &tiled[1])]
    {
        let moves = pixel_moves(before, after);
        eprintln!(
            "open doorway {name}: today {before_mean:.3} tiled {after_mean:.3}; moves {moves:?}"
        );
        assert!(
            (after_mean - before_mean).abs() <= 0.02,
            "{name}: frame mean moved {before_mean} → {after_mean}"
        );
        assert!(
            (moves.darker + moves.brighter) * 1000 <= moves.total,
            "{name}: more than 0.1% of pixels moved by more than 2 levels: {moves:?}"
        );
    }
}

// A7: the mover-door closet still glows by design (probe depth bakes with
// movers absent); only a darkening would flag a dropped probe.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn mover_door_closet_does_not_darken() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "mover-closet");
    let today = station_means(&workspace, scratch.path(), &pair.today, "today");
    let tiled = station_means(&workspace, scratch.path(), &pair.tiled, "tiled");
    for (name, (_, before), (_, after)) in
        [("st2", &today[0], &tiled[0]), ("st3", &today[1], &tiled[1])]
    {
        eprintln!("mover door {name}: today {before:.3} tiled {after:.3}");
        assert!(
            *after >= before - 0.02,
            "{name}: darkened {before} → {after}"
        );
    }
}

// A17b (owner restatement, 2026-11-07): the convex room's wall-hugging probes
// take tiles, and those tiles change no pixel visibly.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn convex_room_tiles_darken_nothing_visible() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "probe-tiles-convex-room");
    eprintln!(
        "convex room: {} tiles",
        tiles_of(&pair.sections).tile_count()
    );
    // Along the west wall, where the probes 4.8 mm off it sit, and across the room.
    for (pose, name) in [
        (
            Pose {
                map: [48.0, 24.0, 64.0],
                yaw: 100.0,
                pitch: -8.0,
            },
            "west-wall",
        ),
        (
            Pose {
                map: [400.0, 300.0, 128.0],
                yaw: 200.0,
                pitch: -15.0,
            },
            "across",
        ),
    ] {
        let before = closet_capture(
            &workspace,
            scratch.path(),
            &pair.today,
            &format!("today-{name}"),
            pose,
        );
        let after = closet_capture(
            &workspace,
            scratch.path(),
            &pair.tiled,
            &format!("tiled-{name}"),
            pose,
        );
        let moves = pixel_moves(&before, &after);
        eprintln!("convex room {name}: {moves:?}");
        assert_eq!(
            moves.darker, 0,
            "{name}: tiles darkened pixels by more than 2 levels: {moves:?}"
        );
    }
}

/// Hallway Q2 views (spike `kit/win/q2.py`), engine coordinates.
const HALLWAY_POSITION: [f32; 3] = [32.1056, 22.86, 53.2384];
const HALLWAY_VIEWS: [(&str, f32); 3] = [("a", -135.0), ("b", 45.0), ("c", -45.0)];

// A6, A25: the hallway's tiles darken no pixel at the Q2 views, and its tile
// texture stays within the memory ceiling (spike count at 8×8 plus 25%).
#[test]
#[ignore = "bakes stress-warren-hallway-inspection (slow) and needs a GPU adapter; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles hallway -- --ignored`"]
fn hallway_tiles_darken_no_pixel_and_fit_the_memory_ceiling() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let pair = tiled_pair(&workspace, "stress-warren-hallway-inspection");
    let tiles = tiles_of(&pair.sections);
    let bytes = postretro_level_format::probe_depth_tiles::probe_depth_tile_texture_bytes(
        tiles.tile_count() as u32,
    );
    eprintln!(
        "hallway: {} tiles, tile texture {bytes} bytes ({:.2} MiB)",
        tiles.tile_count(),
        bytes as f64 / (1024.0 * 1024.0)
    );
    assert!(
        bytes as f64 <= 6.4 * 1024.0 * 1024.0,
        "hallway tiles over 6.4 MiB: {bytes}"
    );
    for (name, yaw) in HALLWAY_VIEWS {
        let [x, y, z] = HALLWAY_POSITION;
        // Map-unit pose that `camera` converts back to these engine coordinates.
        let pose = Pose {
            map: [-z / UNIT, -x / UNIT, y / UNIT],
            yaw,
            pitch: -12.0,
        };
        let before = capture(
            &workspace,
            scratch.path(),
            &pair.today,
            &format!("today-{name}"),
            pose,
            [1920, 1080],
            None,
        );
        let after = capture(
            &workspace,
            scratch.path(),
            &pair.tiled,
            &format!("tiled-{name}"),
            pose,
            [1920, 1080],
            None,
        );
        let moves = pixel_moves(&before, &after);
        eprintln!("hallway {name}: {moves:?}");
        assert_eq!(
            moves.darker, 0,
            "hallway {name}: pixels darkened by more than 2 levels: {moves:?}"
        );
    }
}
