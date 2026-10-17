// Arrival cues use captured points, independently of delayed pawn presentation.
// See: context/lib/audio.md §4 (Teleporters)

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};

pub(crate) fn arrival_sound(
    teleporters: &postretro_physics::teleporters::Teleporters,
    index: u16,
    arrival: Vec3,
) -> Option<SoundRequest> {
    let record = teleporters.records().get(usize::from(index))?;
    if record.arrival_sound.is_empty() {
        return None;
    }
    Some(SoundRequest {
        bus: "sfx".to_owned(),
        sound: record.arrival_sound.clone(),
        looping: false,
        anchor: Some(SoundAnchor::Point(arrival.to_array())),
    })
}

/// One cue per accepted remote generation change, at the sample's fixed arrival
/// point. Netcode already coalesced each pawn's changes to its last sample and
/// kept first samples silent; an entity anchor would resolve the delayed pose.
pub(crate) fn observed_arrival_sounds(
    teleporters: &postretro_physics::teleporters::Teleporters,
    observed: &[postretro_netcode::ObservedTeleport],
    sounds: &mut Vec<SoundRequest>,
) {
    sounds.extend(
        observed
            .iter()
            .filter_map(|cue| arrival_sound(teleporters, cue.last_teleporter, cue.arrival)),
    );
}

/// Name every installed teleporter's `arrival_sound` for the unknown-key check
/// run at level install and committed hot reload (`audio.md` §4). Empty means
/// none and names nothing.
pub(super) fn name_arrival_sounds(
    registry: &postretro_entities::EntityRegistry,
    name: &mut impl FnMut(&str, String),
) {
    for (_, value) in registry.iter_with_kind(postretro_entities::ComponentKind::Teleporter) {
        let postretro_entities::ComponentValue::Teleporter(teleporter) = value else {
            continue;
        };
        if !teleporter.arrival_sound.is_empty() {
            name(
                &teleporter.arrival_sound,
                format!("teleporter {} `arrival_sound`", teleporter.source_entity),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use postretro_entities::{ScriptCtx, TeleporterComponent, Transform};
    use postretro_test_log_capture::LogCapture;

    fn install(ctx: &ScriptCtx, source_entity: u32, arrival_sound: &str) {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TeleporterComponent {
                    index: source_entity as u16,
                    source_entity,
                    armed: true,
                    arrival_sound: arrival_sound.to_string(),
                    on_teleport: String::new(),
                },
            )
            .unwrap();
    }

    // audio.md §4: an unknown key named by any authored sound field warns once
    // after the registry loads, and the level still installs.
    #[test]
    fn teleport_arrival_sound_keys_join_the_unknown_sound_key_check() {
        let ctx = ScriptCtx::new();
        install(&ctx, 3, "sfx/teleport_missing");
        install(&ctx, 4, "fixtures/jump");
        install(&ctx, 5, "");
        // A second teleporter naming the same unknown key still warns once.
        install(&ctx, 6, "sfx/teleport_missing");

        let capture = LogCapture::start();
        let unknown = super::super::warn_unknown_sound_keys(&ctx, |key| key == "fixtures/jump");
        assert_eq!(unknown, ["sfx/teleport_missing"]);
        capture.assert_logged_once(
            log::Level::Warn,
            "unknown sound key `sfx/teleport_missing` (named by teleporter 3 `arrival_sound`)",
        );
        capture.assert_not_logged(log::Level::Warn, "`fixtures/jump`");
        capture.assert_not_logged(log::Level::Warn, "unknown sound key ``");
    }
}
