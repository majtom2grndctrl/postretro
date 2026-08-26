//! Compiler-side adaptation of script-derived light membership into bake input.
//!
//! The versioned JSON seam lives in `postretro-level-format`; this module owns
//! the `MapData` mutation that reserves existing animated-bake structures.

use anyhow::{Context as _, Result, bail};
use postretro_level_format::light_membership::{
    LightComponentSnapshot, LightMembershipManifest, LightTable, LightTableLight,
};

use crate::map_data::{FalloffModel, LightType, MapLight, animated_light_placeholder};

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
}
