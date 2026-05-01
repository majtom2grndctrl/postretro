// Map-authored entity awaiting classname dispatch at level load.
// See: context/lib/build_pipeline.md §Built-in Classname Routing · context/lib/scripting.md §2

use std::collections::HashMap;

use glam::{EulerRot, Quat, Vec3};
use postretro_level_format::map_entity::MapEntityRecord;

/// One map entity as presented to classname handlers and the data-archetype
/// spawn path at level load. `key_values` is the residual KVP bag (reserved
/// keys stripped) as an unordered map; insertion order is not preserved.
/// `tags` is the pre-split `_tags` list; empty `Vec` when the source entity had none.
/// The compiler converts source-format angles to engine convention before
/// serialization — handlers and scripts never see Quake angles.
#[derive(Debug, Clone)]
pub(crate) struct MapEntity {
    pub(crate) classname: String,
    pub(crate) origin: Vec3,
    /// Engine-convention Euler angles (radians): x=pitch, y=yaw, z=roll (YXZ
    /// Euler order). Zeros when the source entity carried no `angles` /
    /// `mangle` / `angle` KVP.
    pub(crate) angles: Vec3,
    pub(crate) key_values: HashMap<String, String>,
    pub(crate) tags: Vec<String>,
}

impl MapEntity {
    /// Convenience: assemble the diagnostic prefix used by handlers when they
    /// log warnings about a malformed key value. Format mirrors classic
    /// `id Tech` baker logs: `classname @ (x, y, z)`.
    pub(crate) fn diagnostic_origin(&self) -> String {
        format!(
            "{} @ ({:.3}, {:.3}, {:.3})",
            self.classname, self.origin.x, self.origin.y, self.origin.z
        )
    }

    /// Build the spawn-time `Transform.rotation` from `self.angles`. Engine
    /// convention is `Quat::from_euler(YXZ, yaw, pitch, roll)` (matches
    /// `EulerDegrees::to_quat` in `conv.rs` and the angle convention produced
    /// by `quake_to_engine_angles` in the level compiler). Returns
    /// `Quat::IDENTITY` when angles are all zero.
    pub(crate) fn rotation_quat(&self) -> Quat {
        // Exact == is intentional: the format adapter returns the literal-zero
        // default sentinel when no angles KVP was present, so this equality
        // check is reliable and there is no need for an epsilon comparison.
        if self.angles == Vec3::ZERO {
            return Quat::IDENTITY;
        }
        Quat::from_euler(EulerRot::YXZ, self.angles.y, self.angles.x, self.angles.z)
    }
}

/// Adapter from the format-crate wire record to the scripting-facing
/// `MapEntity`. Lives in the scripting tree so the loader does not depend on
/// scripting types. Called in `main.rs` at the dispatch boundary, before
/// passing the converted slice to `apply_classname_dispatch`.
impl From<MapEntityRecord> for MapEntity {
    fn from(record: MapEntityRecord) -> Self {
        let mut kv = HashMap::with_capacity(record.key_values.len());
        for (k, v) in record.key_values {
            kv.insert(k, v);
        }
        Self {
            classname: record.classname,
            origin: Vec3::from(record.origin),
            angles: Vec3::from(record.angles),
            key_values: kv,
            tags: record.tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::conv::EulerDegrees;
    use std::collections::HashMap;

    /// Pins the invariant that `MapEntity::rotation_quat` (radians, YXZ via
    /// `glam`) and `EulerDegrees::to_quat` (degrees, YXZ via `glam`) build
    /// the same quaternion for the same logical rotation. A future refactor
    /// that swaps the Euler order or argument layout in either path will
    /// trip this test before it can silently diverge spawn-time and
    /// script-facing rotations.
    #[test]
    fn rotation_quat_matches_euler_degrees_to_quat() {
        // Pick angles that exercise all three axes; nothing on a 90° boundary
        // so YXZ vs other orderings would land at distinct quaternions.
        let pitch_deg: f32 = 12.5;
        let yaw_deg: f32 = -47.0;
        let roll_deg: f32 = 33.0;

        let pitch = pitch_deg.to_radians();
        let yaw = yaw_deg.to_radians();
        let roll = roll_deg.to_radians();

        let entity = MapEntity {
            classname: "_test".to_string(),
            origin: Vec3::ZERO,
            angles: Vec3::new(pitch, yaw, roll),
            key_values: HashMap::new(),
            tags: vec![],
        };

        let from_map = entity.rotation_quat();
        let from_euler = EulerDegrees {
            pitch: pitch_deg,
            yaw: yaw_deg,
            roll: roll_deg,
        }
        .to_quat();

        let eps = 1e-6;
        assert!(
            (from_map.x - from_euler.x).abs() < eps
                && (from_map.y - from_euler.y).abs() < eps
                && (from_map.z - from_euler.z).abs() < eps
                && (from_map.w - from_euler.w).abs() < eps,
            "rotation_quat={from_map:?} vs EulerDegrees::to_quat={from_euler:?}",
        );
    }
}
