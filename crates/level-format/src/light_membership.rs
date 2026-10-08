//! Versioned JSON sidecars exchanged between `prl-build` and `scripts-build`.
//!
//! `prl-build` owns map-light identities; `scripts-build` owns evaluation, so
//! shared JSON records live beside the PRL format. See
//! `context/lib/build_pipeline.md`.

/// The only supported light-membership manifest contract version
/// (`scripts-build` → `prl-build`). The light table versions separately; see
/// [`LightTable::VERSION`].
pub const LIGHT_MEMBERSHIP_MANIFEST_VERSION: u32 = 1;

/// Runtime-present map data supplied to `scripts-build` while evaluating a
/// level data script. `_bake_only` lights are omitted; each surviving
/// `index` is its stable `MapData::lights` vector index, not a runtime entity id.
/// `map_members` carries the runtime-placed movers, trigger volumes and
/// spawners so build-side member queries answer what runtime answers.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightTable {
    pub version: u32,
    pub lights: Vec<LightTableLight>,
    /// Omitted from the wire when empty. Only `version` tells an authoritative
    /// empty (current version) from a lights-only table that never carried the
    /// key; see [`LightTable::members_supplied`].
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub map_members: Vec<MapMember>,
}

impl LightTable {
    /// The version this producer writes. Version 2 declares the member table
    /// supplied: an absent `mapMembers` key means the map has no movers,
    /// trigger volumes or spawners, and build-side member queries answer `[]`
    /// authoritatively.
    pub const VERSION: u32 = 2;

    /// A lights-only table from a producer that predates `mapMembers`. Its
    /// member table is unknown, not empty.
    pub const LIGHTS_ONLY_VERSION: u32 = 1;

    pub fn new(lights: Vec<LightTableLight>) -> Self {
        Self {
            version: Self::VERSION,
            lights,
            map_members: Vec::new(),
        }
    }

    pub fn with_map_members(mut self, map_members: Vec<MapMember>) -> Self {
        self.map_members = map_members;
        self
    }

    /// Accepts both [`Self::LIGHTS_ONLY_VERSION`] and [`Self::VERSION`]; callers
    /// distinguish them with [`Self::members_supplied`].
    pub fn validate_version(&self) -> std::result::Result<(), LightMembershipVersionError> {
        if (Self::LIGHTS_ONLY_VERSION..=Self::VERSION).contains(&self.version) {
            Ok(())
        } else {
            Err(LightMembershipVersionError {
                document: LightMembershipDocument::LightTable,
                found: self.version,
                oldest: Self::LIGHTS_ONLY_VERSION,
                newest: Self::VERSION,
            })
        }
    }

    /// Whether `map_members` is authoritative: true from [`Self::VERSION`] on,
    /// where an empty member list means the map has none.
    pub fn members_supplied(&self) -> bool {
        self.version >= Self::VERSION
    }
}

/// One light available to data-script `getMapEntities("light")`.
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

/// Non-light map kinds whose identity snapshots the build answers. The wire
/// spelling is the `worldQuery` component name.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMemberKind {
    KinematicMover,
    TriggerVolume,
    Spawner,
}

impl MapMemberKind {
    pub const ALL: [Self; 3] = [Self::KinematicMover, Self::TriggerVolume, Self::Spawner];

    /// The `worldQuery` component name, which is also the wire spelling.
    pub fn component(self) -> &'static str {
        match self {
            Self::KinematicMover => "kinematic_mover",
            Self::TriggerVolume => "trigger_volume",
            Self::Spawner => "spawner",
        }
    }

    pub fn from_component(component: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.component() == component)
    }
}

/// One runtime-placed mover, trigger volume or spawner, in authored order
/// within its kind. Mirrors the runtime identity snapshot
/// (`collect_identity_snapshots_json` in `postretro-sim`); `scripts-build`
/// assigns the build id.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq)]
pub struct MapMember {
    pub kind: MapMemberKind,
    pub tags: Vec<String>,
    /// Engine-space position of the runtime Transform, `[x, y, z]` on the wire.
    pub position: [f32; 3],
    /// Tags each NPC a spawner spawns carries. Empty, and omitted from the
    /// wire, for every other kind.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub spawned_tags: Vec<String>,
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
                document: LightMembershipDocument::Manifest,
                found: self.version,
                oldest: Self::VERSION,
                newest: Self::VERSION,
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

/// Which light-membership document carried an unsupported version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightMembershipDocument {
    /// The map-light table `prl-build` hands `scripts-build`.
    LightTable,
    /// The resolved light-membership sidecar `scripts-build` hands back.
    Manifest,
}

impl std::fmt::Display for LightMembershipDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::LightTable => "light table",
            Self::Manifest => "light-membership sidecar",
        })
    }
}

/// A stale or future light table or sidecar was supplied to a tool that does
/// not understand its version. `oldest..=newest` is the accepted range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightMembershipVersionError {
    pub document: LightMembershipDocument,
    pub found: u32,
    pub oldest: u32,
    pub newest: u32,
}

impl std::fmt::Display for LightMembershipVersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported {} version {} ", self.document, self.found)?;
        if self.oldest == self.newest {
            write!(f, "(expected {})", self.newest)
        } else {
            write!(f, "(expected {} through {})", self.oldest, self.newest)
        }
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
    fn light_membership_wire_structs_round_trip_with_camel_case_fields() {
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
        assert_eq!(table_json["version"], LightTable::VERSION);
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
        assert_eq!(manifest_json["version"], LIGHT_MEMBERSHIP_MANIFEST_VERSION);
        assert_eq!(manifest_json["lights"][0]["startActive"], true);
        assert_eq!(manifest_json["lights"][0]["startActiveConflict"], false);
        assert_eq!(manifest_json["stubbedPrimitives"][0], "fireTick");
        assert_eq!(
            serde_json::from_value::<LightMembershipManifest>(manifest_json)
                .expect("manifest round trips"),
            manifest
        );
    }

    // An empty member table leaves the light-table bytes exactly as a
    // lights-only table; a populated one rides under `mapMembers`.
    #[test]
    fn map_members_are_omitted_when_empty_and_round_trip_when_present() {
        let lights_only = serde_json::to_value(LightTable::new(vec![light()])).expect("serializes");
        assert!(lights_only.get("mapMembers").is_none());
        assert_eq!(
            serde_json::from_value::<LightTable>(lights_only)
                .expect("absent key reads as empty")
                .map_members,
            Vec::new()
        );

        let table = LightTable::new(Vec::new()).with_map_members(vec![
            MapMember {
                kind: MapMemberKind::TriggerVolume,
                tags: vec!["plate".to_string()],
                position: [1.0, 2.0, 3.0],
                spawned_tags: Vec::new(),
            },
            MapMember {
                kind: MapMemberKind::Spawner,
                tags: Vec::new(),
                position: [4.0, 5.0, 6.0],
                spawned_tags: vec!["wave_1".to_string()],
            },
        ]);
        let json = serde_json::to_value(&table).expect("serializes");
        assert_eq!(json["mapMembers"][0]["kind"], "trigger_volume");
        assert!(json["mapMembers"][0].get("spawnedTags").is_none());
        assert_eq!(json["mapMembers"][1]["kind"], "spawner");
        assert_eq!(json["mapMembers"][1]["spawnedTags"][0], "wave_1");
        assert_eq!(
            serde_json::from_value::<LightTable>(json).expect("round trips"),
            table
        );
        for kind in MapMemberKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).expect("kind serializes"),
                kind.component()
            );
            assert_eq!(MapMemberKind::from_component(kind.component()), Some(kind));
        }
        assert!(
            serde_json::from_value::<MapMember>(serde_json::json!({
                "kind": "light", "tags": [], "position": [0, 0, 0]
            }))
            .is_err()
        );
    }

    #[test]
    fn version_validation_rejects_stale_and_future_sidecars() {
        for version in [0, LightTable::VERSION + 1] {
            let mut table = LightTable::new(Vec::new());
            table.version = version;
            assert_eq!(
                table.validate_version(),
                Err(LightMembershipVersionError {
                    document: LightMembershipDocument::LightTable,
                    found: version,
                    oldest: LightTable::LIGHTS_ONLY_VERSION,
                    newest: LightTable::VERSION,
                })
            );
        }
        let mut table = LightTable::new(Vec::new());
        table.version = 0;
        assert_eq!(
            table.validate_version().unwrap_err().to_string(),
            format!(
                "unsupported light table version 0 (expected {} through {})",
                LightTable::LIGHTS_ONLY_VERSION,
                LightTable::VERSION
            )
        );
        let mut manifest = LightMembershipManifest::new(Vec::new(), Vec::new());
        manifest.version = LightMembershipManifest::VERSION + 1;
        assert_eq!(
            manifest.validate_version().unwrap_err().to_string(),
            format!(
                "unsupported light-membership sidecar version {} (expected {})",
                LightMembershipManifest::VERSION + 1,
                LightMembershipManifest::VERSION
            )
        );
    }

    // A v2 table supplies its member table, so an absent `mapMembers` key is an
    // authoritative empty; a v1 table predates the key, so its absence says
    // nothing about the map's members.
    #[test]
    fn version_distinguishes_supplied_members_from_lights_only_tables() {
        let current: LightTable =
            serde_json::from_str(r#"{"version":2,"lights":[]}"#).expect("v2 table parses");
        assert_eq!(current.validate_version(), Ok(()));
        assert!(current.members_supplied());
        assert!(current.map_members.is_empty());

        let lights_only: LightTable =
            serde_json::from_str(r#"{"version":1,"lights":[]}"#).expect("v1 table parses");
        assert_eq!(lights_only.validate_version(), Ok(()));
        assert!(!lights_only.members_supplied());
    }
}
