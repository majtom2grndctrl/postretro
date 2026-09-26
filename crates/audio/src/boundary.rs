// Primitive-typed boundary types the frame loop exchanges with the audio
// subsystem: listener pose, sound requests, init errors, and bus-name parsing.
// See: context/lib/audio.md §1

use crate::BusId;

/// Map a boundary bus name to its [`BusId`]. Case-insensitive; the accepted
/// names mirror the registry collection convention (`sfx`, `music`, `ui`).
/// Unknown names return `None` so `play` can warn-and-drop.
pub(crate) fn parse_bus(name: &str) -> Option<BusId> {
    match name.to_ascii_lowercase().as_str() {
        "sfx" => Some(BusId::Sfx),
        "music" => Some(BusId::Music),
        "ui" => Some(BusId::UI),
        _ => None,
    }
}

/// Failures from audio init or asset loading. Init failure is non-fatal: the
/// caller logs and runs the game silent (`Audio` stays `None`). The kira
/// backend error is captured as a string so the backend type never leaks
/// across this boundary.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The kira backend (cpal device/stream) failed to start.
    #[error("audio backend init failed: {0}")]
    Init(String),
}

/// Listener pose handed across the subsystem boundary each frame. Primitives
/// only — the glam-typed `Camera` is converted at the call site, not here.
/// `up` is world up `[0.0, 1.0, 0.0]`; `Camera` has no up accessor and uses
/// `Vec3::Y` internally.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerState {
    /// World-space listener position.
    pub position: [f32; 3],
    /// Normalized forward (look) direction.
    pub forward: [f32; 3],
    /// Normalized world up, always `[0.0, 1.0, 0.0]`.
    pub up: [f32; 3],
}

/// A request to play a sound, crossing the boundary as primitives only.
/// The target bus and sound are named keys resolved inside the subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundRequest {
    /// Mixer bus to route this sound to (e.g. "sfx", "music", "ui").
    pub bus: String,
    /// Registry key of the sound asset to play.
    pub sound: String,
    /// Whether the sound loops until stopped.
    pub looping: bool,
}
