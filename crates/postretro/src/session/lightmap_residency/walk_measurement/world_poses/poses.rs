//! World-space measurement poses: the standing-eye grid, the frame-cost
//! import, and the pose file both builds read.
//! See: context/lib/rendering_pipeline.md §2

use std::fmt::Write as _;

use glam::Vec3;
use postretro_level_loader::LevelWorld;
use postretro_physics::collision::{CollisionWorld, cast_ray};

use super::{interior_bounds, is_interior};

const MAGIC: &str = "postretro-world-poses 1";

/// Eye height above the floor: the player capsule's centre 1.0 m up plus
/// the 0.5 m eye offset (`player.ts`).
pub(super) const EYE_HEIGHT: f32 = 1.5;
/// `--start-pose` and `frame-cost/poses.txt` give the capsule centre.
const CAPSULE_CENTRE_TO_EYE: f32 = 0.5;
/// Headings per grid pose, engine degrees (0 faces -Z).
pub(super) const GRID_YAWS_DEGREES: [f32; 4] = [0.0, 90.0, 180.0, 270.0];
/// Grid columns the default spacing aims for over the interior footprint.
pub(super) const DEFAULT_MAX_COLUMNS: usize = 600;
/// The grid never gets finer than this, metres.
const MIN_SPACING: f32 = 1.0;
/// Spacing rounds up to a multiple of this, metres, so it reads cleanly.
const SPACING_STEP: f32 = 0.25;
/// How far above and below a floor hit the locator is asked, metres.
const FLOOR_PROBE: f32 = 0.05;
/// A column stops after this many surfaces.
const MAX_SURFACES_PER_COLUMN: usize = 64;
/// The standing capsule's radius (`player.ts`): no wall comes nearer the
/// eye than a player could stand, metres.
const PLAYER_RADIUS: f32 = 0.2;
/// Horizontal clearance rays around the eye.
const CLEARANCE_RAYS: usize = 8;

/// One eye pose in world space: the only thing two builds share.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WorldPose {
    /// No whitespace; unique within a set.
    pub(super) label: String,
    pub(super) eye: Vec3,
    /// Engine convention: 0 faces -Z, positive turns toward -X.
    pub(super) yaw_degrees: f32,
    /// Positive looks up.
    pub(super) pitch_degrees: f32,
}

impl WorldPose {
    pub(super) fn yaw(&self) -> f32 {
        self.yaw_degrees.to_radians()
    }

    pub(super) fn pitch(&self) -> f32 {
        self.pitch_degrees.to_radians()
    }

    /// Same world pose, bit for bit.
    pub(super) fn same_pose(&self, other: &WorldPose) -> bool {
        self.eye.to_array().map(f32::to_bits) == other.eye.to_array().map(f32::to_bits)
            && self.yaw_degrees.to_bits() == other.yaw_degrees.to_bits()
            && self.pitch_degrees.to_bits() == other.pitch_degrees.to_bits()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct PoseSet {
    /// How the set was made: `key=value` notes, one per line.
    pub(super) notes: Vec<String>,
    pub(super) poses: Vec<WorldPose>,
}

/// Standing eyes over every floor of a column grid covering the interior
/// cells' footprint. A floor is a surface a downward ray meets with interior
/// space just above and solid just below; its eye stands `EYE_HEIGHT` above
/// it, located interior, with no geometry between floor and eye nor within
/// a player's radius of the eye.
pub(super) fn grid_poses(world: &LevelWorld, max_columns: usize) -> PoseSet {
    let collision = CollisionWorld::from_level(world).expect("static collision mesh");
    let Some((lo, hi)) = interior_bounds(world) else {
        return PoseSet::default();
    };
    let area = (hi.x - lo.x).max(0.0) * (hi.z - lo.z).max(0.0);
    let spacing = grid_spacing(area, max_columns);
    let columns_x = ((hi.x - lo.x) / spacing).ceil().max(1.0) as usize;
    let columns_z = ((hi.z - lo.z) / spacing).ceil().max(1.0) as usize;
    let top = hi.y + 1.0;
    let mut poses = Vec::new();
    let mut floors = 0usize;
    for iz in 0..columns_z {
        for ix in 0..columns_x {
            let x = lo.x + spacing * (ix as f32 + 0.5);
            let z = lo.z + spacing * (iz as f32 + 0.5);
            for (level, floor) in column_floors(world, &collision, x, z, top, hi.y - lo.y + 2.0)
                .into_iter()
                .enumerate()
            {
                floors += 1;
                for yaw in GRID_YAWS_DEGREES {
                    poses.push(WorldPose {
                        label: format!("g{ix}.{iz}.{level}@{yaw}"),
                        eye: floor + Vec3::Y * EYE_HEIGHT,
                        yaw_degrees: yaw,
                        pitch_degrees: 0.0,
                    });
                }
            }
        }
    }
    PoseSet {
        notes: vec![format!(
            "grid spacing={spacing} m columns={columns_x}x{columns_z} (cap {max_columns}) \
             floors={floors} eye={EYE_HEIGHT} m yaws={GRID_YAWS_DEGREES:?} pitch=0 \
             footprint=[{}, {}]..[{}, {}]",
            lo.x, lo.z, hi.x, hi.z
        )],
        poses,
    }
}

/// The coarsest of `MIN_SPACING` and the spacing that puts about
/// `max_columns` columns over `area`, rounded up to `SPACING_STEP`.
pub(super) fn grid_spacing(area: f32, max_columns: usize) -> f32 {
    let raw = (area / max_columns.max(1) as f32).sqrt().max(MIN_SPACING);
    (raw / SPACING_STEP).ceil() * SPACING_STEP
}

/// Every standing floor point in the column at (`x`, `z`), top down.
fn column_floors(
    world: &LevelWorld,
    collision: &CollisionWorld,
    x: f32,
    z: f32,
    top: f32,
    depth: f32,
) -> Vec<Vec3> {
    let mut floors = Vec::new();
    let mut origin = Vec3::new(x, top, z);
    let mut left = depth;
    for _ in 0..MAX_SURFACES_PER_COLUMN {
        let Some(hit) = cast_ray(collision, origin, Vec3::NEG_Y, left) else {
            break;
        };
        let point = origin - Vec3::Y * hit.time_of_impact;
        let above = world.locate_cell(point + Vec3::Y * FLOOR_PROBE);
        let below = world.locate_cell(point - Vec3::Y * FLOOR_PROBE);
        let eye = point + Vec3::Y * EYE_HEIGHT;
        let standing = is_interior(world, above)
            && world.cells.get(below).is_some_and(|cell| cell.is_solid)
            && is_interior(world, world.locate_cell(eye))
            && cast_ray(
                collision,
                point + Vec3::Y * FLOOR_PROBE,
                Vec3::Y,
                EYE_HEIGHT - FLOOR_PROBE,
            )
            .is_none()
            && clear_around(collision, eye);
        if standing {
            floors.push(point);
        }
        // Step just past the surface and keep going down.
        let advance = hit.time_of_impact + 1.0e-3;
        origin -= Vec3::Y * advance;
        left -= advance;
        if left <= 0.0 {
            break;
        }
    }
    floors
}

/// No geometry within `PLAYER_RADIUS` of `eye` horizontally.
fn clear_around(collision: &CollisionWorld, eye: Vec3) -> bool {
    (0..CLEARANCE_RAYS).all(|k| {
        let angle = k as f32 * std::f32::consts::TAU / CLEARANCE_RAYS as f32;
        let direction = Vec3::new(angle.cos(), 0.0, angle.sin());
        cast_ray(collision, eye, direction, PLAYER_RADIUS).is_none()
    })
}

/// Reads a `frame-cost/poses.txt`-style list: `label x,y,z,yaw_deg,pitch_deg`
/// per line, y the pawn's capsule centre as `--start-pose` takes it.
pub(super) fn import_frame_cost(path: &str) -> Result<Vec<WorldPose>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let bad = || format!("{path}: expected `label x,y,z,yaw,pitch`, got {line:?}");
            let (label, values) = line.split_once(char::is_whitespace).ok_or_else(bad)?;
            let values: Vec<f32> = values
                .trim()
                .split(',')
                .map(|v| v.trim().parse::<f32>().map_err(|_| bad()))
                .collect::<Result<_, _>>()?;
            let [x, y, z, yaw, pitch] = values[..] else {
                return Err(bad());
            };
            Ok(WorldPose {
                label: format!("fc:{label}"),
                eye: Vec3::new(x, y + CAPSULE_CENTRE_TO_EYE, z),
                yaw_degrees: yaw,
                pitch_degrees: pitch,
            })
        })
        .collect()
}

pub(super) fn format_poses(set: &PoseSet) -> String {
    let mut text = String::new();
    writeln!(text, "{MAGIC}").expect("string write");
    for note in &set.notes {
        writeln!(text, "# {note}").expect("string write");
    }
    for pose in &set.poses {
        writeln!(text, "pose {}", format_pose(pose)).expect("string write");
    }
    text
}

/// `label x y z yaw pitch`, each float in its shortest exact form.
pub(super) fn format_pose(pose: &WorldPose) -> String {
    format!(
        "{} {} {} {} {} {}",
        pose.label, pose.eye.x, pose.eye.y, pose.eye.z, pose.yaw_degrees, pose.pitch_degrees
    )
}

/// Parses `format_pose`'s six fields from `parts`.
pub(super) fn parse_pose<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<WorldPose> {
    let label = parts.next()?.to_string();
    let mut float = || parts.next()?.parse::<f32>().ok();
    let (x, y, z) = (float()?, float()?, float()?);
    let (yaw_degrees, pitch_degrees) = (float()?, float()?);
    Some(WorldPose {
        label,
        eye: Vec3::new(x, y, z),
        yaw_degrees,
        pitch_degrees,
    })
}

pub(super) fn parse_poses(text: &str) -> Result<PoseSet, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MAGIC) {
        return Err("not a world pose file".to_string());
    }
    let mut set = PoseSet::default();
    for line in lines {
        if let Some(note) = line.strip_prefix("# ") {
            set.notes.push(note.to_string());
        } else if let Some(rest) = line.strip_prefix("pose ") {
            let pose = parse_pose(&mut rest.split_ascii_whitespace())
                .ok_or_else(|| format!("bad pose line {line:?}"))?;
            set.poses.push(pose);
        } else if !line.trim().is_empty() {
            return Err(format!("unexpected line {line:?}"));
        }
    }
    Ok(set)
}

pub(super) fn write_poses(path: &str, set: &PoseSet) -> std::io::Result<()> {
    std::fs::write(path, format_poses(set))
}

pub(super) fn read_poses(path: &str) -> Result<PoseSet, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse_poses(&text).map_err(|why| format!("{path}: {why}"))
}
