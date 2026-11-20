// Status presentation on every machine, from each entity's status presence:
// an emitter that follows the afflicted entity (never this machine's own
// pawn) and a one-shot tick sound every status tick from when presence appears.
// No emitter crosses the wire; each machine attaches its own.
// See: context/lib/scripting.md §2 (Status declarations) · context/lib/audio.md §4

use std::sync::Arc;

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::hit_volume::HitVolumeComponent;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::defense::DefenseVocabulary;

/// Most tick sounds one track requests in one frame, so a long stall never
/// floods the mixer with a backlog.
const MAX_TICK_SOUNDS_PER_FRAME: u32 = 2;

/// One fixed tick in milliseconds. Tick sounds space by the status's whole
/// fixed ticks, the period its damage ticks on, not by raw `tickMs`.
const FIXED_TICK_MS: f32 = crate::frame_timing::TICK_DURATION.as_secs_f32() * 1000.0;

/// One status emitter attached to one afflicted entity this frame, at the
/// entity's body centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AttachedEmitter {
    pub(crate) entity: EntityId,
    pub(crate) status: u16,
    pub(crate) origin: Vec3,
}

/// One (entity, status) whose presence holds: the time since presence
/// appeared and when its next tick sound is due.
#[derive(Debug, Clone, Copy)]
struct Track {
    entity: EntityId,
    status: u16,
    elapsed_ms: f32,
    next_sound_ms: f32,
    live: bool,
}

/// Per-machine status presentation state. Tracks and scratch are reused
/// frame to frame; they grow only when more entities are afflicted.
#[derive(Default)]
pub(crate) struct StatusPresentation {
    tracks: Vec<Track>,
    attached: Vec<AttachedEmitter>,
    candidates: Vec<EntityId>,
    /// Each status's emitter by status index, parsed once per committed
    /// vocabulary.
    emitters: Vec<Option<BillboardEmitterComponent>>,
    vocabulary: Option<Arc<DefenseVocabulary>>,
}

impl StatusPresentation {
    /// The emitters attached this frame.
    pub(crate) fn attached(&self) -> &[AttachedEmitter] {
        &self.attached
    }

    /// Each status's emitter, by status index.
    pub(crate) fn emitters(&self) -> &[Option<BillboardEmitterComponent>] {
        &self.emitters
    }

    /// Advance one rendered frame of `dt_ms`. Presence is read through
    /// [`EntityRegistry::shows_status`]: the host's live instances, or a
    /// connected client's replicated presence mapped by name. A track starts
    /// when presence appears, requests its first tick sound one status tick
    /// (`tickMs` rounded up to whole fixed ticks) later and every status tick
    /// after, and ends when presence clears; presence reappearing restarts the
    /// count. `local_pawn` gets no emitter.
    pub(crate) fn update(
        &mut self,
        registry: &EntityRegistry,
        local_pawn: Option<EntityId>,
        dt_ms: f32,
        sounds: &mut Vec<SoundRequest>,
    ) {
        self.refresh_emitters(registry);
        let vocabulary = Arc::clone(registry.defense_vocabulary());
        self.attached.clear();
        for track in &mut self.tracks {
            track.live = false;
        }
        if vocabulary.statuses.is_empty() {
            self.tracks.clear();
            return;
        }

        self.candidates.clear();
        self.candidates
            .extend(registry.statuses().sets().iter().map(|set| set.entity));
        self.candidates.extend(
            registry
                .iter_with_kind(ComponentKind::StatusPresence)
                .map(|(entity, _)| entity),
        );
        for &entity in &self.candidates {
            for (index, status) in vocabulary.statuses.iter().enumerate() {
                let presented = status.presentation.emitter.is_some()
                    || status.presentation.tick_sound.is_some();
                if !presented || !registry.shows_status(entity, &status.name) {
                    continue;
                }
                let index = index as u16;
                match self
                    .tracks
                    .iter_mut()
                    .find(|track| track.entity == entity && track.status == index)
                {
                    Some(track) => track.live = true,
                    None => self.tracks.push(Track {
                        entity,
                        status: index,
                        elapsed_ms: 0.0,
                        next_sound_ms: tick_period_ms(status.tick_ticks),
                        live: true,
                    }),
                }
            }
        }
        self.tracks.retain(|track| track.live);

        for track in &mut self.tracks {
            let Some(status) = vocabulary.statuses.get(usize::from(track.status)) else {
                continue;
            };
            let origin = body_centre(registry, track.entity);
            track.elapsed_ms += dt_ms.max(0.0);
            if let Some(sound) = &status.presentation.tick_sound {
                let mut played = 0;
                while track.elapsed_ms >= track.next_sound_ms {
                    track.next_sound_ms += tick_period_ms(status.tick_ticks);
                    if played < MAX_TICK_SOUNDS_PER_FRAME {
                        sounds.push(SoundRequest {
                            bus: "sfx".to_string(),
                            sound: sound.clone(),
                            looping: false,
                            anchor: Some(SoundAnchor::Entity {
                                key: crate::sound_events::entity_key(track.entity),
                                point: origin.to_array(),
                            }),
                        });
                        played += 1;
                    }
                }
            }
            if Some(track.entity) != local_pawn
                && self
                    .emitters
                    .get(usize::from(track.status))
                    .is_some_and(Option::is_some)
            {
                self.attached.push(AttachedEmitter {
                    entity: track.entity,
                    status: track.status,
                    origin,
                });
            }
        }
    }

    /// Parse each status's `emitter()` once per committed vocabulary.
    fn refresh_emitters(&mut self, registry: &EntityRegistry) {
        let current = registry.defense_vocabulary();
        if self
            .vocabulary
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, current))
        {
            return;
        }
        self.emitters = current
            .statuses
            .iter()
            .map(|status| {
                let emitter = status.presentation.emitter.as_ref()?;
                parse_emitter(emitter)
                    .map_err(|error| {
                        log::warn!(
                            "[Status] `{}` presentation emitter is not an emitter() value; it shows nothing: {error}",
                            status.name
                        )
                    })
                    .ok()
            })
            .collect();
        self.vocabulary = Some(Arc::clone(current));
        // Indices may have moved; tracks restart against the new table.
        self.tracks.clear();
    }
}

/// An `emitter()` builder value as the component it describes.
pub(crate) fn parse_emitter(
    value: &serde_json::Value,
) -> Result<BillboardEmitterComponent, serde_json::Error> {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("kind");
    }
    serde_json::from_value(value)
}

/// A status's tick period: its whole fixed ticks, at least one.
fn tick_period_ms(tick_ticks: u32) -> f32 {
    tick_ticks.max(1) as f32 * FIXED_TICK_MS
}

/// The afflicted entity's body centre, by one rule every machine shares. A
/// movement pawn's transform is its capsule centre. Anything else centres on
/// its authored hitbox: the host holds it on `Health`, a connected client's
/// display copy on `HitVolume` with the same offset, since neither `Health`
/// nor `Agent` reaches a client. Without a hitbox it is the transform, which
/// for an AI body is already its agent capsule's centre.
fn body_centre(registry: &EntityRegistry, entity: EntityId) -> Vec3 {
    let position = registry
        .get_component::<Transform>(entity)
        .map(|transform| transform.position)
        .unwrap_or(Vec3::ZERO);
    if registry
        .has_component_kind(entity, ComponentKind::PlayerMovement)
        .unwrap_or(false)
    {
        return position;
    }
    let hitbox = registry
        .get_component::<HealthComponent>(entity)
        .ok()
        .and_then(|health| health.hitbox)
        .or_else(|| {
            registry
                .get_component::<HitVolumeComponent>(entity)
                .ok()
                .map(|volume| volume.hitbox)
        });
    position + hitbox.map_or(Vec3::ZERO, |hitbox| hitbox.offset)
}

#[cfg(test)]
#[path = "status_presentation_tests.rs"]
mod tests;
