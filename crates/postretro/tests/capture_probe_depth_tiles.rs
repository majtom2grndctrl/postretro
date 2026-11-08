// GPU capture proofs for direction-aware probe visibility: closet fixtures
// rendered with their baked id 59 against the same PRL without it. The closet
// pool rows also prove promotion never changes a fluid.
// See: context/lib/rendering_pipeline.md §4 (Probe depth moments, Promoted
// static lights)
//
// "Today" is the tiled bake with id 59 stripped, so every other section is
// byte-identical and only the tiles differ. Means and pixel moves are Rec. 709
// luma of the post-tonemap sRGB capture (0–255), as the pilot measured them;
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
/// Horizontal field of view of every capture here, in degrees.
const FOV_DEG: f32 = 100.0;
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
        "fov_deg": FOV_DEG
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

/// Capture-scene fields beyond the map, pose, and resolution.
#[derive(Clone, Copy, Default)]
struct SceneExtras {
    light_term_mask: Option<u32>,
    /// A `force_promotion` entry: tag and pinned `w`.
    force_promotion: Option<(&'static str, f32)>,
}

/// Run one capture; `Err` carries the child's output when it fails.
fn try_capture(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
    pose: Pose,
    resolution: [u32; 2],
    extras: SceneExtras,
) -> Result<RgbaImage, String> {
    let scene_path = scratch.join(format!("{label}.scene.json"));
    let output_path = scratch.join(format!("{label}.png"));
    let mut scene = serde_json::json!({
        "map": map.display().to_string(),
        "camera": camera(pose),
        "resolution": resolution,
        "output": output_path.display().to_string(),
        "force_full_resident_sh_compose": true
    });
    if let Some(mask) = extras.light_term_mask {
        scene["light_term_mask"] = mask.into();
    }
    if let Some((tag, weight)) = extras.force_promotion {
        scene["force_promotion"] = serde_json::json!([{ "tag": tag, "weight": weight }]);
    }
    fs::write(&scene_path, serde_json::to_vec_pretty(&scene).unwrap()).unwrap();
    let result = capture_command(workspace, &scene_path)
        .env("RUST_LOG", "warn")
        .output()
        .expect("launch postretro capture");
    if !result.status.success() {
        return Err(format!(
            "capture `{label}` failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        ));
    }
    Ok(load_capture_rgba(&output_path))
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
    extras: SceneExtras,
) -> RgbaImage {
    try_capture(workspace, scratch, map, label, pose, resolution, extras)
        .unwrap_or_else(|failure| panic!("{failure}"))
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
        SceneExtras::default(),
    )
}

// The static-door reference reproduces the leak without tiles, and tiles take
// it out.
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

// An absent id 59 and a zero-tile id 59 render identically.
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

// An open doorway reads as it did; tiles touch only probes that need them.
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

// The mover-door closet still glows by design (probe depth bakes with
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

// The convex room's wall-hugging probes take tiles, and the tiled build moves
// no pixel by more than 2 levels against the same PRL without id 59.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn convex_room_tiles_move_no_pixel() {
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
            (moves.darker, moves.brighter),
            (0, 0),
            "{name}: tiles moved pixels by more than 2 levels: {moves:?}"
        );
    }
}

/// Three hallway views at one position, engine coordinates.
const HALLWAY_POSITION: [f32; 3] = [32.1056, 22.86, 53.2384];
const HALLWAY_VIEWS: [(&str, f32); 3] = [("a", -135.0), ("b", 45.0), ("c", -45.0)];

// The hallway's tiles darken no pixel at the three views, and its tile texture
// stays within the memory ceiling (the per-direction depth spike's count at
// 8×8 plus 25%). The byte count recomputes `probe_depth_tile_texture_bytes`;
// the renderer's ledger test proves the ledger records exactly these bytes, so
// the formula stands for the ledger.
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
            SceneExtras::default(),
        );
        let after = capture(
            &workspace,
            scratch.path(),
            &pair.tiled,
            &format!("tiled-{name}"),
            pose,
            [1920, 1080],
            SceneExtras::default(),
        );
        let moves = pixel_moves(&before, &after);
        eprintln!("hallway {name}: {moves:?}");
        assert_eq!(
            moves.darker, 0,
            "hallway {name}: pixels darkened by more than 2 levels: {moves:?}"
        );
    }
}

/// Largest share of its no-tile brightness the kinematic receiver behind the
/// static door may keep with tiles Measured in a pilot on a
/// Radeon Pro 5300M: the kinematic slab kept 0.006 of its no-tile brightness
/// (3.434 -> 0.020 luma over 7,644 px), and a sprite on the closet floor at the
/// door kept 0.033-0.049 (billboard sampler mirror, compiler test). 0.10 is the
/// worst measured share with about 2x margin.
const KINEMATIC_FRACTION_CEILING: f64 = 0.10;

/// Pixel of a map-unit point seen from `pose` at `resolution`, with the
/// capture's horizontal field of view; `None` behind the camera.
fn project(point: [f32; 3], pose: Pose, fov_deg: f32, resolution: [u32; 2]) -> Option<[f32; 2]> {
    let d = [0, 1, 2].map(|i| point[i] - pose.map[i]);
    let (sy, cy) = pose.yaw.to_radians().sin_cos();
    let (sp, cp) = pose.pitch.to_radians().sin_cos();
    let forward = [cy * cp, sy * cp, sp];
    let right = [sy, -cy, 0.0];
    let up = [-cy * sp, -sy * sp, cp];
    let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    let depth = dot(forward);
    if depth <= 0.0 {
        return None;
    }
    let [w, h] = resolution.map(|v| v as f32);
    let half_x = (fov_deg.to_radians() / 2.0).tan();
    let half_y = half_x * h / w;
    Some([
        w / 2.0 * (1.0 + dot(right) / depth / half_x),
        h / 2.0 * (1.0 - dot(up) / depth / half_y),
    ])
}

/// The kinematic receiver's slab box in map units,
/// mover-closet.README.md §Reference variants.
const RECEIVER_SLAB_MIN: [f32; 3] = [296.0, 505.0, 0.0];
const RECEIVER_SLAB_MAX: [f32; 3] = [344.0, 521.0, 8.0];

/// Screen bounding box of the receiver slab's projected corners at a pose.
struct SlabScreenBox {
    lo: [f32; 2],
    hi: [f32; 2],
}

impl SlabScreenBox {
    /// The receiver map's slab at `pose`.
    fn of(pose: Pose) -> Self {
        Self::of_box(RECEIVER_SLAB_MIN, RECEIVER_SLAB_MAX, pose)
    }

    /// Any map-unit box at `pose`; every corner must be in front.
    fn of_box(min: [f32; 3], max: [f32; 3], pose: Pose) -> Self {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let corners = [x0, x1].into_iter().flat_map(|x| {
            [y0, y1]
                .into_iter()
                .flat_map(move |y| [[x, y, z0], [x, y, z1]])
        });
        let pixels: Vec<[f32; 2]> = corners
            .map(|c| {
                project(c, pose, FOV_DEG, [CLOSET_WIDTH, CLOSET_HEIGHT]).expect("slab in front")
            })
            .collect();
        Self {
            lo: pixels
                .iter()
                .fold([f32::MAX; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]),
            hi: pixels
                .iter()
                .fold([f32::MIN; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]),
        }
    }

    fn contains(&self, x: u32, y: u32) -> bool {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        fx >= self.lo[0] && fx <= self.hi[0] && fy >= self.lo[1] && fy <= self.hi[1]
    }
}

// Kinematic half: a resting kinematic slab on the closet floor just behind
// the static door keeps at most a set share of its no-tile brightness. Its
// pixels are those inside its projected box that it changes against the plain
// static-door map, both without tiles. (The sprite half reads through the
// billboard sampler's CPU mirror: capture draws no billboards.)
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn kinematic_receiver_behind_the_static_door_darkens_with_tiles() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let plain = tiled_pair(&workspace, "mover-closet-static-door");
    let receiver = tiled_pair(&workspace, "mover-closet-kinematic-receiver");
    let backdrop = closet_capture(
        &workspace,
        scratch.path(),
        &plain.today,
        "backdrop",
        STATION_3,
    );
    let today = closet_capture(
        &workspace,
        scratch.path(),
        &receiver.today,
        "today",
        STATION_3,
    );
    let tiled = closet_capture(
        &workspace,
        scratch.path(),
        &receiver.tiled,
        "tiled",
        STATION_3,
    );

    let slab = SlabScreenBox::of(STATION_3);
    let (mut before, mut after, mut count) = (0.0f64, 0.0f64, 0usize);
    for (x, y, pixel) in today.enumerate_pixels() {
        if !slab.contains(x, y) {
            continue;
        }
        if backdrop.get_pixel(x, y).0 == pixel.0 {
            continue;
        }
        before += luma(pixel.0);
        after += luma(tiled.get_pixel(x, y).0);
        count += 1;
    }
    assert!(count > 0, "the slab must be in station 3's view");
    let (before, after) = (before / count as f64, after / count as f64);
    let fraction = after / before;
    eprintln!(
        "kinematic slab: {count} px, today {before:.3}, tiled {after:.3}, fraction {fraction:.3}"
    );
    assert!(before > 1.0, "the slab must glow without tiles: {before}");
    assert!(
        fraction <= KINEMATIC_FRACTION_CEILING,
        "the slab keeps {fraction:.3} of its no-tile brightness"
    );
}

/// The closet pool's `closet_water` surface in map units,
/// mover-closet.README.md §Layout.
const POOL_X: [f32; 2] = [224.0, 416.0];
const POOL_Y: [f32; 2] = [544.0, 688.0];
const POOL_SURFACE_Z: f32 = -8.0;
/// Map units trimmed from each pool edge: the pit lip stands 8 units above the
/// surface, so a near-edge view ray can meet the lip first, and edge pixels
/// straddle the floor.
const POOL_INSET: f32 = 8.0;
/// Fewest pool pixels a row accepts, so a mean never rests on a sliver.
const MIN_POOL_PIXELS: usize = 2_000;

/// Pixels whose view ray meets the pool surface inside its inset rectangle,
/// by the inverse of `project`. Nothing stands between either closet station
/// and the inset surface.
fn pool_pixels(pose: Pose) -> Vec<(u32, u32)> {
    let (sy, cy) = pose.yaw.to_radians().sin_cos();
    let (sp, cp) = pose.pitch.to_radians().sin_cos();
    let forward = [cy * cp, sy * cp, sp];
    let right = [sy, -cy, 0.0];
    let up = [-cy * sp, -sy * sp, cp];
    let [w, h] = [CLOSET_WIDTH, CLOSET_HEIGHT].map(|v| v as f32);
    let half_x = (FOV_DEG.to_radians() / 2.0).tan();
    let half_y = half_x * h / w;
    let mut pixels = Vec::new();
    for y in 0..CLOSET_HEIGHT {
        for x in 0..CLOSET_WIDTH {
            let nx = 2.0 * (x as f32 + 0.5) / w - 1.0;
            let ny = 1.0 - 2.0 * (y as f32 + 0.5) / h;
            let dir: [f32; 3] =
                std::array::from_fn(|i| forward[i] + right[i] * nx * half_x + up[i] * ny * half_y);
            if dir[2] >= 0.0 {
                continue;
            }
            let t = (POOL_SURFACE_Z - pose.map[2]) / dir[2];
            let hit = [pose.map[0] + t * dir[0], pose.map[1] + t * dir[1]];
            if hit[0] >= POOL_X[0] + POOL_INSET
                && hit[0] <= POOL_X[1] - POOL_INSET
                && hit[1] >= POOL_Y[0] + POOL_INSET
                && hit[1] <= POOL_Y[1] - POOL_INSET
            {
                pixels.push((x, y));
            }
        }
    }
    pixels
}

fn rgb_sum([r, g, b, _]: [u8; 4]) -> f64 {
    f64::from(r) + f64::from(g) + f64::from(b)
}

fn region_mean(image: &RgbaImage, pixels: &[(u32, u32)], measure: fn([u8; 4]) -> f64) -> f64 {
    pixels
        .iter()
        .map(|&(x, y)| measure(image.get_pixel(x, y).0))
        .sum::<f64>()
        / pixels.len() as f64
}

/// Ceiling on the static-door pool's mean luma at station 3: the closet's
/// tiled station-3 frame-mean ceiling (`static_door_closet_glows_...`).
const DARK_POOL_CEILING: f64 = 0.05;
/// How far the kinematic-receiver pool may read above the static-door pool at
/// station 3. Promotion must not change a fluid, so the only legitimate
/// difference is the slab's own bake footprint (its mover light gate and probe
/// channel), which ends short of the pit. Measured on a Radeon Pro 5300M: both
/// pools 0.000 over 71,334 px, unpinned. A fluid that adds the promoted lights
/// back unshadowed reads 6.48 in the same box at the unpinned capture's w of
/// about 0.06; 0.25 is under 4% of that and leaves room for one-level rounding
/// on a few percent of pixels.
const RECEIVER_POOL_EPSILON: f64 = 0.25;
/// The receiver map's tagged centre bright-room light: of the three, the one
/// nearest station 3, so it ranks first for the two cube slots the slab's
/// point lights compete for. mover-closet.README.md §Reference variants.
const CENTRE_LIGHT_TAG: &str = "centre_light";

// A promoted light never lights the closet pool through the static door. The
// kinematic slab behind the door promotes the bright-room lights, and the
// capture pins the centre one fully promoted (w=1). A promoted light reaches a
// fluid only through its promotion-free baked direct term; adding its runtime
// term back unshadowed would light the pool through the wall, where the
// static-door map reads black. The pin fails the capture if the centre light
// holds no slot, so the row cannot pass on an unpromoted light.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn kinematic_receiver_leaves_the_closet_pool_as_dark_as_the_static_door() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let plain = compile_dev_map(&workspace, "mover-closet-static-door");
    let receiver = compile_dev_map(&workspace, "mover-closet-kinematic-receiver");
    let slab = SlabScreenBox::of(STATION_3);
    let pool: Vec<_> = pool_pixels(STATION_3)
        .into_iter()
        .filter(|&(x, y)| !slab.contains(x, y))
        .collect();
    assert!(
        pool.len() >= MIN_POOL_PIXELS,
        "station 3 must see the pool: {} px",
        pool.len()
    );

    let plain_image = closet_capture(
        &workspace,
        scratch.path(),
        &plain,
        "pool-static-door",
        STATION_3,
    );
    let receiver_image = capture(
        &workspace,
        scratch.path(),
        &receiver,
        "pool-receiver",
        STATION_3,
        [CLOSET_WIDTH, CLOSET_HEIGHT],
        SceneExtras {
            force_promotion: Some((CENTRE_LIGHT_TAG, 1.0)),
            ..SceneExtras::default()
        },
    );
    let plain_pool = region_mean(&plain_image, &pool, luma);
    let receiver_pool = region_mean(&receiver_image, &pool, luma);
    eprintln!(
        "closet pool st3 ({} px): static door {plain_pool:.3}, kinematic receiver {receiver_pool:.3}",
        pool.len()
    );
    assert!(
        plain_pool <= DARK_POOL_CEILING,
        "the static-door pool must read dark: {plain_pool}"
    );
    assert!(
        receiver_pool <= plain_pool + RECEIVER_POOL_EPSILON,
        "promotion lit the closet pool: receiver {receiver_pool} against static door {plain_pool}"
    );
}

/// The line-of-sight fixture: the open-doorway map plus a tagged doorway light
/// that sees the pool and a slab, hidden from station 2, that promotes it.
const LOS_MAP: &str = "mover-closet-no-door-kinematic-receiver";
const DOORWAY_LIGHT_TAG: &str = "doorway_light";
/// Drift allowed between the promoted pool's mean summed RGB and the slab-free
/// pool's. The same tolerance (12, summed RGB) as `LIT_ENDPOINT_DRIFT` in
/// `assert_promoted_door_red_shadow_crossfade` (capture_frame.rs), which
/// bounds each pixel's difference; this one bounds the difference of the two
/// region means. Another test target cannot share the constant.
const LIT_ENDPOINT_DRIFT: f64 = 12.0;
/// The LOS fixture's receiver slab in map units, mover-closet.README.md
/// §Reference variants.
const LOS_SLAB_MIN: [f32; 3] = [560.0, 24.0, 0.0];
const LOS_SLAB_MAX: [f32; 3] = [608.0, 40.0, 8.0];
/// A bright-room view of the LOS slab from the doorway light's side. The
/// doorway light (320, 400, 64) stands west, north and above the slab, so it
/// lights the slab's west (x 560), north (y 40) and top (z 8) faces, which
/// face it with nothing between; the camera, also west, north and above, sees
/// exactly those three faces. The slab's centre is 457 map units from the
/// light, inside its 900-unit falloff. The doorway light is also the point
/// light nearest this pose (305 units; the nearest bright-room light is 344),
/// so it ranks first for the two cube slots, and the pin fails the capture if
/// it does not hold one.
const SLAB_VIEW: Pose = Pose {
    map: [480.0, 140.0, 56.0],
    yaw: 314.0,
    pitch: -19.0,
};
/// Fewest slab-box pixels that must differ between the w=0 and w=1 captures.
/// A mover reads a promoted light as `(1 - w)` × its baked direct SH (the
/// handoff atlas) plus `w` × the forward runtime term, so the two endpoints
/// shade the lit faces differently; a pin that never reached the GPU would
/// leave them byte-identical. Measured on a Radeon Pro 5300M: 4,197 of 6,669
/// slab-box px differ between w=0 and w=1; the floor is half that.
const MIN_SLAB_PIN_DIFFERENCES: usize = 2098;
/// Floor on the lit pool's mean luma at station 2: the doorway light's direct
/// term on the water. Measured on a Radeon Pro 5300M: 56.5 over 84,542 px
/// (the dark closet pool reads 0.000); the floor is under half of that.
const LIT_POOL_FLOOR: f64 = 25.0;

/// The LOS fixture with its `receiver_slab begin` .. `end` block cut, as a
/// self-deleting sibling map: the same lights, pool and fog, no mover.
fn slab_free_los_map(workspace: &Path) -> tempfile::TempPath {
    let maps = workspace.join("content/dev/maps");
    let source = fs::read_to_string(maps.join(format!("{LOS_MAP}.map"))).expect("read LOS map");
    let begin = source
        .find("// receiver_slab begin")
        .expect("LOS map marks its slab block");
    let end_marker = "// receiver_slab end\n";
    let end = source
        .find(end_marker)
        .expect("LOS map closes its slab block")
        + end_marker.len();
    let stripped = format!("{}{}", &source[..begin], &source[end..]);
    assert!(
        !stripped.contains("kinematic_mover"),
        "the slab-free reference must hold no mover"
    );
    let path = tempfile::Builder::new()
        .prefix(&format!(".{LOS_MAP}-no-slab-"))
        .suffix(".map")
        .tempfile_in(&maps)
        .expect("reserve slab-free map path")
        .into_temp_path();
    fs::write(&path, stripped).expect("write slab-free map");
    path
}

// Promotion leaves a lit pool exactly as baked. The doorway light lights the
// pool through the open doorway; the hidden slab promotes it. Pinning its w to
// 0 and to 1 must leave every pool pixel byte-identical, and both must match
// the same map without the slab, while the same pins visibly change the lit
// slab from a view that sees it. Forcing the light where it holds no slot
// fails the capture by its tag, so the pinned rows cannot pass vacuously.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_probe_depth_tiles -- --ignored`"]
fn promoted_doorway_light_leaves_the_lit_pool_unchanged() {
    let workspace = workspace_root();
    let scratch = tempfile::tempdir().expect("scratch dir");
    let fixture = compile_dev_map(&workspace, LOS_MAP);
    let slab_free_source = slab_free_los_map(&workspace);
    let slab_free_name = slab_free_source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("slab-free map stem")
        .to_owned();
    let reference = compile_dev_map(&workspace, &slab_free_name);
    let pool = pool_pixels(STATION_2);
    assert!(
        pool.len() >= MIN_POOL_PIXELS,
        "station 2 must see the pool: {} px",
        pool.len()
    );

    let pinned = |weight: f32| {
        capture(
            &workspace,
            scratch.path(),
            &fixture,
            &format!("los-w{weight}"),
            STATION_2,
            [CLOSET_WIDTH, CLOSET_HEIGHT],
            SceneExtras {
                force_promotion: Some((DOORWAY_LIGHT_TAG, weight)),
                ..SceneExtras::default()
            },
        )
    };
    let unpromoted = pinned(0.0);
    let promoted = pinned(1.0);
    let baseline = closet_capture(
        &workspace,
        scratch.path(),
        &reference,
        "los-no-slab",
        STATION_2,
    );

    let lit = region_mean(&baseline, &pool, luma);
    let promoted_rgb = region_mean(&promoted, &pool, rgb_sum);
    let baseline_rgb = region_mean(&baseline, &pool, rgb_sum);
    let identical = pool
        .iter()
        .filter(|&&(x, y)| unpromoted.get_pixel(x, y) == promoted.get_pixel(x, y))
        .count();
    eprintln!(
        "lit pool st2 ({} px): slab-free luma {lit:.3}; summed RGB w=1 {promoted_rgb:.3}, slab-free {baseline_rgb:.3}; w=0 and w=1 identical at {identical} px",
        pool.len()
    );
    assert!(
        lit >= LIT_POOL_FLOOR,
        "the doorway light must light the pool: {lit}"
    );
    assert_eq!(
        identical,
        pool.len(),
        "pinning the promoted light's w moved pool pixels"
    );
    assert!(
        (promoted_rgb - baseline_rgb).abs() <= LIT_ENDPOINT_DRIFT,
        "the promoted pool drifted from the slab-free pool: {promoted_rgb} against {baseline_rgb}"
    );

    // Positive control: the pin reaches the GPU. Station 2 sees no reader of
    // the pinned w (the slab is hidden and shadows nothing in view), so the
    // byte-identical pool above would also pass if the pin were dropped. The
    // slab is the reader: seen lit by the doorway light, it must shade
    // differently at w=0 and w=1.
    let slab_view = |weight: f32| {
        capture(
            &workspace,
            scratch.path(),
            &fixture,
            &format!("los-slab-w{weight}"),
            SLAB_VIEW,
            [CLOSET_WIDTH, CLOSET_HEIGHT],
            SceneExtras {
                force_promotion: Some((DOORWAY_LIGHT_TAG, weight)),
                ..SceneExtras::default()
            },
        )
    };
    let slab_unpromoted = slab_view(0.0);
    let slab_promoted = slab_view(1.0);
    let slab = SlabScreenBox::of_box(LOS_SLAB_MIN, LOS_SLAB_MAX, SLAB_VIEW);
    let (mut slab_pixels, mut slab_differences) = (0usize, 0usize);
    for (x, y, pixel) in slab_unpromoted.enumerate_pixels() {
        if !slab.contains(x, y) {
            continue;
        }
        slab_pixels += 1;
        if slab_promoted.get_pixel(x, y) != pixel {
            slab_differences += 1;
        }
    }
    eprintln!(
        "LOS slab view: {slab_differences} of {slab_pixels} slab-box px differ between w=0 and w=1"
    );
    assert!(slab_pixels > 0, "the slab view must see the slab");
    assert!(
        slab_differences >= MIN_SLAB_PIN_DIFFERENCES,
        "pinning the doorway light's w left the lit slab unchanged: {slab_differences} of {slab_pixels} px differ"
    );

    // Without the slab, nothing promotes the doorway light: the pinned row
    // must fail by name rather than render an unpromoted frame.
    let failure = try_capture(
        &workspace,
        scratch.path(),
        &reference,
        "los-no-slab-forced",
        STATION_2,
        [CLOSET_WIDTH, CLOSET_HEIGHT],
        SceneExtras {
            force_promotion: Some((DOORWAY_LIGHT_TAG, 1.0)),
            ..SceneExtras::default()
        },
    )
    .expect_err("forcing an unpromoted light must fail the capture");
    assert!(
        failure.contains(&format!("force_promotion tag `{DOORWAY_LIGHT_TAG}`"))
            && failure.contains("did not win a shadow-pool slot"),
        "the failure must name the tag and the missing slot:\n{failure}"
    );
}
