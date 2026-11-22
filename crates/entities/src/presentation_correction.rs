// Render-only pose corrections the registry's presented-pose accessor applies
// after its blend.
// See: context/lib/entity_model.md §5 (Per-Entity Transform Interpolation)

use glam::{Quat, Vec3};

use crate::Transform;

/// A rigid change to an entity's pose: a world translation of its origin and a
/// world rotation about that origin. Applied to a `Transform` as
/// `position + translation`, `rotation * transform.rotation`. Scale is never
/// corrected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseCorrection {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl PoseCorrection {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    /// The correction that carries `from` onto `to`'s position and rotation.
    pub fn between(from: &Transform, to: &Transform) -> Self {
        Self {
            translation: to.position - from.position,
            rotation: (to.rotation * from.rotation.inverse()).normalize(),
        }
    }

    pub fn apply(&self, transform: &Transform) -> Transform {
        Transform {
            position: transform.position + self.translation,
            rotation: (self.rotation * transform.rotation).normalize(),
            scale: transform.scale,
        }
    }

    pub fn inverse(&self) -> Self {
        Self {
            translation: -self.translation,
            rotation: self.rotation.inverse(),
        }
    }

    /// This correction, then `next`.
    pub fn then(&self, next: &Self) -> Self {
        Self {
            translation: self.translation + next.translation,
            rotation: (next.rotation * self.rotation).normalize(),
        }
    }

    /// The fraction `remaining` of this correction, toward identity.
    pub fn scaled(&self, remaining: f32) -> Self {
        Self {
            translation: self.translation * remaining,
            rotation: Quat::IDENTITY.slerp(self.rotation, remaining).normalize(),
        }
    }

    pub fn is_finite(&self) -> bool {
        self.translation.is_finite() && self.rotation.is_finite()
    }

    /// Below `epsilon` metres of translation and `epsilon` radians of rotation.
    pub fn is_within(&self, epsilon: f32) -> bool {
        self.translation.length() < epsilon && self.angle() < epsilon
    }

    /// The rotation's angle in radians, robust near identity.
    pub fn angle(&self) -> f32 {
        let rotation = self.rotation.normalize();
        2.0 * rotation.xyz().length().atan2(rotation.w.abs())
    }

    /// The farthest any of `points`, in the entity's local frame at `pose`,
    /// moves under this correction.
    pub fn max_point_displacement(&self, pose: &Transform, points: &[Vec3]) -> f32 {
        let corrected = self.apply(pose);
        points
            .iter()
            .map(|local| {
                let at = |t: &Transform| t.position + t.rotation * (t.scale * *local);
                (at(&corrected) - at(pose)).length()
            })
            .fold(0.0, f32::max)
    }
}

#[cfg(test)]
#[path = "presentation_correction_tests.rs"]
mod tests;
