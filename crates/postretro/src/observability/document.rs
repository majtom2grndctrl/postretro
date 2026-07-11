// Output document vocabulary: the entity-state dump a headless run emits, plus
// the filter that selects entities out of a live registry.
// See: context/plans/in-progress/agentic-observability

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use postretro_entities::{ComponentValue, EntityRegistry};

use super::runspec::DumpSpec;
use super::{ALL_KINDS, DumpError};

/// The headless output document — the stable, tool-facing surface a run emits.
/// Field order here is the emitted top-level order; nested map keys are sorted by
/// [`super::to_deterministic_json`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OutputDocument {
    /// The map the run loaded (echoed from the runspec for provenance).
    pub map: String,
    /// Number of fixed ticks actually advanced.
    pub ticks_run: u32,
    /// Filtered entity records (see [`EntityRecord`]).
    pub entities: Vec<EntityRecord>,
    /// Count of records dropped by the entry cap. `0` when nothing was
    /// truncated; a positive value is reported explicitly so a truncated dump is
    /// never silently short.
    pub truncated: usize,
    /// Per-tick event lists. Empty when `dump.events` is false.
    pub events: Vec<TickEventRecord>,
    /// Summary of the local player pawn, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerPawnSummary>,
    /// What headless mode leaves out of frame, in two categories.
    pub out_of_frame: OutOfFrame,
}

/// One dumped entity component: its raw entity id, its tags, and the
/// `ComponentValue` serialized through its own serde derive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EntityRecord {
    /// Raw packed entity id (`index | generation << 16`).
    pub entity: u32,
    /// The entity's tags (empty when untagged).
    pub tags: Vec<String>,
    /// The component payload. Serializes with its `kind` discriminant tag.
    pub component: ComponentValue,
}

/// Per-tick event lists collected across the run. The driver fills these from the
/// sim's `TickEvents`; the builder includes them only when `dump.events` is set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct TickEventRecord {
    pub tick: u32,
    #[serde(default)]
    pub movement: Vec<String>,
    #[serde(default)]
    pub ai: Vec<String>,
    #[serde(default)]
    pub weapon: Vec<String>,
    #[serde(default)]
    pub death: Vec<String>,
}

/// Curated view of the local player pawn. Filled by the driver from the
/// post-run registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlayerPawnSummary {
    pub entity: u32,
    pub position: [f32; 3],
    pub facing_yaw: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<PawnHealth>,
}

/// Health slice of the pawn summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PawnHealth {
    pub current: f32,
    pub max: f32,
}

/// The two-category out-of-frame declaration: what a headless run cannot or does
/// not report, so a consumer never mistakes absence for "there is none".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OutOfFrame {
    /// State that does not exist as entities in headless mode at all.
    pub absent_headless: Vec<String>,
    /// State — non-entity runtime data, or a windowed runtime behavior — that is
    /// present in the world but deliberately outside the headless dump or sim.
    pub present_not_dumped: Vec<String>,
}

impl OutOfFrame {
    /// The canonical headless declaration. Constant across runs.
    pub(crate) fn headless() -> Self {
        Self {
            // Baked map lights live in level data, not the entity registry, so
            // they are simply absent from a headless entity dump.
            absent_headless: vec!["map_lights".to_string()],
            // These exist at runtime but are not entity components, so the dump
            // does not carry them. `trigger_evaluation` is the odd one out: the
            // trigger-volume entities ARE dumped, but headless drives no trigger
            // context into `simulate_tick`, so the trigger system never fires.
            present_not_dumped: vec![
                "collision_geometry".to_string(),
                "mover_geometry".to_string(),
                "hit_zones".to_string(),
                "trigger_evaluation".to_string(),
            ],
        }
    }
}

/// The entities a [`DumpSpec`] selects out of a registry, plus the count dropped
/// by the entry cap.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DumpSelection {
    pub records: Vec<EntityRecord>,
    /// Number of matched records omitted by the cap (`0` when nothing dropped).
    pub truncated: usize,
}

/// Apply a dump filter against a live registry, producing the record list and
/// truncation count.
///
/// Selection order: component kinds in `ComponentKind` discriminant order, and
/// within each kind, entities in slot-index order — deterministic across runs.
/// A `None` component filter walks every kind (so an entity carrying several
/// components yields one record per component). The tag filter, entity-id
/// allowlist, and cap all compose.
///
/// `dump.cap: 0` is a valid, distinct-from-default value: it empties `records`
/// and reports the entire matched population as `truncated` (honest — the count
/// is correct — but easy to mistake for the default cap).
pub(crate) fn apply_dump(
    registry: &EntityRegistry,
    dump: &DumpSpec,
) -> Result<DumpSelection, DumpError> {
    let kind_filter = dump.resolve_component()?;
    let id_allow: Option<HashSet<u32>> = dump
        .entities
        .as_ref()
        .map(|ids| ids.iter().copied().collect());
    let tag = dump.tag.as_deref();

    let kinds: Vec<_> = match kind_filter {
        Some(kind) => vec![kind],
        None => ALL_KINDS.to_vec(),
    };

    let mut records = Vec::new();
    for kind in kinds {
        for (id, value) in registry.query_by_component_and_tag(kind, tag) {
            let raw = id.to_raw();
            if let Some(allow) = &id_allow {
                if !allow.contains(&raw) {
                    continue;
                }
            }
            let tags = registry
                .get_tags(id)
                .map(|t| t.to_vec())
                .unwrap_or_default();
            records.push(EntityRecord {
                entity: raw,
                tags,
                component: value.clone(),
            });
        }
    }

    let truncated = records.len().saturating_sub(dump.cap);
    if truncated > 0 {
        records.truncate(dump.cap);
    }

    Ok(DumpSelection { records, truncated })
}

/// Assemble the full output document. Applies the entity filter to `registry`,
/// carries through the driver-supplied per-tick events (only when `dump.events`
/// is set) and player summary, and stamps the constant out-of-frame declaration.
pub(crate) fn build_output_document(
    map: impl Into<String>,
    ticks_run: u32,
    registry: &EntityRegistry,
    dump: &DumpSpec,
    events: Vec<TickEventRecord>,
    player: Option<PlayerPawnSummary>,
) -> Result<OutputDocument, DumpError> {
    let selection = apply_dump(registry, dump)?;
    Ok(OutputDocument {
        map: map.into(),
        ticks_run,
        entities: selection.records,
        truncated: selection.truncated,
        events: if dump.events { events } else { Vec::new() },
        player,
        out_of_frame: OutOfFrame::headless(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
    use std::collections::HashMap;

    fn health(max: f32) -> HealthComponent {
        HealthComponent {
            max,
            current: max,
            hitbox: None,
            death_handled: false,
            zone_multipliers: HashMap::new(),
            contributor_ledger: Default::default(),
        }
    }

    fn spawn_with_health(reg: &mut EntityRegistry, tags: &[&str], max: f32) -> EntityId {
        let id = reg.spawn(Transform::default());
        if !tags.is_empty() {
            reg.set_tags(id, tags.iter().map(|t| t.to_string()).collect())
                .unwrap();
        }
        reg.set_component_value(id, ComponentValue::Health(health(max)))
            .unwrap();
        id
    }

    fn dump_for_component(kind: &str) -> DumpSpec {
        DumpSpec {
            component: Some(kind.to_string()),
            ..DumpSpec::default()
        }
    }

    #[test]
    fn component_filter_selects_only_matching_kind() {
        let mut reg = EntityRegistry::new();
        let with_health = spawn_with_health(&mut reg, &[], 100.0);
        // A second entity carrying only a Transform — must not appear in a
        // health dump.
        let _bare = reg.spawn(Transform::default());

        let selection = apply_dump(&reg, &dump_for_component("health")).unwrap();
        assert_eq!(selection.records.len(), 1);
        assert_eq!(selection.records[0].entity, with_health.to_raw());
        assert_eq!(selection.records[0].component.kind(), ComponentKind::Health);
        assert_eq!(selection.truncated, 0);
    }

    #[test]
    fn tag_filter_keeps_only_tagged_entities() {
        let mut reg = EntityRegistry::new();
        let tagged = spawn_with_health(&mut reg, &["enemy"], 50.0);
        let _untagged = spawn_with_health(&mut reg, &[], 50.0);

        let dump = DumpSpec {
            component: Some("health".to_string()),
            tag: Some("enemy".to_string()),
            ..DumpSpec::default()
        };
        let selection = apply_dump(&reg, &dump).unwrap();
        assert_eq!(selection.records.len(), 1);
        assert_eq!(selection.records[0].entity, tagged.to_raw());
        assert_eq!(selection.records[0].tags, vec!["enemy".to_string()]);
    }

    #[test]
    fn entity_id_allowlist_restricts_to_listed_ids() {
        let mut reg = EntityRegistry::new();
        let a = spawn_with_health(&mut reg, &[], 10.0);
        let _b = spawn_with_health(&mut reg, &[], 10.0);
        let c = spawn_with_health(&mut reg, &[], 10.0);

        let dump = DumpSpec {
            component: Some("health".to_string()),
            entities: Some(vec![a.to_raw(), c.to_raw()]),
            ..DumpSpec::default()
        };
        let selection = apply_dump(&reg, &dump).unwrap();
        let ids: Vec<u32> = selection.records.iter().map(|r| r.entity).collect();
        assert_eq!(ids, vec![a.to_raw(), c.to_raw()]);
    }

    #[test]
    fn none_component_filter_dumps_every_component_of_every_entity() {
        let mut reg = EntityRegistry::new();
        // One entity with Transform (implicit) + Health => two component records.
        let _id = spawn_with_health(&mut reg, &[], 20.0);

        let selection = apply_dump(&reg, &DumpSpec::default()).unwrap();
        let kinds: HashSet<ComponentKind> = selection
            .records
            .iter()
            .map(|r| r.component.kind())
            .collect();
        assert!(kinds.contains(&ComponentKind::Transform));
        assert!(kinds.contains(&ComponentKind::Health));
    }

    #[test]
    fn cap_truncates_and_reports_omitted_count() {
        let mut reg = EntityRegistry::new();
        for _ in 0..5 {
            spawn_with_health(&mut reg, &[], 10.0);
        }
        let dump = DumpSpec {
            component: Some("health".to_string()),
            cap: 2,
            ..DumpSpec::default()
        };
        let selection = apply_dump(&reg, &dump).unwrap();
        assert_eq!(selection.records.len(), 2, "capped to the limit");
        assert_eq!(selection.truncated, 3, "omitted count reported explicitly");
    }

    #[test]
    fn apply_dump_surfaces_unknown_component_kind() {
        let reg = EntityRegistry::new();
        let dump = DumpSpec {
            component: Some("not_real".to_string()),
            ..DumpSpec::default()
        };
        assert_eq!(
            apply_dump(&reg, &dump),
            Err(DumpError::UnknownComponentKind("not_real".to_string()))
        );
    }

    #[test]
    fn build_document_carries_out_of_frame_two_categories() {
        let reg = EntityRegistry::new();
        let doc = build_output_document(
            "content/dev/maps/x.prl",
            42,
            &reg,
            &DumpSpec::default(),
            vec![],
            None,
        )
        .unwrap();

        assert_eq!(doc.map, "content/dev/maps/x.prl");
        assert_eq!(doc.ticks_run, 42);
        assert_eq!(
            doc.out_of_frame.absent_headless,
            vec!["map_lights".to_string()]
        );
        assert_eq!(
            doc.out_of_frame.present_not_dumped,
            vec![
                "collision_geometry".to_string(),
                "mover_geometry".to_string(),
                "hit_zones".to_string(),
                "trigger_evaluation".to_string(),
            ]
        );
    }

    #[test]
    fn build_document_omits_events_when_flag_off() {
        let reg = EntityRegistry::new();
        let events = vec![TickEventRecord {
            tick: 0,
            death: vec!["enemy_died".to_string()],
            ..TickEventRecord::default()
        }];
        let dump = DumpSpec {
            events: false,
            ..DumpSpec::default()
        };
        let doc = build_output_document("m.prl", 1, &reg, &dump, events.clone(), None).unwrap();
        assert!(doc.events.is_empty(), "events suppressed when flag off");

        let dump_on = DumpSpec::default();
        let doc_on = build_output_document("m.prl", 1, &reg, &dump_on, events, None).unwrap();
        assert_eq!(doc_on.events.len(), 1);
    }

    #[test]
    fn build_document_truncation_surfaces_on_the_document() {
        let mut reg = EntityRegistry::new();
        for _ in 0..4 {
            spawn_with_health(&mut reg, &[], 10.0);
        }
        let dump = DumpSpec {
            component: Some("health".to_string()),
            cap: 1,
            ..DumpSpec::default()
        };
        let doc = build_output_document("m.prl", 1, &reg, &dump, vec![], None).unwrap();
        assert_eq!(doc.entities.len(), 1);
        assert_eq!(doc.truncated, 3);
    }

    #[test]
    fn output_document_serializes_deterministically() {
        // End-to-end: a document whose dumped Health carries a multi-entry
        // zone-multiplier map must serialize byte-identically across two builds
        // with different map insertion order.
        fn doc_with_multipliers(order: &[(&str, f32)]) -> OutputDocument {
            let mut reg = EntityRegistry::new();
            let id = reg.spawn(Transform::default());
            let mut zone_multipliers = HashMap::new();
            for (tag, factor) in order {
                zone_multipliers.insert((*tag).to_string(), *factor);
            }
            reg.set_component_value(
                id,
                ComponentValue::Health(HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    zone_multipliers,
                    contributor_ledger: Default::default(),
                }),
            )
            .unwrap();
            build_output_document(
                "m.prl",
                10,
                &reg,
                &dump_for_component("health"),
                vec![],
                None,
            )
            .unwrap()
        }

        let a = super::super::to_deterministic_json(&doc_with_multipliers(&[
            ("head", 2.0),
            ("leg", 0.5),
            ("torso", 1.0),
        ]))
        .unwrap();
        let b = super::super::to_deterministic_json(&doc_with_multipliers(&[
            ("torso", 1.0),
            ("head", 2.0),
            ("leg", 0.5),
        ]))
        .unwrap();
        assert_eq!(a, b);
    }
}
