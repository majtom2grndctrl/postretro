//! The residency walks' camera projection, pitch and read-completion modes.
//! See: context/lib/rendering_pipeline.md §4

// Read once per process from `POSTRETRO_LIGHTMAP_WALK_*`; unset keeps the
// harness's original run: 100° 16:9, pitch 0, reads racing real time.

use glam::Vec3;

use crate::camera::RenderCamera;

const HFOV_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_HFOV";
const ASPECT_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_ASPECT";
const PITCH_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_PITCH";
const READS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_READS";
const RECORD_OUT_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_OUT";
const RECORD_IN_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_TRIP_RECORD_IN";
const PLANAR_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_PLANAR";

/// Default camera: `camera.rs` HFOV at 16:9.
const DEFAULT_HFOV_DEGREES: f32 = 100.0;
const DEFAULT_ASPECT: (f32, f32) = (16.0, 9.0);

/// Horizontal FOV is fixed; vertical follows the aspect, as the player
/// camera's does. So 130° 4:3 is the widest frustum, not 21:9.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Projection {
    pub(super) hfov_degrees: f32,
    pub(super) aspect: f32,
    /// The aspect as given, for labels.
    pub(super) aspect_label: (f32, f32),
}

impl Default for Projection {
    fn default() -> Self {
        Self {
            hfov_degrees: DEFAULT_HFOV_DEGREES,
            aspect: DEFAULT_ASPECT.0 / DEFAULT_ASPECT.1,
            aspect_label: DEFAULT_ASPECT,
        }
    }
}

impl Projection {
    pub(super) fn label(&self) -> String {
        format!(
            "HFOV {} deg at {}:{}",
            self.hfov_degrees, self.aspect_label.0, self.aspect_label.1
        )
    }
}

/// How a frame's reads meet its drains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ReadsMode {
    #[default]
    /// One drain per frame; a frame with reads in flight sleeps to its
    /// 16.7 ms deadline, so misses race disk reads. The pool mirror retires
    /// a grown-out generation per drain call.
    RealTime,
    /// Deterministic (O17, O29): before each of the frame's drains every read
    /// in flight completes, so reads issued in drain 0 land in drain 1. Reads
    /// held back by the 32-permit cap or the in-hand byte cap are issued in
    /// drain 1 and land next frame, or are dropped if the block has left
    /// demand. Misses are counted once, after the last drain, and the pool
    /// mirror retires a grown-out generation per frame.
    CompletedReads,
}

impl ReadsMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::RealTime => "real-time reads",
            Self::CompletedReads => "completed reads",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct WalkModes {
    pub(super) projection: Projection,
    /// Radians, positive looking up. Fixed for the run; yaw still turns.
    pub(super) pitch: f32,
    pub(super) pitch_degrees: f32,
    pub(super) reads: ReadsMode,
    /// Where this run writes its per-frame trip record.
    pub(super) record_out: Option<String>,
    /// A pre-change run's record this run is paired with.
    pub(super) record_in: Option<String>,
    /// Run the play frame's planar mirror step each frame, at the very high
    /// tier's preset, and stream main visible ∪ the mirror's drawable
    /// visible cells (`mirror`).
    pub(super) planar: bool,
}

impl WalkModes {
    pub(super) fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let mut modes = Self::default();
        if let Some(hfov) = var(HFOV_ENV) {
            let degrees: f32 = hfov.trim().parse().expect("HFOV degrees");
            assert!(
                degrees > 1.0 && degrees < 179.0,
                "{HFOV_ENV} must lie in (1, 179) degrees"
            );
            modes.projection.hfov_degrees = degrees;
        }
        if let Some(aspect) = var(ASPECT_ENV) {
            let (w, h) = aspect
                .split_once(':')
                .map(|(w, h)| (w.trim().parse::<f32>(), h.trim().parse::<f32>()))
                .and_then(|(w, h)| w.ok().zip(h.ok()))
                .filter(|&(w, h)| w > 0.0 && h > 0.0)
                .unwrap_or_else(|| panic!("{ASPECT_ENV} = W:H, e.g. 16:9 or 4:3"));
            modes.projection.aspect = w / h;
            modes.projection.aspect_label = (w, h);
        }
        if let Some(pitch) = var(PITCH_ENV) {
            let degrees: f32 = pitch.trim().parse().expect("pitch degrees");
            assert!(
                degrees.abs() < 89.0,
                "{PITCH_ENV} must lie in (-89, 89) degrees"
            );
            modes.pitch_degrees = degrees;
            modes.pitch = degrees.to_radians();
        }
        if let Some(reads) = var(READS_ENV) {
            modes.reads = match reads.trim() {
                "realtime" | "real-time" => ReadsMode::RealTime,
                "completed" => ReadsMode::CompletedReads,
                other => panic!("{READS_ENV} = realtime | completed, not {other:?}"),
            };
        }
        modes.record_out = var(RECORD_OUT_ENV);
        modes.record_in = var(RECORD_IN_ENV);
        if let Some(planar) = var(PLANAR_ENV) {
            modes.planar = match planar.trim() {
                "1" | "on" => true,
                "0" | "off" => false,
                other => panic!("{PLANAR_ENV} = 1 | on | 0 | off, not {other:?}"),
            };
        }
        modes
    }

    /// Projection, pitch and read mode: a paired run must match all three.
    /// Planar stays out, so a planar-on run pairs with the same walk's
    /// planar-off record: the bake-gap gate with planar on (§4).
    pub(super) fn label(&self) -> String {
        format!(
            "{}, pitch {} deg, {}",
            self.projection.label(),
            self.pitch_degrees,
            self.reads.label()
        )
    }
}

/// The eye looking along `yaw` (0 faces -Z) tilted by `pitch`, as the render
/// camera the walk and the planar mirror step read: view and projection apart,
/// as the play frame hands them over.
pub(super) fn render_camera(
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    projection: Projection,
) -> RenderCamera {
    let (up, level) = pitch.sin_cos();
    let look = Vec3::new(-yaw.sin() * level, up, -yaw.cos() * level);
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + look, Vec3::Y);
    let aspect = projection.aspect;
    let vfov = 2.0 * ((projection.hfov_degrees.to_radians() / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    RenderCamera {
        eye_position: eye,
        forward: look,
        view_matrix: view,
        view_projection: proj * view,
    }
}
