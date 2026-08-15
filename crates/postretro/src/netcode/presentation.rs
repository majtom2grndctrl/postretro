// Host-to-client routing and engine conversion for passive presentation events.
// See: context/lib/networking.md

use std::collections::BTreeMap;

use glam::Vec3;
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationFade, PresentationMotion,
    PresentationSpawn, PresentationTemplateHandle,
};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    PresentationFact as WirePresentationFact, ServerPresentationMessage, ServerPresentationPayload,
};
use postretro_scripting_core::data_descriptors::PresentationTemplate;

use super::MovementOwners;

/// Drain the host's presentation intake once per frame and route each transient
/// to exactly one screen. A remote pawn owner receives one unreliable packet;
/// host-owned, absent, and non-pawn presenters remain host-local.
pub(crate) fn route_host_presentation_spawns(
    registry: &mut EntityRegistry,
    server: &mut NetServer,
    owners: &MovementOwners,
) {
    let spawns = registry.take_presentation_spawns();
    for spawn in spawns {
        match presentation_recipient(&spawn, owners) {
            Some(client_id) => {
                // A failed addressed send is intentionally a dropped cosmetic.
                let _ =
                    server.send_presentation(client_id, presentation_message_from_spawn(&spawn));
            }
            None => registry.push_presentation_spawn(spawn),
        }
    }
}

/// Convert received one-shot presentation events into the client-local intake.
/// Overlay facts are deliberately retained for their dedicated Task 8 ingest
/// path; no fallback presentation is invented for an unsupported payload.
pub(crate) fn ingest_client_presentation_messages(
    registry: &mut EntityRegistry,
    messages: Vec<ServerPresentationMessage>,
    templates: &[PresentationTemplate],
) {
    for message in messages {
        let ServerPresentationPayload::Spawn {
            template_id,
            anchor,
            value,
            facts,
        } = message.payload
        else {
            continue;
        };
        let Some(template) = templates.iter().find(|template| template.id == template_id) else {
            continue;
        };

        let mut facts = facts
            .into_iter()
            .map(|(name, fact)| (name, presentation_fact_from_wire(fact)))
            .collect::<BTreeMap<_, _>>();
        // `value` is the conventional impact fact. Preserve an explicitly
        // stamped same-named fact if a future producer supplies one.
        facts
            .entry("value".to_string())
            .or_insert(PresentationFact::Number(value));

        registry.push_presentation_spawn(PresentationSpawn {
            world_anchor: Vec3::new(anchor[0], anchor[1], anchor[2]),
            template: PresentationTemplateHandle::from(template.id.clone()),
            facts,
            presenter: None,
            lifetime_seconds: template.lifetime_ms as f32 / 1_000.0,
            motion: PresentationMotion {
                rise_pixels: template.motion.rise,
                easing: template.motion.easing,
            },
            fade: PresentationFade {
                duration_seconds: template.lifetime_ms.saturating_sub(template.fade.start_ms)
                    as f32
                    / 1_000.0,
            },
            scatter_radius: template.spawn_scatter.radius,
        });
    }
}

fn presentation_recipient(spawn: &PresentationSpawn, owners: &MovementOwners) -> Option<u64> {
    spawn
        .presenter
        .map(|presenter| EntityId::from_raw(presenter.0))
        .and_then(|source| owners.owner_of(source))
}

fn presentation_message_from_spawn(spawn: &PresentationSpawn) -> ServerPresentationMessage {
    ServerPresentationMessage {
        payload: ServerPresentationPayload::Spawn {
            template_id: spawn.template.0.clone(),
            anchor: spawn.world_anchor.to_array(),
            value: match spawn.facts.get("value") {
                Some(PresentationFact::Number(value)) => *value,
                _ => 0.0,
            },
            facts: spawn
                .facts
                .iter()
                .map(|(name, fact)| (name.clone(), presentation_fact_to_wire(fact)))
                .collect(),
        },
    }
}

fn presentation_fact_to_wire(fact: &PresentationFact) -> WirePresentationFact {
    match fact {
        PresentationFact::Number(value) => WirePresentationFact::Number(*value),
        PresentationFact::Text(value) => WirePresentationFact::Text(value.clone()),
        PresentationFact::Bool(value) => WirePresentationFact::Bool(*value),
    }
}

fn presentation_fact_from_wire(fact: WirePresentationFact) -> PresentationFact {
    match fact {
        WirePresentationFact::Number(value) => PresentationFact::Number(value),
        WirePresentationFact::Text(value) => PresentationFact::Text(value),
        WirePresentationFact::Bool(value) => PresentationFact::Bool(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{PresentationPresenter, PresentationTemplateHandle};
    use postretro_foundation::PresentationEasing;
    use postretro_scripting_core::data_descriptors::{
        PresentationTemplateFade, PresentationTemplateMotion, PresentationTemplateSpawnScatter,
    };

    fn spawn(presenter: Option<EntityId>) -> PresentationSpawn {
        PresentationSpawn {
            world_anchor: Vec3::new(1.0, 2.0, 3.0),
            template: PresentationTemplateHandle::from("damage-number"),
            facts: BTreeMap::from([
                ("value".to_string(), PresentationFact::Number(40.0)),
                ("critical".to_string(), PresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    PresentationFact::Text("critical hit".to_string()),
                ),
            ]),
            presenter: presenter.map(|id| PresentationPresenter(id.to_raw())),
            lifetime_seconds: 0.9,
            motion: PresentationMotion {
                rise_pixels: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationFade {
                duration_seconds: 0.4,
            },
            scatter_radius: 0.15,
        }
    }

    fn template() -> PresentationTemplate {
        PresentationTemplate {
            id: "damage-number".to_string(),
            root: postretro_scripting_core::ui::descriptor::Widget::Spacer(
                postretro_scripting_core::ui::descriptor::SpacerWidget {
                    flex_grow: 0.0,
                    id: None,
                    visible_when: None,
                    role: None,
                },
            ),
            lifetime_ms: 900,
            motion: PresentationTemplateMotion {
                rise: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationTemplateFade { start_ms: 500 },
            spawn_scatter: PresentationTemplateSpawnScatter { radius: 0.15 },
        }
    }

    #[test]
    fn presentation_recipient_addresses_only_the_owning_remote_pawn() {
        let remote = EntityId::from_raw(7);
        let host_pawn = EntityId::from_raw(8);
        let mut owners = MovementOwners::new();
        owners.set(remote, 41);

        assert_eq!(
            presentation_recipient(&spawn(Some(remote)), &owners),
            Some(41)
        );
        assert_eq!(
            presentation_recipient(&spawn(Some(host_pawn)), &owners),
            None
        );
        assert_eq!(presentation_recipient(&spawn(None), &owners), None);
    }

    #[test]
    fn spawn_message_preserves_template_anchor_value_and_all_facts() {
        let original = spawn(Some(EntityId::from_raw(7)));
        let message = presentation_message_from_spawn(&original);
        let ServerPresentationPayload::Spawn {
            template_id,
            anchor,
            value,
            facts,
        } = message.payload
        else {
            panic!("spawn conversion must produce a Spawn payload");
        };

        assert_eq!(template_id, original.template.0);
        assert_eq!(anchor, original.world_anchor.to_array());
        assert_eq!(value, 40.0);
        assert_eq!(
            facts,
            BTreeMap::from([
                ("value".to_string(), WirePresentationFact::Number(40.0)),
                ("critical".to_string(), WirePresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    WirePresentationFact::Text("critical hit".to_string()),
                ),
            ])
        );
    }

    #[test]
    fn client_spawn_ingest_restores_local_pool_values_without_presenter_identity() {
        let mut registry = EntityRegistry::new();
        ingest_client_presentation_messages(
            &mut registry,
            vec![presentation_message_from_spawn(&spawn(Some(
                EntityId::from_raw(7),
            )))],
            &[template()],
        );

        let spawned = registry.take_presentation_spawns();
        assert_eq!(spawned.len(), 1);
        let spawned = &spawned[0];
        assert_eq!(spawned.world_anchor, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(spawned.template.0, "damage-number");
        assert_eq!(
            spawned.facts.get("value"),
            Some(&PresentationFact::Number(40.0))
        );
        assert_eq!(
            spawned.facts.get("critical"),
            Some(&PresentationFact::Bool(true))
        );
        assert_eq!(
            spawned.facts.get("label"),
            Some(&PresentationFact::Text("critical hit".to_string()))
        );
        assert_eq!(spawned.presenter, None);
        assert_eq!(spawned.lifetime_seconds, 0.9);
        assert_eq!(spawned.motion.rise_pixels, 12.0);
        assert_eq!(spawned.fade.duration_seconds, 0.4);
        assert_eq!(spawned.scatter_radius, 0.15);
    }
}
