//! Local, plain-data intake for frame-time impact particle bursts.

use glam::Vec3;

/// Maximum queued bursts between frame drains. Overflow drops newest requests.
pub const MAX_PENDING_IMPACT_PARTICLE_REQUESTS: usize = 128;

/// One local cosmetic contact. This request has no entity identity or wire form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactParticleRequest {
    pub point: Vec3,
    pub normal: Vec3,
}
