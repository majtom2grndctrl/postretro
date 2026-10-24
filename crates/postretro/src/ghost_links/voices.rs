// Ghost voices: a second voice on the Ghost bus beside each member sound this machine admits.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/entity_model.md §7d

use std::collections::{HashMap, VecDeque};

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::{
    AiEmission, Emitter, EntityId, EntityRegistry, KinematicMoverComponent, Transform,
    WeaponEmission,
};
use postretro_foundation::{SeamlessMapping, ShotId};

use super::GhostLinks;
use super::clocks::{self, PoseStamps, PresentationClocks};
use super::eligibility::{self, CandidateOrigin, CandidateView, GhostCategory};

/// The boundary bus name every ghost voice plays on.
pub(crate) const GHOST_BUS: &str = "ghost";

/// Ledger entries kept: the bound on memory, not on a projectile's flight. On
/// the host each shot takes two (its raw and its normalized id), so this keeps
/// the last 2048 shots: a battle firing a hundred shots a second keeps a
/// projectile's fire entry through a 20 s flight. An evicted entry only means
/// that shot's impact is not ghosted. Record and lookup are constant time.
const MAX_SHOT_OWNERS: usize = 4096;

/// A sound this machine is about to play, with what ghost admission needs
/// beyond its anchor: an impact's shot owner, which the contact set alone
/// does not carry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalSound {
    pub(crate) request: SoundRequest,
    /// The pawn whose shot made this impact, resolved from the shot id the
    /// way observer cues normalize it. `None` for every other sound, and for an
    /// impact without a shot id or whose shot this machine never saw fired.
    pub(crate) shot_owner: Option<EntityId>,
}

impl LocalSound {
    /// Whether this sound's anchor can ghost at all: an entity or a contact
    /// set. A point-anchored or unanchored sound never does.
    fn may_ghost(&self) -> bool {
        matches!(
            self.request.anchor,
            Some(SoundAnchor::Entity { .. } | SoundAnchor::Contacts(_))
        )
    }
}

impl From<SoundRequest> for LocalSound {
    fn from(request: SoundRequest) -> Self {
        Self {
            request,
            shot_owner: None,
        }
    }
}

/// Per-level ghost-voice state: which entity fired each recent shot, and the
/// sub-tick alpha this frame's admission reads clocks at.
#[derive(Debug, Default)]
pub(super) struct GhostVoices {
    /// Recent shots and their owners. A shot recorded under its raw id and,
    /// on the host, its normalized one. `None` marks an id two different owners
    /// fired under: attributed to neither.
    shot_owners: HashMap<ShotId, Option<EntityId>>,
    /// The ledger's ids, oldest first, for eviction.
    shot_order: VecDeque<ShotId>,
    alpha: f32,
}

/// What admission reads about this frame: where each member presents, and
/// how this machine knows its candidates. Built by the same role selection
/// the membership update uses, so a voice reads a member's link state at the
/// clock its ghost presents at.
pub(crate) struct VoiceFrame<'a> {
    pub clocks: PresentationClocks,
    pub view: CandidateView<'a>,
    pub stamps: PoseStamps<'a>,
}

impl GhostLinks {
    /// Remember that `owner` fired `shot`, so the shot's later impacts can be
    /// attributed. Ids are matched whole and never decoded: a raw local id
    /// (the host's own shots carry pawn 0) is never read as a network id, and
    /// an id two owners fired under belongs to neither, so an impact is never
    /// attributed to an entity that did not fire it.
    pub(crate) fn record_shot_owner(&mut self, shot: ShotId, owner: EntityId) {
        let voices = &mut self.voices;
        if let Some(entry) = voices.shot_owners.get_mut(&shot) {
            if *entry != Some(owner) {
                *entry = None;
            }
            return;
        }
        if voices.shot_order.len() == MAX_SHOT_OWNERS
            && let Some(oldest) = voices.shot_order.pop_front()
        {
            voices.shot_owners.remove(&oldest);
        }
        voices.shot_order.push_back(shot);
        voices.shot_owners.insert(shot, Some(owner));
    }

    /// The entity that fired `shot`, if this machine saw it fired.
    pub(crate) fn shot_owner(&self, shot: Option<ShotId>) -> Option<EntityId> {
        self.voices.shot_owners.get(&shot?).copied().flatten()
    }

    /// Every ghost voice `sound` admits, pushed to `out` (cleared first). The
    /// caller plays them only if `sound`'s own voice was admitted, so a ghost
    /// voice never starts beside a voice this machine refused.
    ///
    /// - An entity-anchored sound ghosts through each armed link whose volume
    ///   holds its fire-time point, whose style carries audio and which shows
    ///   the entity's category at the clock that entity presents at this
    ///   frame. The voice sits at the mapped fire-time point and
    ///   plays out wherever the member goes next.
    /// - An impact (a contact set) ghosts only when its shot's owner is such a
    ///   member, judged at the owner's point, and the style also carries impact
    ///   audio. Contacts outside the volume drop.
    /// - A point-anchored or unanchored sound never ghosts.
    ///
    /// The viewer's own pawn, its projectiles and their sounds never ghost.
    /// Bounded by the links containing one point. Runs per sound, never
    /// per frame: each admitted voice allocates its request, and an impact's
    /// voice its mapped contact list too.
    pub(crate) fn ghost_voices(
        &self,
        registry: &EntityRegistry,
        frame: &VoiceFrame<'_>,
        sound: &LocalSound,
        out: &mut Vec<SoundRequest>,
    ) {
        out.clear();
        if self.links.is_empty() {
            return;
        }
        match &sound.request.anchor {
            Some(SoundAnchor::Entity { key, point }) => {
                let Some(entity) = crate::sound_events::entity_from_key(*key) else {
                    return;
                };
                let Some(member) = self.voice_member(registry, frame.view, entity) else {
                    return;
                };
                let point = Vec3::from_array(*point);
                self.visit_member_links(
                    registry,
                    frame,
                    entity,
                    member,
                    point,
                    false,
                    |_, mapping| {
                        out.push(ghost_request(
                            &sound.request.sound,
                            SoundAnchor::Point(mapping.point(point).to_array()),
                        ));
                    },
                );
            }
            Some(SoundAnchor::Contacts(contacts)) => {
                let Some(owner) = sound.shot_owner else {
                    return;
                };
                let Some(member) = self.voice_member(registry, frame.view, owner) else {
                    return;
                };
                // The owner is judged where membership and its ghost present it.
                let lane = clocks::member_lane(registry, owner, member.1, member.0);
                let Some(pose) = frame.stamps.presented_transform(owner, lane).or_else(|| {
                    registry
                        .interpolated_transform(owner, self.voices.alpha)
                        .ok()
                }) else {
                    return;
                };
                let owner_point = self
                    .movers
                    .member_point(registry, owner, member.0, pose)
                    .position;
                self.visit_member_links(
                    registry,
                    frame,
                    owner,
                    member,
                    owner_point,
                    true,
                    |index, mapping| {
                        let mapped: Vec<[f32; 3]> = contacts
                            .iter()
                            .map(|contact| Vec3::from_array(*contact))
                            .filter(|contact| self.regions.contains(index, *contact))
                            .map(|contact| mapping.point(contact).to_array())
                            .collect();
                        if !mapped.is_empty() {
                            out.push(ghost_request(
                                &sound.request.sound,
                                SoundAnchor::Contacts(mapped),
                            ));
                        }
                    },
                );
            }
            Some(SoundAnchor::Point(_)) | None => {}
        }
    }

    /// Visit each link through which `entity` (classified as `member`), judged
    /// at `point`, is a member whose style carries audio (and impact audio, for
    /// `impact`), with the link's index and mapping.
    #[allow(clippy::too_many_arguments)]
    fn visit_member_links(
        &self,
        registry: &EntityRegistry,
        frame: &VoiceFrame<'_>,
        entity: EntityId,
        (category, origin): (GhostCategory, CandidateOrigin<'_>),
        point: Vec3,
        impact: bool,
        mut visit: impl FnMut(usize, SeamlessMapping),
    ) {
        let lane = clocks::member_lane(registry, entity, origin, category);
        let clock = frame
            .stamps
            .state_tick(entity, lane)
            .unwrap_or_else(|| frame.clocks.lane(lane));
        let shown = category.members_bit();
        // Only a mover's id matters to `listed_by`; any pose names it.
        let pose = registry
            .get_component::<Transform>(entity)
            .copied()
            .unwrap_or_default();
        let listing = self.movers.member_point(registry, entity, category, pose);
        self.regions.visit_containing(point, |index| {
            let style = self.link_style(u16::try_from(index).unwrap_or(u16::MAX));
            let carries = if impact {
                style.carries_impact_audio()
            } else {
                style.carries_audio()
            };
            if carries
                && self.movers.listed_by(&listing, index)
                && self.links[index].history.state_at(clock).shows(shown)
            {
                visit(index, self.links[index].mapping);
            }
        });
    }

    /// Whether `entity` can be a ghost member on this machine, as which
    /// category, and where classification reads it. The viewer's own pawn and
    /// projectiles never are.
    fn voice_member<'a>(
        &self,
        registry: &EntityRegistry,
        view: CandidateView<'a>,
        entity: EntityId,
    ) -> Option<(GhostCategory, CandidateOrigin<'a>)> {
        let viewer = crate::followed_player_pawn(registry);
        if Some(entity) == viewer {
            return None;
        }
        if let (Some(viewer), Ok(projectile)) = (
            viewer,
            registry.get_component::<ProjectileComponent>(entity),
        ) && projectile.owner_pawn == viewer
        {
            return None;
        }
        let origin = match view {
            CandidateView::Authoritative => CandidateOrigin::Local,
            // Movers load locally on every role; every other client candidate
            // is a display copy known by its replicated class.
            CandidateView::Client(_)
                if registry
                    .get_component::<KinematicMoverComponent>(entity)
                    .is_ok() =>
            {
                CandidateOrigin::Local
            }
            CandidateView::Client(replication) => replication
                .remote_entities_by_class()
                .find(|(remote, _)| *remote == entity)
                .map(|(_, class)| CandidateOrigin::Replicated(class))?,
        };
        let category =
            eligibility::classify(registry, entity, origin, &self.opt_ins, &self.movers)?;
        Some((category, origin))
    }
}

fn ghost_request(sound: &str, anchor: SoundAnchor) -> SoundRequest {
    SoundRequest {
        bus: GHOST_BUS.to_string(),
        sound: sound.to_string(),
        looping: false,
        anchor: Some(anchor),
    }
}

/// This frame's admitted ghost voices, each tagged with the index of the
/// sound it echoes.
#[derive(Debug, Default)]
pub(crate) struct GhostVoicePlan {
    voices: Vec<(usize, SoundRequest)>,
}

impl GhostVoicePlan {
    /// Play `sounds` in order through `play(request, ghost)`, which reports
    /// whether the voice was admitted (`Audio::play` returned a handle);
    /// `ghost` is true for a ghost voice. Each admitted sound's ghost voices
    /// start right after it; a refused sound starts none, so a ghost voice
    /// never plays beside a voice this machine refused.
    pub(crate) fn play_beside(
        self,
        sounds: impl IntoIterator<Item = LocalSound>,
        mut play: impl FnMut(SoundRequest, bool) -> bool,
    ) {
        // Voices are planned in sound order, so one pass pairs them.
        let mut voices = self.voices.into_iter().peekable();
        for (index, sound) in sounds.into_iter().enumerate() {
            let admitted = play(sound.request, false);
            while let Some((_, voice)) = voices.next_if(|(of, _)| *of == index) {
                if admitted {
                    play(voice, true);
                }
            }
        }
    }
}

impl crate::App {
    /// Audio stage, before this frame's sounds are built: remember who fired
    /// each shot this frame, so its impacts (now or after a flight) can be
    /// attributed. On the host each shot is also recorded under its normalized
    /// id, the owner's network id, which projectile impacts and observer cues
    /// carry. On a connected client, observer fire cues carry it already.
    pub(crate) fn record_ghost_shot_owners(
        &mut self,
        weapons: &[WeaponEmission],
        ai: &[AiEmission],
    ) {
        // With no link no impact can ghost, so no owner is ever read.
        if self.ghost_links.links.is_empty() {
            return;
        }
        let allocator = match self
            .session
            .as_ref()
            .and_then(|session| session.net_endpoint.as_ref())
        {
            Some(crate::netcode::NetEndpoint::Host { allocator, .. }) => Some(allocator),
            _ => None,
        };
        let fired = weapons
            .iter()
            .filter(|emission| emission.address == "activate")
            .map(|emission| (emission.shot_id, &emission.emitter))
            .chain(
                ai.iter()
                    .map(|emission| (emission.shot_id, &emission.emitter)),
            )
            .chain(
                self.observer_weapon_cues
                    .iter()
                    .filter(|cue| cue.kind == crate::netcode::weapon_cues::WeaponCueKind::Activate)
                    .map(|cue| (Some(cue.shot_id), &cue.emitter)),
            );
        for (shot, emitter) in fired {
            let (Some(raw), Emitter::Entity { id: owner, .. }) = (shot, emitter) else {
                continue;
            };
            self.ghost_links.record_shot_owner(raw, *owner);
            if let Some(network_id) = allocator.and_then(|ids| ids.network_id_for_entity(*owner)) {
                let mut normalized = raw;
                normalized.pawn = network_id.0;
                self.ghost_links.record_shot_owner(normalized, *owner);
            }
        }
    }

    /// The ghost voices each of `sounds` admits this frame, read at `alpha`,
    /// the frame's render sub-tick. Admission happens before any of them
    /// plays; the caller plays each sound's ghosts only if it was admitted.
    pub(crate) fn plan_ghost_voices(
        &mut self,
        sounds: &[LocalSound],
        alpha: f32,
    ) -> GhostVoicePlan {
        self.ghost_links.voices.alpha = alpha;
        let mut plan = GhostVoicePlan::default();
        if self.ghost_links.links.is_empty() || !sounds.iter().any(LocalSound::may_ghost) {
            return plan;
        }
        let Some(session) = self.session.as_ref() else {
            return plan;
        };
        self.ghost_links
            .refresh_opt_ins(&session.scripting.script_ctx.data_registry.borrow());
        let (source, view, stamps) = super::frame::frame_sources(
            session,
            self.remote_player_presentation.render_server_tick,
            self.ghost_links.completed_ticks(),
            alpha,
        );
        let frame = VoiceFrame {
            clocks: PresentationClocks::compute(source, alpha),
            view,
            stamps,
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        let mut voices = Vec::new();
        for (index, sound) in sounds.iter().enumerate() {
            self.ghost_links
                .ghost_voices(&registry, &frame, sound, &mut voices);
            plan.voices
                .extend(voices.drain(..).map(|voice| (index, voice)));
        }
        plan
    }

    /// [`Self::plan_ghost_voices`] for one sound played outside the audio
    /// stage (a `playSound` reaction drained at frame end), at the alpha that
    /// stage read.
    pub(crate) fn plan_ghost_voices_for(&mut self, sound: &LocalSound) -> GhostVoicePlan {
        let alpha = self.ghost_links.voices.alpha;
        self.plan_ghost_voices(std::slice::from_ref(sound), alpha)
    }
}

#[cfg(test)]
#[path = "voices_tests.rs"]
mod tests;
