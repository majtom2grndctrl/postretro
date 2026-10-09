// Per-shot observer weapon cue publication, delivery, and observer impact bursts.
// See: context/lib/networking.md §Combat authority · context/lib/audio.md §4
use crate::{App, netcode};
use postretro_entities::{AiCue, AiEmission, Emitter, WeaponEmission};
use postretro_foundation::ShotId;
use postretro_net::wire::NetworkId;

impl App {
    /// Burst this frame's received impact cues, once per contact. Only a
    /// connected client receives cues; the host and single player burst from
    /// their own simulation and HIT ingestion, so this route never doubles them.
    pub(crate) fn spawn_observer_impact_bursts(
        &self,
        registry: &mut postretro_entities::EntityRegistry,
    ) {
        if self.is_connected_client() {
            netcode::weapon_cues::spawn_observer_impact_bursts(
                registry,
                &self.observer_weapon_cues,
            );
        }
    }

    /// Publish every tick before producer ownership/identity can retire. Remote
    /// HIT contacts have already been published by host ingestion and are not in
    /// this batch. Gameplay projectile contacts retain the normalized launch id.
    pub(crate) fn publish_observer_weapon_cues(
        &mut self,
        weapons: &[WeaponEmission],
        ai: &[AiEmission],
    ) {
        let Some(netcode::NetEndpoint::Host {
            server,
            allocator,
            owners,
            ..
        }) = self.session.as_mut().and_then(|s| s.net_endpoint.as_mut())
        else {
            return;
        };
        let mut produced = Vec::<(ShotId, ShotId, Option<u64>)>::new();
        for emission in weapons.iter().filter(|e| e.address == "activate") {
            let (Some(raw), Emitter::Entity { id: pawn, .. }) =
                (emission.shot_id, &emission.emitter)
            else {
                continue;
            };
            let shot = netcode::weapon_cues::observer_shot_id(allocator, *pawn, raw);
            let owner = owners.owner_of(*pawn);
            produced.push((raw, shot, owner));
            if let Some(cue) = netcode::weapon_cues::freeze_observer_weapon_cue(
                shot,
                NetworkId(shot.pawn),
                netcode::weapon_cues::WeaponCueKind::Activate,
                emission.action.as_deref(),
                emission.sounds.as_ref().and_then(|s| s.fire.as_deref()),
                None,
                &emission.emitter,
                allocator,
            ) {
                netcode::weapon_cues::send_observer_weapon_cue_to_server(server, owner, cue);
            }
        }
        for emission in ai {
            let (Some(raw), Emitter::Entity { id: pawn, .. }) =
                (emission.shot_id, &emission.emitter)
            else {
                continue;
            };
            let shot = netcode::weapon_cues::observer_shot_id(allocator, *pawn, raw);
            let owner = owners.owner_of(*pawn);
            produced.push((raw, shot, owner));
            let extra = match &emission.cue {
                AiCue::Attack { graph, attack } => {
                    graph.attacks.get(attack).and_then(|a| a.sound.as_deref())
                }
                _ => None,
            };
            if let Some(cue) = netcode::weapon_cues::freeze_observer_weapon_cue(
                shot,
                NetworkId(shot.pawn),
                netcode::weapon_cues::WeaponCueKind::Activate,
                emission.action.as_deref(),
                emission.sounds.as_ref().and_then(|s| s.fire.as_deref()),
                extra,
                &emission.emitter,
                allocator,
            ) {
                netcode::weapon_cues::send_observer_weapon_cue_to_server(server, owner, cue);
            }
        }
        for emission in weapons.iter().filter(|e| e.address == "impact") {
            let Some(raw) = emission.shot_id else {
                continue;
            };
            // Same-tick hitscan uses the explicitly identified producer above.
            // Later gameplay flights were normalized at mirror registration;
            // they are local/AI authoritative flights (remote flights use HIT).
            let (shot, owner) = produced
                .iter()
                .find(|(id, _, _)| *id == raw)
                .map(|(_, shot, owner)| (*shot, *owner))
                .unwrap_or((raw, None));
            if let Some(cue) = netcode::weapon_cues::freeze_observer_weapon_cue(
                shot,
                NetworkId(shot.pawn),
                netcode::weapon_cues::WeaponCueKind::Impact,
                emission.action.as_deref(),
                emission.sounds.as_ref().and_then(|s| s.impact.as_deref()),
                None,
                &emission.emitter,
                allocator,
            ) {
                netcode::weapon_cues::send_observer_weapon_cue_to_server(server, owner, cue);
            }
        }
    }
}
