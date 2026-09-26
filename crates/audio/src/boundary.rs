// Primitive-typed boundary types the frame loop exchanges with the audio
// subsystem: listener pose, sound requests and their anchors, attenuation,
// init errors, and bus-name parsing.
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
    /// Entity key of the pawn the listener is attached to, if any. A sound
    /// anchored on this key plays non-spatial. Keys are the caller's opaque
    /// entity identities, the same ones [`SoundAnchor::Entity`] carries.
    pub attached: Option<u64>,
}

/// A request to play a sound, crossing the boundary as primitives only.
/// The target bus and sound are named keys resolved inside the subsystem.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundRequest {
    /// Mixer bus to route this sound to (e.g. "sfx", "music", "ui").
    pub bus: String,
    /// Registry key of the sound asset to play.
    pub sound: String,
    /// Whether the sound loops until stopped. Anchored requests are one-shots.
    pub looping: bool,
    /// Where the sound plays from. `None` plays it unpositioned on its bus;
    /// `Some` spatializes it on the SFX bus (see `audio.md` §5).
    pub anchor: Option<SoundAnchor>,
}

/// Where a positioned sound plays from. Every variant carries a fire-time
/// point, so an emitter gone before the audio step still has a position.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundAnchor {
    /// Follows an entity each frame through the caller's position resolver,
    /// starting from `point`. Freezes at its last position once the resolver
    /// stops answering for `key`.
    Entity { key: u64, point: [f32; 3] },
    /// A fixed world point.
    Point([f32; 3]),
    /// An impact's contact points. The sound plays at the contact nearest the
    /// listener of the frame it starts in.
    Contacts(Vec<[f32; 3]>),
}

/// How a positioned sound falls off with distance. Captured when a sound
/// starts, so a later change affects only sounds started after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attenuation {
    /// Distance at and below which a sound plays at full level.
    pub min_distance: f32,
    /// Distance at and beyond which a sound is silent.
    pub max_distance: f32,
    /// Falloff shape between the two distances.
    pub curve: AttenuationCurve,
}

/// Falloff shape between an attenuation's minimum and maximum distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttenuationCurve {
    Linear,
    Quadratic,
}

impl Attenuation {
    /// Engine-seeded default, in engine units (metres). Close fights stay at
    /// full level; the long sightlines of the dev arenas fade out before the
    /// far side of a map.
    pub const DEFAULT: Self = Self {
        min_distance: 2.0,
        max_distance: 60.0,
        curve: AttenuationCurve::Linear,
    };

    /// Finite, non-negative distances with the minimum strictly below the
    /// maximum. Callers validate authored values against this before use.
    pub fn is_valid(&self) -> bool {
        self.min_distance.is_finite()
            && self.max_distance.is_finite()
            && self.min_distance >= 0.0
            && self.min_distance < self.max_distance
    }
}

impl Default for Attenuation {
    fn default() -> Self {
        Self::DEFAULT
    }
}
