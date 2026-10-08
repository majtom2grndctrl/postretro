//! Compiler-side adaptation of script-derived light membership into bake input.
//!
//! The versioned JSON seam lives in `postretro-level-format`; this module owns
//! the `MapData` mutation that reserves existing animated-bake structures.

use anyhow::{Context as _, Result, bail};
use postretro_level_format::light_membership::{
    LightComponentSnapshot, LightMembershipManifest, LightTable, LightTableLight, MapMember,
    MapMemberKind,
};

use crate::map_data::{
    FalloffModel, LightType, MapEntityRecord, MapKinematicMover, MapLight, MapTriggerVolume,
    animated_light_placeholder,
};

/// Runtime classname whose built-in handler places a spawner
/// (`postretro-sim` `scripting::builtins::entity_spawner::CLASSNAME`). The
/// level compiler interprets no other map-entity classname as a member.
const ENTITY_SPAWNER_CLASSNAME: &str = "entity_spawner";

/// Raw KVP the runtime spawner handler splits on whitespace into the tags its
/// spawns carry.
const SPAWNED_TAGS_KEY: &str = "spawned_tags";

/// Inventory emitted by `prl-build` after it accepts a manifest. Keeping it as
/// data makes the routing decision directly testable; logging stays at the
/// compiler boundary rather than inside light namespaces.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MembershipInventory {
    pub(crate) derived_static_indices: Vec<usize>,
    pub(crate) flag_only_indices: Vec<usize>,
    pub(crate) dynamic_target_indices: Vec<usize>,
    pub(crate) start_active_conflict_indices: Vec<usize>,
    pub(crate) stubbed_primitives: Vec<String>,
}

/// Build the compiler query table from the parsed map lights. The vec position
/// is intentionally the only identity sent across the sidecar seam. The
/// script host removes internal routing fields before exposing snapshots.
pub(crate) fn light_table_from_lights(lights: &[MapLight]) -> Result<LightTable> {
    let lights = lights
        .iter()
        .enumerate()
        // `_bake_only` lights have no runtime entity. Omitting them keeps the
        // compiler query's result order and membership faithful to the runtime
        // query while `index` below retains raw MapData identity for the
        // sidecar's compiler-facing remap.
        .filter(|(_, light)| !light.bake_only)
        .map(|(index, light)| {
            Ok(LightTableLight {
                index: u32::try_from(index)
                    .context("map has more than u32::MAX lights; cannot build light table")?,
                tags: light.tags.clone(),
                position: vec3(light.origin),
                is_dynamic: light.is_dynamic,
                component: LightComponentSnapshot {
                    origin: vec3(light.origin),
                    light_type: light_type_name(light.light_type).to_owned(),
                    intensity: light.intensity,
                    color: light.color,
                    falloff_model: falloff_model_name(light.falloff_model).to_owned(),
                    falloff_range: light.falloff_range,
                    cone_angle_inner: light.cone_angle_inner,
                    cone_angle_outer: light.cone_angle_outer,
                    cone_direction: light.cone_direction,
                    is_dynamic: light.is_dynamic,
                    // Slots are allocated only after the sidecar is consumed;
                    // the parsed map has no compose-slot number to expose yet.
                    animated_slot: None,
                    // Runtime LightBridge initializes every map-light
                    // component with no script animation. Authored bake curves
                    // are compiler input, not the setupLevel snapshot.
                    animation: None,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(LightTable::new(lights))
}

/// Build the non-light member table: exactly the movers, trigger volumes
/// (switches included) and spawners the runtime places, each with the tags and
/// Transform position runtime gives it. Order is authored order within each
/// kind, the order runtime spawns them and therefore answers queries in.
///
/// Positions mirror the runtime spawn sites: a mover's packed origin
/// (`runtime_movers` spawns at the PRL record origin), a trigger volume's AABB
/// center (`TriggerVolumeBridge::populate_from_level`), and a spawner's
/// narrowed entity origin (`entity_spawner::handle`).
pub(crate) fn map_members_from_map(
    movers: &[MapKinematicMover],
    triggers: &[MapTriggerVolume],
    entities: &[MapEntityRecord],
) -> Vec<MapMember> {
    let movers = movers.iter().map(|mover| MapMember {
        kind: MapMemberKind::KinematicMover,
        tags: mover.tags.clone(),
        position: vec3(mover.origin),
        spawned_tags: Vec::new(),
    });
    let triggers = triggers.iter().map(|trigger| MapMember {
        kind: MapMemberKind::TriggerVolume,
        tags: trigger.tags.clone(),
        position: aabb_center(trigger.aabb_min, trigger.aabb_max),
        spawned_tags: Vec::new(),
    });
    let spawners = entities
        .iter()
        .filter(|entity| entity.classname == ENTITY_SPAWNER_CLASSNAME)
        .map(|entity| MapMember {
            kind: MapMemberKind::Spawner,
            tags: entity.tags.clone(),
            position: vec3(entity.origin),
            // The runtime reads the KVP table last-occurrence-wins.
            spawned_tags: entity
                .key_values
                .iter()
                .rev()
                .find(|(key, _)| key == SPAWNED_TAGS_KEY)
                .map(|(_, raw)| raw.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
        });
    movers.chain(triggers).chain(spawners).collect()
}

/// Validate and apply script-derived membership before namespaces are formed.
///
/// Static targets become the same empty animation placeholder produced by the
/// Quake `_animated` parser. Dynamic targets are deliberately only reported:
/// their runtime path owns all animation and no bake structure is reserved.
pub(crate) fn apply_manifest(
    lights: &mut [MapLight],
    authored_start_active_defaults: &[bool],
    manifest: &LightMembershipManifest,
) -> Result<MembershipInventory> {
    manifest
        .validate_version()
        .map_err(anyhow::Error::from)
        .context("invalid light-membership manifest")?;
    if authored_start_active_defaults.len() != lights.len() {
        bail!(
            "authored light start-state table has {} entries for {} map lights",
            authored_start_active_defaults.len(),
            lights.len()
        );
    }

    let mut inventory = MembershipInventory {
        stubbed_primitives: manifest.stubbed_primitives.clone(),
        ..MembershipInventory::default()
    };
    let mut script_targeted_static = vec![false; lights.len()];
    let mut seen_record_indices = vec![false; lights.len()];

    for record in &manifest.lights {
        let index = usize::try_from(record.index)
            .context("light-membership record index does not fit usize")?;
        let light_count = lights.len();
        if index < seen_record_indices.len() && seen_record_indices[index] {
            bail!(
                "light-membership manifest contains duplicate record for map-light index {index}"
            );
        }
        let light = lights.get_mut(index).ok_or_else(|| {
            anyhow::anyhow!(
                "light-membership record references map-light index {index}, but the map has {} lights",
                light_count
            )
        })?;
        seen_record_indices[index] = true;

        if record.is_dynamic != light.is_dynamic {
            bail!(
                "light-membership record for map-light index {index} reports isDynamic={}, but parsed map data reports isDynamic={}",
                record.is_dynamic,
                light.is_dynamic
            );
        }

        if record.start_active_conflict {
            inventory.start_active_conflict_indices.push(index);
        }

        if light.is_dynamic {
            inventory.dynamic_target_indices.push(index);
            continue;
        }

        script_targeted_static[index] = true;
        if light.animation.is_none() && !light.is_animated {
            light.animation = Some(animated_light_placeholder(
                record
                    .start_active
                    .unwrap_or(authored_start_active_defaults[index]),
            ));
            inventory.derived_static_indices.push(index);
        } else if light.is_animated {
            let animation = light.animation.as_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "map-light index {index} is marked _animated but has no placeholder animation"
                )
            })?;
            if let Some(start_active) = record.start_active {
                animation.start_active = start_active;
            }
        }
    }

    inventory.flag_only_indices = lights
        .iter()
        .enumerate()
        .filter_map(|(index, light)| {
            (light.is_animated && !script_targeted_static[index]).then_some(index)
        })
        .collect();

    Ok(inventory)
}

pub(crate) fn log_inventory(inventory: &MembershipInventory, lights: &[MapLight]) {
    for &index in &inventory.derived_static_indices {
        log::info!(
            "[prl-build] light membership: derived animated-bake reservation for static light {index} (tags: {})",
            tags_for_log(&lights[index])
        );
    }
    for &index in &inventory.flag_only_indices {
        log::info!(
            "[prl-build] light membership: explicit _animated reservation for static light {index} (tags: {})",
            tags_for_log(&lights[index])
        );
    }
    for &index in &inventory.dynamic_target_indices {
        log::info!(
            "[prl-build] light membership: dynamic light {index} remains runtime-only (tags: {})",
            tags_for_log(&lights[index])
        );
    }
    for &index in &inventory.start_active_conflict_indices {
        log::warn!(
            "[prl-build] light membership: conflicting levelLoad startActive values for light {index} (tags: {}); using the manifest's last value",
            tags_for_log(&lights[index])
        );
    }
    for primitive in &inventory.stubbed_primitives {
        log::info!(
            "[prl-build] light membership: data-script evaluation stubbed primitive {primitive}"
        );
    }
}

/// The runtime trigger Transform: `(min + max) * 0.5` in `f32`.
fn aabb_center(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|axis| (min[axis] + max[axis]) * 0.5)
}

fn vec3(origin: glam::DVec3) -> [f32; 3] {
    [origin.x as f32, origin.y as f32, origin.z as f32]
}

fn light_type_name(light_type: LightType) -> &'static str {
    match light_type {
        LightType::Point => "Point",
        LightType::Spot => "Spot",
        LightType::Directional => "Directional",
    }
}

fn falloff_model_name(model: FalloffModel) -> &'static str {
    match model {
        FalloffModel::Linear => "Linear",
        FalloffModel::InverseDistance => "InverseDistance",
        FalloffModel::InverseSquared => "InverseSquared",
    }
}

fn tags_for_log(light: &MapLight) -> String {
    if light.tags.is_empty() {
        "<none>".to_owned()
    } else {
        light.tags.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
    use crate::map_data::{FalloffModel, ShadowType};
    use glam::DVec3;
    use postretro_level_format::light_membership::LightMembershipRecord;

    fn light(dynamic: bool) -> MapLight {
        MapLight {
            origin: DVec3::new(1.0, 2.0, 3.0),
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 12.0,
            light_size: 0.25,
            angular_diameter: 0.5,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: dynamic,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![if dynamic { "dynamic" } else { "wave" }.to_owned()],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn manifest(records: Vec<LightMembershipRecord>) -> LightMembershipManifest {
        LightMembershipManifest::new(records, vec!["fireTick".to_owned()])
    }

    fn record(index: u32, dynamic: bool, start_active: Option<bool>) -> LightMembershipRecord {
        LightMembershipRecord {
            index,
            is_dynamic: dynamic,
            start_active,
            start_active_conflict: false,
        }
    }

    #[test]
    fn static_manifest_target_matches_explicit_animated_membership() {
        let mut derived = vec![light(false)];
        let mut flagged = vec![light(false)];
        flagged[0].is_animated = true;
        flagged[0].animation = Some(animated_light_placeholder(true));

        let inventory = apply_manifest(
            &mut derived,
            &[true],
            &manifest(vec![record(0, false, None)]),
        )
        .expect("valid static record applies");

        assert_eq!(inventory.derived_static_indices, vec![0]);
        assert!(StaticBakedLights::from_lights(&derived).is_empty());
        assert_eq!(
            AnimatedBakedLights::from_lights(&derived).len(),
            AnimatedBakedLights::from_lights(&flagged).len(),
            "script-derived and _animated lights must reserve identical animated structures"
        );
        assert_eq!(derived[0].animation, flagged[0].animation);
    }

    #[test]
    fn light_table_snapshot_matches_runtime_initial_animation_state() {
        let mut lights = vec![light(false)];
        lights[0].animation = Some(animated_light_placeholder(false));

        let table = light_table_from_lights(&lights).expect("light table builds");

        assert!(
            table.lights[0].component.animation.is_none(),
            "setupLevel must see the runtime LightBridge initial snapshot, not compiler bake curves"
        );
    }

    #[test]
    fn flagged_target_uses_script_resolved_start_active_in_place() {
        let mut lights = vec![light(false)];
        lights[0].is_animated = true;
        lights[0].animation = Some(animated_light_placeholder(false));

        apply_manifest(
            &mut lights,
            &[false],
            &manifest(vec![record(0, false, Some(true))]),
        )
        .expect("valid flagged record applies");

        assert!(lights[0].animation.as_ref().unwrap().start_active);
    }

    #[test]
    fn trigger_only_static_target_preserves_authored_inactive_default() {
        let mut lights = vec![light(false)];

        apply_manifest(
            &mut lights,
            &[false],
            &manifest(vec![record(0, false, None)]),
        )
        .expect("trigger-only record applies");

        assert!(!lights[0].animation.as_ref().unwrap().start_active);
    }

    #[test]
    fn dynamic_manifest_target_creates_no_bake_membership() {
        let mut lights = vec![light(true)];
        let inventory = apply_manifest(
            &mut lights,
            &[true],
            &manifest(vec![record(0, true, Some(false))]),
        )
        .expect("dynamic record is normal");

        assert_eq!(inventory.dynamic_target_indices, vec![0]);
        assert!(lights[0].animation.is_none());
        assert!(AnimatedBakedLights::from_lights(&lights).is_empty());
    }

    #[test]
    fn inventory_captures_flag_only_dynamic_conflicts_and_stubs() {
        let mut lights = vec![light(false), light(true)];
        lights[0].is_animated = true;
        lights[0].animation = Some(animated_light_placeholder(true));
        let mut dynamic_record = record(1, true, None);
        dynamic_record.start_active_conflict = true;
        let manifest =
            LightMembershipManifest::new(vec![dynamic_record], vec!["spawnParticle".to_owned()]);

        let inventory =
            apply_manifest(&mut lights, &[true, true], &manifest).expect("manifest applies");

        assert_eq!(inventory.flag_only_indices, vec![0]);
        assert_eq!(inventory.dynamic_target_indices, vec![1]);
        assert_eq!(inventory.start_active_conflict_indices, vec![1]);
        assert_eq!(inventory.stubbed_primitives, ["spawnParticle"]);
    }

    #[test]
    fn manifest_rejects_stale_versions_and_invalid_indices() {
        let mut stale = manifest(vec![]);
        stale.version = 0;
        assert!(
            apply_manifest(&mut [], &[], &stale)
                .unwrap_err()
                .to_string()
                .contains("invalid light-membership manifest")
        );

        let mut lights = vec![light(false)];
        let error = apply_manifest(
            &mut lights,
            &[true],
            &manifest(vec![record(4, false, None)]),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("map-light index 4"));
    }

    #[test]
    fn manifest_rejects_dynamic_tier_mismatch() {
        let mut lights = vec![light(false)];
        let error = apply_manifest(&mut lights, &[true], &manifest(vec![record(0, true, None)]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("reports isDynamic=true"));
    }

    #[test]
    fn manifest_rejects_duplicate_records() {
        let mut lights = vec![light(false)];
        let error = apply_manifest(
            &mut lights,
            &[true],
            &manifest(vec![record(0, false, None), record(0, false, None)]),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("duplicate record"));
    }

    #[test]
    fn light_table_uses_map_indices_and_script_facing_snapshots() {
        let table = light_table_from_lights(&[light(false)]).expect("table builds");
        assert_eq!(table.version, LightTable::VERSION);
        assert_eq!(table.lights[0].index, 0);
        assert_eq!(table.lights[0].tags, ["wave"]);
        assert_eq!(table.lights[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(table.lights[0].component.light_type, "Point");
        assert_eq!(table.lights[0].component.falloff_model, "InverseSquared");
    }

    #[test]
    fn light_table_omits_bake_only_lights_but_keeps_raw_source_indices() {
        // Regression: exposing a bake-only sibling shifted script-derived ids
        // away from the compact AlphaLights/runtime query order.
        let mut bake_only = light(false);
        bake_only.bake_only = true;
        bake_only.tags = vec!["bake-only".to_string()];
        let mut runtime_light = light(false);
        runtime_light.tags = vec!["runtime".to_string()];

        let table = light_table_from_lights(&[bake_only, runtime_light]).expect("table builds");

        assert_eq!(table.lights.len(), 1);
        assert_eq!(table.lights[0].index, 1);
        assert_eq!(table.lights[0].tags, ["runtime"]);
    }

    /// Parse `map_text` through the real `.map` front end.
    fn parse_inline_map(label: &str, map_text: &str) -> crate::map_data::MapData {
        let path = std::env::temp_dir().join(format!(
            "postretro-map-members-{label}-{}.map",
            std::process::id()
        ));
        std::fs::write(&path, format!("{}\n", map_text.trim())).expect("write fixture map");
        let parsed = crate::parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        parsed.expect("fixture map parses")
    }

    fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
        let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
        format!(
            "{{\n\
             ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1\n\
             ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1\n\
             ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1\n\
             ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1\n\
             ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1\n\
             ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1\n\
             }}"
        )
    }

    fn worldspawn() -> String {
        format!(
            "// entity 0\n{{\n\"classname\" \"worldspawn\"\n{}\n}}\n",
            box_brush([-512, -512, -512], [-448, -448, -448], "static_tex")
        )
    }

    /// A mover, a trigger volume, a switch, a spawner, and a point entity the
    /// runtime does not place as a member, in that authored order.
    fn member_map() -> String {
        let mover = box_brush([-32, -32, -16], [32, 32, 16], "mover_tex");
        let trigger = box_brush([256, 0, 0], [288, 32, 64], "trigger_tex");
        let switch = box_brush([512, 0, 0], [544, 32, 32], "switch_tex");
        format!(
            r#"{world}// entity 1
{{
"classname" "kinematic_mover"
"name" "lift_a"
"path" "wp_a"
"_tags" "lift arena"
{mover}
}}
// entity 2
{{
"classname" "kinematic_waypoint"
"name" "wp_a"
"next" "wp_b"
"origin" "0 0 0"
}}
// entity 3
{{
"classname" "kinematic_waypoint"
"name" "wp_b"
"next" ""
"origin" "0 0 64"
}}
// entity 4
{{
"classname" "trigger_volume"
"name" "plate"
"on_fire" "open_gate"
"_tags" "plate"
{trigger}
}}
// entity 5
{{
"classname" "switch"
"name" "panel"
"on_fire" "open_door"
"_tags" "panel"
{switch}
}}
// entity 6
{{
"classname" "entity_spawner"
"origin" "64 32 16"
"archetype" "cultist"
"count" "2"
"_tags" "closet"
"spawned_tags" "  wave_1 wave_2 "
}}
// entity 7
{{
"classname" "info_marker"
"origin" "8 8 8"
"_tags" "closet"
"spawned_tags" "not_a_spawner"
}}
"#,
            world = worldspawn()
        )
    }

    // Regression: build-side mover, trigger and spawner queries answered `[]`
    // because `prl-build` sent no member table, so a level script indexing a
    // member failed the map compile. The table now carries exactly the
    // members runtime places, at the positions and with the tags runtime
    // gives them.
    #[test]
    fn parsed_map_members_mirror_runtime_placement() {
        let map = parse_inline_map("members", &member_map());
        let members = map_members_from_map(
            &map.kinematic_movers,
            &map.trigger_volumes,
            &map.map_entities,
        );

        let kinds: Vec<_> = members.iter().map(|member| member.kind).collect();
        assert_eq!(
            kinds,
            [
                MapMemberKind::KinematicMover,
                MapMemberKind::TriggerVolume,
                MapMemberKind::TriggerVolume,
                MapMemberKind::Spawner,
            ],
            "a switch is a trigger volume; an unhandled classname is no member"
        );
        let tags: Vec<_> = members.iter().map(|member| member.tags.clone()).collect();
        assert_eq!(
            tags,
            [
                vec!["lift".to_string(), "arena".to_string()],
                vec!["plate".to_string()],
                vec!["panel".to_string()],
                vec!["closet".to_string()],
            ]
        );
        assert_eq!(members[3].spawned_tags, ["wave_1", "wave_2"]);
        assert!(
            members[..3]
                .iter()
                .all(|member| member.spawned_tags.is_empty())
        );

        // Runtime Transform sources: the packed mover origin, the trigger
        // AABB center (after switch use-reach growth), the spawner origin.
        assert_eq!(members[0].position, vec3(map.kinematic_movers[0].origin));
        for (member, trigger) in members[1..3].iter().zip(&map.trigger_volumes) {
            let min = glam::Vec3::from(trigger.aabb_min);
            let max = glam::Vec3::from(trigger.aabb_max);
            assert_eq!(member.position, ((min + max) * 0.5).to_array());
        }
        let spawner = map
            .map_entities
            .iter()
            .find(|entity| entity.classname == ENTITY_SPAWNER_CLASSNAME)
            .expect("spawner record");
        assert_eq!(members[3].position, vec3(spawner.origin));
        assert_ne!(members[3].position, [0.0; 3]);

        let json = serde_json::to_value(
            light_table_from_lights(&map.lights)
                .expect("table builds")
                .with_map_members(members),
        )
        .expect("table serializes");
        let wire_kinds: Vec<_> = json["mapMembers"]
            .as_array()
            .expect("mapMembers present")
            .iter()
            .map(|member| member["kind"].as_str().expect("kind").to_owned())
            .collect();
        assert_eq!(
            wire_kinds,
            [
                "kinematic_mover",
                "trigger_volume",
                "trigger_volume",
                "spawner"
            ]
        );
        assert_eq!(json["mapMembers"][3]["spawnedTags"][1], "wave_2");
        assert!(json["mapMembers"][0].get("spawnedTags").is_none());
    }

    // A map with no movers, triggers or spawners sends the same light-table
    // bytes as before the member table existed.
    #[test]
    fn map_without_members_sends_a_lights_only_light_table() {
        let map = parse_inline_map("no-members", &worldspawn());
        let members = map_members_from_map(
            &map.kinematic_movers,
            &map.trigger_volumes,
            &map.map_entities,
        );
        assert!(members.is_empty());
        let table = light_table_from_lights(&map.lights)
            .expect("table builds")
            .with_map_members(members);
        assert_eq!(
            serde_json::to_vec(&table).expect("table serializes"),
            br#"{"version":1,"lights":[]}"#
        );
    }
}
