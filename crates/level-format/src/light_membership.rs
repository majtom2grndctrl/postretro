//! Versioned JSON sidecars exchanged between `prl-build` and `scripts-build`.
//!
//! `prl-build` owns map-light identities; `scripts-build` owns evaluation, so
//! shared JSON records live beside the PRL format. See
//! `context/lib/build_pipeline.md`.

/// The only supported light-membership sidecar contract version.
pub const LIGHT_MEMBERSHIP_MANIFEST_VERSION: u32 = 1;

/// Runtime-present map-light data supplied to `scripts-build` while evaluating
/// a level data script. `_bake_only` lights are omitted; each surviving
/// `index` is its stable `MapData::lights` vector index, not a runtime entity id.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightTable {
    pub version: u32,
    pub lights: Vec<LightTableLight>,
}

impl LightTable {
    pub const VERSION: u32 = LIGHT_MEMBERSHIP_MANIFEST_VERSION;

    pub fn new(lights: Vec<LightTableLight>) -> Self {
        Self {
            version: Self::VERSION,
            lights,
        }
    }

    pub fn validate_version(&self) -> std::result::Result<(), LightMembershipVersionError> {
        if self.version == Self::VERSION {
            Ok(())
        } else {
            Err(LightMembershipVersionError {
                found: self.version,
                expected: Self::VERSION,
            })
        }
    }
}

/// One light available to data-script `world.query({ component: "light" })`.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightTableLight {
    pub index: u32,
    pub tags: Vec<String>,
    /// Engine-space position, encoded as `[x, y, z]` for a stable JSON wire
    /// format. The script host reshapes it to the SDK's `{ x, y, z }` value.
    pub position: [f32; 3],
    pub is_dynamic: bool,
    /// Build-side `LightComponent` snapshot. Arrays in this wire record are
    /// reshaped to `{ x, y, z }` vectors before the SDK sees them. Internal
    /// routing fields are removed from the authored query surface.
    pub component: LightComponentSnapshot,
}

/// Build-side light component as it crosses the compiler-side JSON seam.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightComponentSnapshot {
    pub origin: [f32; 3],
    pub light_type: String,
    pub intensity: f32,
    pub color: [f32; 3],
    pub falloff_model: String,
    pub falloff_range: f32,
    pub cone_angle_inner: Option<f32>,
    pub cone_angle_outer: Option<f32>,
    pub cone_direction: Option<[f32; 3]>,
    pub is_dynamic: bool,
    /// Compose-side routing metadata retained for wire compatibility. The
    /// manifest evaluator removes it before exposing query snapshots to scripts.
    pub animated_slot: Option<u32>,
    pub animation: Option<LightAnimationSnapshot>,
}

/// Full script-facing runtime animation snapshot. Curves are not baked by the
/// membership manifest; this field exists so query handles retain the normal
/// `LightComponent` shape.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightAnimationSnapshot {
    pub period_ms: f32,
    pub phase: Option<f32>,
    pub play_count: Option<u32>,
    pub start_active: Option<bool>,
    pub brightness: Option<Vec<f32>>,
    pub color: Option<Vec<[f32; 3]>>,
    pub direction: Option<Vec<[f32; 3]>>,
}

/// Resolved output from `scripts-build`. Includes dynamic targets so
/// `prl-build` can report them as normal runtime-only animation paths.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightMembershipManifest {
    pub version: u32,
    pub lights: Vec<LightMembershipRecord>,
    pub stubbed_primitives: Vec<String>,
}

impl LightMembershipManifest {
    pub const VERSION: u32 = LIGHT_MEMBERSHIP_MANIFEST_VERSION;

    pub fn new(lights: Vec<LightMembershipRecord>, stubbed_primitives: Vec<String>) -> Self {
        Self {
            version: Self::VERSION,
            lights,
            stubbed_primitives,
        }
    }

    pub fn validate_version(&self) -> std::result::Result<(), LightMembershipVersionError> {
        if self.version == Self::VERSION {
            Ok(())
        } else {
            Err(LightMembershipVersionError {
                found: self.version,
                expected: Self::VERSION,
            })
        }
    }
}

/// One resolved, map-light-indexed animation target.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightMembershipRecord {
    pub index: u32,
    pub is_dynamic: bool,
    /// `None` means no level-load reaction addressed this light. A
    /// level-load step with omitted/null `startActive` resolves to `Some(true)`
    /// because that is the runtime descriptor default.
    pub start_active: Option<bool>,
    pub start_active_conflict: bool,
}

/// A stale or future sidecar was supplied to a compiler that only understands
/// the v1 contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightMembershipVersionError {
    pub found: u32,
    pub expected: u32,
}

impl std::fmt::Display for LightMembershipVersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unsupported light-membership sidecar version {} (expected {})",
            self.found, self.expected
        )
    }
}

impl std::error::Error for LightMembershipVersionError {}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    fn light() -> LightTableLight {
        LightTableLight {
            index: 7,
            tags: vec!["arena".to_string()],
            position: [1.0, 2.0, 3.0],
            is_dynamic: false,
            component: LightComponentSnapshot {
                origin: [1.0, 2.0, 3.0],
                light_type: "Point".to_string(),
                intensity: 1.25,
                color: [0.5, 0.75, 1.0],
                falloff_model: "InverseSquared".to_string(),
                falloff_range: 12.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: false,
                animated_slot: Some(3),
                animation: Some(LightAnimationSnapshot {
                    period_ms: 500.0,
                    phase: Some(0.25),
                    play_count: None,
                    start_active: Some(true),
                    brightness: Some(vec![0.0, 1.0]),
                    color: Some(vec![[1.0, 0.0, 0.0]]),
                    direction: None,
                }),
            },
        }
    }

    #[test]
    fn light_membership_wire_structs_round_trip_with_v1_camel_case_fields() {
        let table = LightTable::new(vec![light()]);
        let manifest = LightMembershipManifest::new(
            vec![LightMembershipRecord {
                index: 7,
                is_dynamic: false,
                start_active: Some(true),
                start_active_conflict: false,
            }],
            vec!["fireTick".to_string()],
        );

        let table_json = serde_json::to_value(&table).expect("table serializes");
        assert_eq!(table_json["version"], 1);
        assert_eq!(table_json["lights"][0]["isDynamic"], false);
        assert_eq!(table_json["lights"][0]["component"]["lightType"], "Point");
        assert_eq!(table_json["lights"][0]["component"]["animatedSlot"], 3);
        assert_eq!(
            table_json["lights"][0]["component"]["animation"]["periodMs"],
            500.0
        );
        assert_eq!(
            serde_json::from_value::<LightTable>(table_json).expect("table round trips"),
            table
        );

        let manifest_json = serde_json::to_value(&manifest).expect("manifest serializes");
        assert_eq!(manifest_json["lights"][0]["startActive"], true);
        assert_eq!(manifest_json["lights"][0]["startActiveConflict"], false);
        assert_eq!(manifest_json["stubbedPrimitives"][0], "fireTick");
        assert_eq!(
            serde_json::from_value::<LightMembershipManifest>(manifest_json)
                .expect("manifest round trips"),
            manifest
        );
    }

    #[test]
    fn version_validation_rejects_stale_sidecars() {
        let mut table = LightTable::new(Vec::new());
        table.version = 0;
        assert_eq!(
            table.validate_version(),
            Err(LightMembershipVersionError {
                found: 0,
                expected: LIGHT_MEMBERSHIP_MANIFEST_VERSION,
            })
        );
    }
}
