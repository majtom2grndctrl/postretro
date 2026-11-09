// Plumbing for the mover probe channel's capture proofs: capture scenes with
// forced mover travel, PRL section rewrites (strip id 57, or only its depth
// block), generated map variants (a mover made static at a waypoint, an added
// entity), and the screen-space measures the rows read.
// See: context/lib/rendering_pipeline.md §7.8 · §4 (Mover state lighting)

use std::fs;
use std::path::Path;

use image::RgbaImage;
use postretro_level_format as prl_format;
use postretro_level_format::SectionId;
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;

use super::capture_support::{capture_command, load_capture_rgba};

/// TrenchBroom unit in metres.
pub const UNIT: f32 = 0.0254;

/// A capture pose in map units, yaw and pitch in degrees.
#[derive(Clone, Copy)]
pub struct Pose {
    pub map: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

/// Capture-scene fields beyond the map and pose.
#[derive(Clone, Default)]
pub struct Scene {
    pub resolution: [u32; 2],
    /// `force_movers` entries: mover name and travel fraction.
    pub force_movers: Vec<(&'static str, f32)>,
    /// `force_promotion` entries: light tag and pinned `w`.
    pub force_promotion: Vec<(&'static str, f32)>,
    /// Reflections off (`reflection_quality` "off"); the closet rows judge
    /// the closet without the pool's mover-blind reflection probe.
    pub reflections_off: bool,
    pub light_term_mask: Option<u32>,
    /// `POSTRETRO_SH_STREAMING` for the child; `sync-proof` when `None`.
    pub streaming: Option<&'static str>,
}

/// Run one capture of `map` at `pose`, panicking with the child's output on
/// failure. Copies the PNG into `MOVER_CHANNEL_CAPTURE_DIR` as `<label>.png`
/// when that variable is set, so a run can keep its stills.
pub fn capture(
    workspace: &Path,
    scratch: &Path,
    map: &Path,
    label: &str,
    pose: Pose,
    scene: &Scene,
) -> RgbaImage {
    let scene_path = scratch.join(format!("{label}.scene.json"));
    let output_path = scratch.join(format!("{label}.png"));
    let [x, y, z] = pose.map;
    let mut json = serde_json::json!({
        "map": map.display().to_string(),
        "camera": {
            "position": [-y * UNIT, z * UNIT, -x * UNIT],
            "yaw_deg": pose.yaw,
            "pitch_deg": pose.pitch,
            "fov_deg": pose.fov
        },
        "resolution": scene.resolution,
        "output": output_path.display().to_string(),
        "force_full_resident_sh_compose": true
    });
    if !scene.force_movers.is_empty() {
        json["force_movers"] = scene
            .force_movers
            .iter()
            .map(|(name, t)| serde_json::json!({ "name": name, "t": t }))
            .collect();
    }
    if !scene.force_promotion.is_empty() {
        json["force_promotion"] = scene
            .force_promotion
            .iter()
            .map(|(tag, weight)| serde_json::json!({ "tag": tag, "weight": weight }))
            .collect();
    }
    if scene.reflections_off {
        json["reflection_quality"] = "off".into();
    }
    if let Some(mask) = scene.light_term_mask {
        json["light_term_mask"] = mask.into();
    }
    fs::write(&scene_path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    let mut command = capture_command(workspace, &scene_path);
    if let Some(mode) = scene.streaming {
        command.env("POSTRETRO_SH_STREAMING", mode);
    }
    let result = command
        .env("RUST_LOG", "warn")
        .output()
        .expect("launch postretro capture");
    assert!(
        result.status.success(),
        "capture `{label}` failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    if let Some(dir) = std::env::var_os("MOVER_CHANNEL_CAPTURE_DIR") {
        fs::copy(&output_path, Path::new(&dir).join(format!("{label}.png")))
            .expect("keep the capture still");
    }
    load_capture_rgba(&output_path)
}

pub fn luma([r, g, b, _]: [u8; 4]) -> f64 {
    0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
}

pub fn rgb_sum([r, g, b, _]: [u8; 4]) -> f64 {
    f64::from(r) + f64::from(g) + f64::from(b)
}

pub fn frame_mean(image: &RgbaImage) -> f64 {
    image.pixels().map(|p| luma(p.0)).sum::<f64>() / f64::from(image.width() * image.height())
}

/// Mean of `measure` over the pixels `keep` admits; `None` when it admits none.
pub fn masked_mean(
    image: &RgbaImage,
    keep: impl Fn(u32, u32) -> bool,
    measure: fn([u8; 4]) -> f64,
) -> Option<f64> {
    let (mut sum, mut count) = (0.0, 0usize);
    for (x, y, pixel) in image.enumerate_pixels() {
        if keep(x, y) {
            sum += measure(pixel.0);
            count += 1;
        }
    }
    (count > 0).then(|| sum / count as f64)
}

/// The largest per-channel difference between two frames, alpha excluded.
pub fn max_channel_delta(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..3).map(move |c| p.0[c].abs_diff(q.0[c])))
        .max()
        .unwrap_or(0)
}

/// Pixel of a map-unit point seen from `pose` at `resolution`; `None` behind
/// the camera.
pub fn project(point: [f32; 3], pose: Pose, resolution: [u32; 2]) -> Option<[f32; 2]> {
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
    let half_x = (pose.fov.to_radians() / 2.0).tan();
    let half_y = half_x * h / w;
    Some([
        w / 2.0 * (1.0 + dot(right) / depth / half_x),
        h / 2.0 * (1.0 - dot(up) / depth / half_y),
    ])
}

/// Screen bounding box of a map-unit box's projected corners at a pose.
pub struct ScreenBox {
    lo: [f32; 2],
    hi: [f32; 2],
}

impl ScreenBox {
    /// Every corner must be in front of the camera.
    pub fn of(min: [f32; 3], max: [f32; 3], pose: Pose, resolution: [u32; 2]) -> Self {
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        for x in [min[0], max[0]] {
            for y in [min[1], max[1]] {
                for z in [min[2], max[2]] {
                    let p = project([x, y, z], pose, resolution).expect("box corner in front");
                    lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                    hi = [hi[0].max(p[0]), hi[1].max(p[1])];
                }
            }
        }
        Self { lo, hi }
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        fx >= self.lo[0] && fx <= self.hi[0] && fy >= self.lo[1] && fy <= self.hi[1]
    }
}

pub fn read_sections(path: &Path) -> Vec<prl_format::SectionBlob> {
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

/// A PRL path beside the compiled fixtures, so capture still derives its
/// materials root from content/dev/maps. Deletes itself on drop.
pub fn sibling_prl(workspace: &Path, label: &str) -> tempfile::TempPath {
    tempfile::Builder::new()
        .prefix(&format!(".{label}-"))
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve variant PRL path")
        .into_temp_path()
}

/// `prl` rewritten beside the fixtures with id 57 dropped (`depth_only`
/// false) or with only its depth block dropped (`true`). Test-only: the
/// compiler has no switch for either. Every other section keeps its bytes,
/// so a map whose id 59 was trimmed for mover depth keeps that trim.
pub fn without_channel(
    workspace: &Path,
    prl: &Path,
    label: &str,
    depth_only: bool,
) -> tempfile::TempPath {
    let sections = read_sections(prl);
    let channel_id = SectionId::MoverProbeChannel as u32;
    assert!(
        sections.iter().any(|blob| blob.section_id == channel_id),
        "{label}: the fixture ships id 57"
    );
    let blobs: Vec<_> = sections
        .into_iter()
        .filter_map(|blob| {
            if blob.section_id != channel_id {
                return Some(blob);
            }
            if !depth_only {
                return None;
            }
            let mut channel =
                MoverProbeChannelSection::from_bytes(&blob.data).expect("id 57 decodes");
            assert!(!channel.depth_records.is_empty(), "{label}: a depth block");
            channel.depth_records.clear();
            channel.depth_tiles.clear();
            Some(prl_format::SectionBlob {
                data: channel.to_bytes(),
                ..blob
            })
        })
        .collect();
    let path = sibling_prl(workspace, label);
    let mut file = fs::File::create(&path).expect("create variant PRL");
    prl_format::write_prl(&mut file, &blobs).expect("write variant PRL");
    path
}

/// A generated map beside the fixtures, compiled by its file stem.
/// Deletes itself on drop.
pub fn sibling_map(workspace: &Path, label: &str, source: &str) -> tempfile::TempPath {
    let path = tempfile::Builder::new()
        .prefix(&format!(".{label}-"))
        .suffix(".map")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve generated map path")
        .into_temp_path();
    fs::write(&path, source).expect("write generated map");
    path
}

pub fn read_map(workspace: &Path, name: &str) -> String {
    fs::read_to_string(workspace.join(format!("content/dev/maps/{name}.map")))
        .unwrap_or_else(|err| panic!("read {name}.map: {err}"))
}

/// `map` with one more entity appended.
pub fn with_entity(map: &str, entity: &str) -> String {
    format!("{}\n{}\n", map.trim_end(), entity.trim())
}

/// Top-level `{ … }` entity blocks of a map, comments dropped.
fn entities(map: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for line in map.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "{" {
            depth += 1;
        }
        if depth > 0 {
            current.push_str(line);
            current.push('\n');
        }
        if trimmed == "}" {
            depth -= 1;
            if depth == 0 {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    out
}

fn key_of(entity: &str, key: &str) -> Option<String> {
    let prefix = format!("\"{key}\" \"");
    entity.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_string)
    })
}

/// `map` with kinematic mover `name` made static at its waypoint
/// `waypoint`: its brushes join the worldspawn moved by that waypoint's
/// offset from the path's first, and the mover and its waypoints leave the
/// map. Mirrors level-compiler's `tests/mover_bounce_support` `staticize`;
/// another crate's test support cannot be shared.
pub fn staticize(map: &str, name: &str, waypoint: &str) -> String {
    let blocks = entities(map);
    let mover = blocks
        .iter()
        .find(|block| {
            key_of(block, "classname").as_deref() == Some("kinematic_mover")
                && key_of(block, "name").as_deref() == Some(name)
        })
        .unwrap_or_else(|| panic!("no kinematic_mover `{name}`"));
    let first = key_of(mover, "path").expect("a mover path");
    let origin = |waypoint: &str| -> [f64; 3] {
        let block = blocks
            .iter()
            .find(|block| key_of(block, "name").as_deref() == Some(waypoint))
            .unwrap_or_else(|| panic!("no waypoint `{waypoint}`"));
        let values: Vec<f64> = key_of(block, "origin")
            .expect("a waypoint origin")
            .split_whitespace()
            .map(|value| value.parse().expect("a coordinate"))
            .collect();
        [values[0], values[1], values[2]]
    };
    let (from, to) = (origin(&first), origin(waypoint));
    let offset = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let inner = mover
        .trim()
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .expect("an entity block");
    let moved: Vec<String> = entities(inner)
        .iter()
        .map(|brush| translate_brush(brush, offset))
        .collect();
    let mut waypoints = vec![first];
    while let Some(next) = blocks
        .iter()
        .find(|block| key_of(block, "name").as_deref() == waypoints.last().map(String::as_str))
        .and_then(|block| key_of(block, "next"))
        .filter(|next| !next.is_empty() && !waypoints.contains(next))
    {
        waypoints.push(next);
    }
    let mut out = Vec::new();
    for block in &blocks {
        let classname = key_of(block, "classname");
        if classname.as_deref() == Some("worldspawn") {
            let body = block.trim().strip_suffix('}').expect("a closed worldspawn");
            out.push(format!("{body}{}\n}}", moved.join("\n")));
            continue;
        }
        let is_mover = std::ptr::eq(block, mover);
        let is_waypoint = key_of(block, "name").is_some_and(|n| waypoints.contains(&n))
            && classname.as_deref() == Some("kinematic_waypoint");
        let carried = key_of(block, "carrier").as_deref() == Some(name);
        if !is_mover && !is_waypoint && !carried {
            out.push(block.clone());
        }
    }
    out.join("\n") + "\n"
}

/// One brush translated by `offset` (map units): every plane point moves.
fn translate_brush(brush: &str, offset: [f64; 3]) -> String {
    brush
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with('(') {
                return line.to_string();
            }
            let mut out = String::new();
            let mut rest = trimmed;
            for _ in 0..3 {
                let open = rest.find('(').expect("a plane point");
                let close = rest.find(')').expect("a closed plane point");
                let point: Vec<f64> = rest[open + 1..close]
                    .split_whitespace()
                    .map(|value| value.parse().expect("a coordinate"))
                    .collect();
                out.push_str(&format!(
                    "( {} {} {} ) ",
                    point[0] + offset[0],
                    point[1] + offset[1],
                    point[2] + offset[2]
                ));
                rest = &rest[close + 1..];
            }
            out.push_str(rest.trim());
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}
