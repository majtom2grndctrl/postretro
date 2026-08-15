//! VM-free payloads shared by presentation producers and the registry intake.

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Hard ceiling for registry-side spawn intake between render frames. It is
/// intentionally separate from the smaller live-pool budget: a same-frame
/// burst may still reach the pool's deterministic eviction policy, but neither
/// side of the bridge can grow without bound.
pub const MAX_PENDING_PRESENTATION_SPAWNS: usize = 128;

/// Stable handle for a presentation template registered by a future authoring
/// surface. The registry carries the handle but never resolves it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PresentationTemplateHandle(pub String);

impl From<String> for PresentationTemplateHandle {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for PresentationTemplateHandle {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Producer-stamped scalar presentation facts. They deliberately do not read
/// from the registry after intake, so a transient remains valid when its source
/// entity is removed later in the same frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PresentationFact {
    Number(f32),
    Text(String),
    Bool(bool),
}

/// Facts keyed by the template-visible name. An ordered map makes future
/// serialization and diagnostics deterministic without giving producers access
/// to renderer state.
pub type PresentationFacts = BTreeMap<String, PresentationFact>;

/// Screen-space motion applied by the app-side transient pool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PresentationMotion {
    /// Total upward displacement in device pixels over the transient lifetime.
    pub rise_pixels: f32,
}

impl Default for PresentationMotion {
    fn default() -> Self {
        Self { rise_pixels: 0.0 }
    }
}

/// Fade parameters applied by the app-side transient pool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PresentationFade {
    /// Fade-out duration at the end of the transient lifetime, in seconds.
    pub duration_seconds: f32,
}

impl Default for PresentationFade {
    fn default() -> Self {
        Self {
            duration_seconds: 0.0,
        }
    }
}

/// One spawn request crossing from registry-side presentation producers to the
/// app-side pool. Time is intentionally absent: intake stamps it from the
/// frame-time clock so fixed-tick producers cannot make the visual clock drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationSpawn {
    pub world_anchor: Vec3,
    pub template: PresentationTemplateHandle,
    pub facts: PresentationFacts,
    pub lifetime_seconds: f32,
    pub motion: PresentationMotion,
    pub fade: PresentationFade,
}
