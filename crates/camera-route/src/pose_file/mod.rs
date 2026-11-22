// The camera-route pose file (`postretro-camera-route 1`): writer and strict parser.
// See: context/lib/rendering_pipeline.md §12

// Text, one record per line. Floats are written `{}` (shortest round-trip),
// so a parse is bit-equal to what was written; angles are engine radians and
// positions are eye points in world metres. Header lines appear once, in
// order: origin, params, projection, bake, cadence_hz, then an optional
// spawn, then one or more poses.
//
// Pose fields by origin:
// - `walk <step> <cell>` on a pose: in an exact file (far-point, random) it is
//   the walk's own step and cell. In a smoothed file it is the walk step and
//   the cell the eye locates in, which can differ from the walk's cell on a
//   detour.
// - `cut` marks a pose reached by a jump that interpolation never crosses.
//   Exact routes never mark cuts. A smoothed file marks `cut` where the
//   walk teleports and also on a locate miss: where no proven line reaches
//   the walk's next cell, or a pose's eye locates outside the walk's current
//   and next cell.
// - The `params` line records the generator's options in the user's units
//   (`pitch` and `hfov` in degrees, `speed` in m/s, `turn_cap` in degrees per
//   second). The pose lines hold engine radians.
// - `bake` carries `id51` as 16 lowercase hex digits or `none`, and
//   `walk_epoch` as plain decimal: exactly the form `bake_identity` writes,
//   since the engine compares the tokens as raw strings.

use std::fmt::Write as _;

use glam::Vec3;
use thiserror::Error;

use crate::bake::bake_token;
use crate::exact::RoutePose;
use crate::path::WalkKind;

mod parse;

pub use parse::parse_pose_file;

pub const POSE_FILE_MAGIC: &str = "postretro-camera-route 1";
/// The magic line up to the version, to tell an unsupported version from a
/// foreign file.
const POSE_FILE_MAGIC_PREFIX: &str = "postretro-camera-route ";

/// The header lines every file carries, in order.
const HEADER_LINES: [&str; 5] = ["origin", "params", "projection", "bake", "cadence_hz"];

/// How a generated route was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedKind {
    FarPoint,
    Random,
    Smoothed,
}

impl GeneratedKind {
    pub const ALL: [GeneratedKind; 3] = [Self::FarPoint, Self::Random, Self::Smoothed];

    pub fn key(self) -> &'static str {
        match self {
            Self::FarPoint => "far-point",
            Self::Random => "random",
            Self::Smoothed => "smoothed",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// The exact kind that replays `walk` frame for frame.
    pub fn exact(walk: WalkKind) -> Self {
        match walk {
            WalkKind::FarPointTour => Self::FarPoint,
            WalkKind::Random => Self::Random,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOrigin {
    Generated(GeneratedKind),
    Recorded,
}

/// The projection the poses were made for: horizontal FOV fixed, vertical
/// following the aspect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub hfov_degrees: f32,
    pub aspect: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteHeader {
    pub origin: RouteOrigin,
    /// How the generator was invoked, in written order, in the user's units:
    /// `pitch` and `hfov` in degrees, unlike the pose lines (radians). Keys
    /// and values are single tokens without `=` in the key.
    pub params: Vec<(String, String)>,
    pub projection: Projection,
    /// `bake_identity`'s string.
    pub bake: String,
    /// Poses per second of route time.
    pub cadence_hz: u32,
    /// The walk's spawn eye, on exact kinds.
    pub spawn: Option<Vec3>,
}

impl RouteHeader {
    /// The id-51 hash the poses were made on, `none` when the level had none.
    pub fn id51(&self) -> Option<&str> {
        bake_token(&self.bake, "id51")
    }

    pub fn walk_epoch(&self) -> Option<u32> {
        bake_token(&self.bake, "walk_epoch")?.parse().ok()
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find_map(|(k, v)| (k == key).then_some(v.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoseFile {
    pub header: RouteHeader,
    pub poses: Vec<RoutePose>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("line {line}: {reason}")]
pub struct PoseFileError {
    /// 1-based; one past the last line when something is missing at the end.
    pub line: usize,
    pub reason: ParseReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseReason {
    #[error("the file is empty")]
    Empty,
    #[error("unsupported version {version}: this build reads `{POSE_FILE_MAGIC}`")]
    UnsupportedVersion { version: String },
    #[error("not a camera-route pose file: expected `{POSE_FILE_MAGIC}`, found {found:?}")]
    NotAPoseFile { found: String },
    #[error("expected the `{expected}` line, found `{found}`")]
    OutOfOrder {
        expected: &'static str,
        found: String,
    },
    #[error("missing the `{expected}` line")]
    Missing { expected: &'static str },
    #[error("unknown line {0:?}")]
    UnknownLine(String),
    #[error("blank line")]
    BlankLine,
    #[error("malformed `{line}` line: {why}")]
    Malformed { line: &'static str, why: String },
    #[error("bad float {text:?} for `{field}`")]
    BadFloat { field: &'static str, text: String },
    #[error("bad integer {text:?} for `{field}`")]
    BadInteger { field: &'static str, text: String },
    #[error("no `pose` lines after the header")]
    NoPoses,
}

/// The file's text. Every float is written shortest round-trip.
pub fn format_pose_file(file: &PoseFile) -> String {
    let header = &file.header;
    let mut text = String::with_capacity(256 + file.poses.len() * 64);
    let mut line = |args: std::fmt::Arguments<'_>| {
        text.write_fmt(args).expect("string write");
        text.push('\n');
    };
    line(format_args!("{POSE_FILE_MAGIC}"));
    match header.origin {
        RouteOrigin::Generated(kind) => line(format_args!("origin generated {}", kind.key())),
        RouteOrigin::Recorded => line(format_args!("origin recorded")),
    }
    let mut params = String::from("params");
    for (key, value) in &header.params {
        debug_assert!(
            !key.is_empty() && !key.contains(['=', ' ', '\n']) && !value.contains([' ', '\n']),
            "pose-file params are single tokens: {key:?}={value:?}"
        );
        write!(params, " {key}={value}").expect("string write");
    }
    line(format_args!("{params}"));
    line(format_args!(
        "projection hfov_degrees={} aspect={}",
        header.projection.hfov_degrees, header.projection.aspect
    ));
    debug_assert!(!header.bake.contains('\n'), "the bake identity is one line");
    line(format_args!("bake {}", header.bake));
    line(format_args!("cadence_hz {}", header.cadence_hz));
    if let Some(spawn) = header.spawn {
        line(format_args!("spawn {} {} {}", spawn.x, spawn.y, spawn.z));
    }
    for pose in &file.poses {
        let mut record = format!(
            "pose {} {} {} {} {}",
            pose.eye.x, pose.eye.y, pose.eye.z, pose.yaw, pose.pitch
        );
        if let Some((step, cell)) = pose.walk {
            write!(record, " walk {step} {cell}").expect("string write");
        }
        if pose.cut {
            record.push_str(" cut");
        }
        line(format_args!("{record}"));
    }
    text
}
