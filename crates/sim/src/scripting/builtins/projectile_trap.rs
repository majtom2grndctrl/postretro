//! Built-in classname handler for `projectile_trap`: keeps the authored configuration for install.
//! See: context/lib/build_pipeline.md §Custom FGD · context/lib/entity_model.md §Components

use glam::Vec3;

use super::MapEntity;
use postretro_entities::components::projectile_trap::{
    DEFAULT_TRAP_INTERVAL_MS, ProjectileTrapComponent,
};
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};

/// FGD `classname` this handler binds to.
pub const CLASSNAME: &str = "projectile_trap";

/// One optional KVP's parse. A blank value is unset and takes the default.
fn optional_value<'a>(entity: &'a MapEntity, key: &str) -> Option<&'a str> {
    entity
        .key_values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// The first invalid timing KVP, described for the one inert warning.
fn timing_from_entity(entity: &MapEntity) -> Result<(f64, f64, bool), String> {
    let number = |key: &str, default: f64| -> Result<f64, String> {
        match optional_value(entity, key) {
            None => Ok(default),
            Some(raw) => raw
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("`{key}` `{raw}` is not a finite number")),
        }
    };
    let interval_ms = number("interval_ms", DEFAULT_TRAP_INTERVAL_MS)?;
    if interval_ms <= 0.0 {
        return Err(format!("`interval_ms` {interval_ms} must be positive"));
    }
    let phase_ms = number("phase_ms", 0.0)?;
    if phase_ms < 0.0 {
        return Err(format!("`phase_ms` {phase_ms} must not be negative"));
    }
    let start_on_spawn = match optional_value(entity, "start_on_spawn") {
        None | Some("0") | Some("false") | Some("False") => false,
        Some("1") | Some("true") | Some("True") => true,
        Some(other) => return Err(format!("`start_on_spawn` `{other}` must be 0 or 1")),
    };
    Ok((interval_ms, phase_ms, start_on_spawn))
}

/// Spawn the trap at its origin. Its rotation is the view frame (forward `-Z`
/// along the map arrow): a trap has no model, and it fires along that axis.
pub fn handle(entity: &MapEntity, registry: &mut EntityRegistry) -> Option<EntityId> {
    let transform = Transform {
        position: entity.origin,
        rotation: entity.rotation_quat(),
        scale: Vec3::ONE,
    };
    let id = registry.try_spawn(transform, &entity.tags).or_else(|| {
        log::warn!(
            "[Loader] {}: entity registry exhausted; dropping projectile_trap",
            entity.diagnostic_origin()
        );
        None
    })?;
    let weapon = entity
        .key_values
        .get("weapon")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let component = match timing_from_entity(entity) {
        Ok((interval_ms, phase_ms, start_on_spawn)) => {
            ProjectileTrapComponent::new(weapon, interval_ms, phase_ms, start_on_spawn)
        }
        Err(reason) => {
            log::warn!(
                "[Loader] {}: {reason}; projectile_trap is inert",
                entity.diagnostic_origin()
            );
            ProjectileTrapComponent {
                timing_valid: false,
                ..ProjectileTrapComponent::new(weapon, DEFAULT_TRAP_INTERVAL_MS, 0.0, false)
            }
        }
    };
    let _ = registry.set_component(id, component);
    Some(id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn entity_with_kvps(pairs: &[(&str, &str)]) -> MapEntity {
        MapEntity {
            map_ordinal: None,
            classname: CLASSNAME.to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            key_values: pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
            tags: vec!["wave".to_string()],
        }
    }

    fn parsed(pairs: &[(&str, &str)]) -> ProjectileTrapComponent {
        let mut registry = EntityRegistry::new();
        let id = handle(&entity_with_kvps(pairs), &mut registry).expect("trap spawns");
        assert_eq!(registry.get_tags(id).unwrap(), &["wave"]);
        registry
            .get_component::<ProjectileTrapComponent>(id)
            .unwrap()
            .clone()
    }

    #[test]
    fn projectile_trap_kvps_default_to_a_silent_one_second_trap() {
        let trap = parsed(&[("weapon", "enemy_rifle")]);
        assert_eq!(
            trap,
            ProjectileTrapComponent::new("enemy_rifle".into(), 1000.0, 0.0, false)
        );
        let blank = parsed(&[
            ("weapon", "enemy_rifle"),
            ("interval_ms", " "),
            ("phase_ms", ""),
            ("start_on_spawn", ""),
        ]);
        assert_eq!(blank, trap, "a blank optional KVP takes its default");
        let authored = parsed(&[
            ("weapon", " enemy_rifle "),
            ("interval_ms", "250"),
            ("phase_ms", "40"),
            ("start_on_spawn", "1"),
        ]);
        assert_eq!(
            authored,
            ProjectileTrapComponent::new("enemy_rifle".into(), 250.0, 40.0, true)
        );
    }

    #[test]
    fn projectile_trap_invalid_timing_kvp_is_inert_with_one_warning() {
        for pairs in [
            [("interval_ms", "0")],
            [("interval_ms", "-5")],
            [("interval_ms", "NaN")],
            [("interval_ms", "inf")],
            [("phase_ms", "-1")],
            [("phase_ms", "soon")],
            [("start_on_spawn", "yes")],
        ] {
            let capture = postretro_test_log_capture::LogCapture::start();
            let trap = parsed(&pairs);
            assert!(!trap.timing_valid, "{pairs:?} leaves the trap inert");
            assert!(!trap.start_on_spawn);
            capture.assert_logged_once(log::Level::Warn, "projectile_trap is inert");
        }
    }

    // The view frame: forward `-Z` follows the map arrow, so yaw 90 (Quake `+Y`)
    // fires along engine `-X`.
    #[test]
    fn projectile_trap_rotation_is_the_view_frame() {
        let mut entity = entity_with_kvps(&[("weapon", "enemy_rifle")]);
        entity.angles = Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0);
        let mut registry = EntityRegistry::new();
        let id = handle(&entity, &mut registry).unwrap();
        let forward = registry.get_component::<Transform>(id).unwrap().rotation * Vec3::NEG_Z;
        assert!(forward.abs_diff_eq(Vec3::NEG_X, 1e-6), "forward {forward}");
    }
}
