// Ghost-link runtime: installed links, their state history and per-frame membership.
// See: context/lib/entity_model.md §7d

mod bus;
#[cfg(feature = "capture")]
mod capture;
mod clocks;
mod commands;
mod eligibility;
mod frame;
mod history;
mod install;
mod look;
mod membership;
mod movers;
mod opt_ins;
#[cfg(test)]
mod opt_ins_test_fixtures;
mod replication;
mod voices;

use std::time::Instant;

use glam::Vec3;
use postretro_collision::TeleporterRegions;
use postretro_entities::{ComponentKind, ComponentValue, DataRegistry, EntityId, EntityRegistry};
use postretro_foundation::SeamlessMapping;
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_limiter::DEFAULT_FRAME_SECONDS;

use crate::scripting_systems::billboard_ghost_render::ghost_presented_transform;

pub(crate) use bus::GhostBus;
use clocks::{PoseStamps, PresentationClocks};
use eligibility::CandidateView;
pub(crate) use eligibility::GhostCategory;
use history::{LinkHistory, LinkState};
pub(crate) use install::spawn_loaded_ghost_links;
pub(crate) use look::GhostLook;
pub(crate) use membership::GhostMember;
use membership::{GhostKey, MembershipTable, ramp_step};
use movers::GhostMovers;
use opt_ins::GhostOptIns;
pub(crate) use opt_ins::warn_if_links_without_opt_ins;
pub(crate) use voices::LocalSound;

/// One installed link: its mapping, the entity holding its live state, and that
/// state's history over host time.
#[derive(Debug)]
struct InstalledLink {
    mapping: SeamlessMapping,
    entity: Option<EntityId>,
    history: LinkHistory,
}

/// A level's ghost links, built at install and dropped at unload. Answers, once
/// per frame, which ghosts exist, through which link, at what mapped pose and
/// at what presented membership ([`Self::members`]).
#[derive(Debug, Default)]
pub(crate) struct GhostLinks {
    links: Vec<InstalledLink>,
    /// Link volumes in map order; the point query visits each containing link once.
    regions: TeleporterRegions,
    membership: MembershipTable,
    /// This peer's descriptor opt-ins, following the committed descriptor
    /// snapshot ([`Self::refresh_opt_ins`]).
    opt_ins: GhostOptIns,
    /// Each link's movers, by its `ghost_tags`, resolved at install.
    movers: GhostMovers,
    /// Recent shot owners and the alpha ghost-voice admission reads at.
    voices: voices::GhostVoices,
    /// Single player's count of completed fixed ticks: the host-tick stand-in
    /// that stamps history and clocks when no endpoint exists.
    completed_ticks: u32,
    /// The last stamp recorded; a lower one means the stamping clock restarted.
    last_stamp: Option<u32>,
    /// When the previous membership update presented, for the ramp's frame time.
    last_update: Option<Instant>,
    /// The presented-frame seconds the latest update ramped by: the per-ghost
    /// flash clamp limits against the same frame time.
    last_elapsed: f32,
    /// Each link's resolved style and the script time looks present at.
    looks: look::LinkLooks,
    /// A connected client's received look filters, host and rebased.
    received: replication::ReceivedLooks,
}

/// One frame's inputs to [`GhostLinks::update`].
pub(crate) struct GhostFrame<'a> {
    pub clocks: PresentationClocks,
    /// The render sub-tick alpha the registry's presented transforms blend by.
    pub alpha: f32,
    /// Presented-frame seconds since the previous update.
    pub elapsed_seconds: f32,
    pub view: CandidateView<'a>,
    /// Per-member pose stamps for delay-buffered members.
    pub stamps: PoseStamps<'a>,
}

impl GhostLinks {
    /// Build from the level's id-56 records and the link entities install spawned.
    /// Each link's local spawn state is its installed component's.
    pub(crate) fn install(world: &LevelWorld, registry: &EntityRegistry) -> Self {
        let records = &world.ghost_links.links;
        let mut entities = vec![None; records.len()];
        for (id, value) in registry.iter_with_kind(ComponentKind::GhostLink) {
            if let ComponentValue::GhostLink(link) = value
                && let Some(slot) = entities.get_mut(usize::from(link.index))
            {
                *slot = Some((id, LinkState::from_component(link)));
            }
        }
        let links: Vec<InstalledLink> = records
            .iter()
            .zip(entities)
            .map(|(record, bound)| InstalledLink {
                mapping: SeamlessMapping::new(
                    Vec3::from(record.landmark_origin),
                    record.landmark_yaw,
                    Vec3::from(record.target_origin),
                    record.target_yaw,
                ),
                entity: bound.map(|(id, _)| id),
                history: LinkHistory::new(bound.map_or(
                    LinkState::new(record.enabled_on_spawn, record.members),
                    |(_, state)| state,
                )),
            })
            .collect();
        let regions = TeleporterRegions::build(
            records
                .iter()
                .map(|record| {
                    record
                        .brushes
                        .iter()
                        .map(postretro_physics::environment_volumes::convex_leaf)
                        .collect()
                })
                .collect(),
        );
        if !links.is_empty() {
            log::info!("[Ghost] installed {} links", links.len());
        }
        Self {
            received: replication::ReceivedLooks::new(links.len()),
            links,
            regions,
            movers: GhostMovers::resolve(world),
            ..Self::default()
        }
    }

    /// The landmark → target mapping of link `index`.
    #[cfg(test)]
    pub(crate) fn mapping(&self, index: u16) -> Option<SeamlessMapping> {
        self.links.get(usize::from(index)).map(|link| link.mapping)
    }

    /// Link `index`'s state history, for tests that drive it directly.
    #[cfg(test)]
    pub(crate) fn history_mut(&mut self, index: u16) -> Option<&mut LinkHistory> {
        self.links
            .get_mut(usize::from(index))
            .map(|link| &mut link.history)
    }

    /// Single player's completed fixed ticks, for [`clocks::ClockSource::SinglePlayer`].
    pub(crate) fn completed_ticks(&self) -> u32 {
        self.completed_ticks
    }

    /// Record every link's end-of-tick component state after one completed fixed
    /// tick, on the host and in single player. `host_tick` is the endpoint's
    /// post-increment tick; without one the single-player counter advances and
    /// stamps instead. Only a changed state is kept.
    pub(crate) fn record_after_fixed_tick(
        &mut self,
        registry: &EntityRegistry,
        host_tick: Option<u32>,
    ) {
        let stamp = host_tick.unwrap_or_else(|| {
            self.completed_ticks = self.completed_ticks.wrapping_add(1);
            self.completed_ticks
        });
        let restarted = self.last_stamp.is_some_and(|last| stamp < last);
        self.last_stamp = Some(stamp);
        for link in &mut self.links {
            let Some(component) = link.entity.and_then(|id| {
                registry
                    .get_component::<postretro_entities::GhostLinkComponent>(id)
                    .ok()
            }) else {
                continue;
            };
            // Armed state, categories and the host's script colour filter, as
            // this tick left them.
            let state = LinkState::from_component(component);
            if restarted {
                link.history.rebase(state);
            } else {
                link.history.push(stamp, state);
            }
        }
    }

    /// Follow the committed descriptor snapshot before [`Self::update`], so a
    /// committed reload that flips an opt-in re-resolves membership on the next
    /// frame, through the membership ramp.
    pub(crate) fn refresh_opt_ins(&mut self, data: &DataRegistry) {
        self.opt_ins.refresh(data);
    }

    /// Presented-frame seconds since the previous update, measured from the
    /// frame's `now` the way the photosensitivity limiter measures its frames.
    pub(crate) fn frame_elapsed(&mut self, now: Instant) -> f32 {
        let elapsed = self.last_update.map_or(DEFAULT_FRAME_SECONDS, |previous| {
            now.saturating_duration_since(previous).as_secs_f32()
        });
        self.last_update = Some(now);
        self.last_elapsed = elapsed;
        elapsed
    }

    /// Evaluate membership from presented state, once per frame after every
    /// presented pose is written and before render collection. Bounded: one
    /// point query per candidate, one pass over live ghosts, no allocation once
    /// warm.
    pub(crate) fn update(&mut self, registry: &EntityRegistry, frame: GhostFrame<'_>) {
        let Self {
            links,
            regions,
            membership,
            opt_ins,
            movers,
            looks,
            ..
        } = self;
        let clocks = frame.clocks;
        // The viewer is never a candidate (`visit_candidates`); a ghost it
        // already has, from before the camera followed it, drops here.
        membership.begin_frame(crate::followed_player_pawn(registry));
        if !links.is_empty() {
            let oldest = clocks.oldest_readable();
            for link in links.iter_mut() {
                link.history.prune_before(oldest);
            }
            eligibility::visit_candidates(
                registry,
                frame.view,
                opt_ins,
                movers,
                |entity, category, origin| {
                    let Some(presented) = ghost_presented_transform(registry, entity, frame.alpha)
                    else {
                        return;
                    };
                    let lane = clocks::member_lane(registry, entity, origin, category);
                    let lane_clock = clocks.lane(lane);
                    let clock = frame.stamps.state_tick(entity, lane).unwrap_or(lane_clock);
                    let lag = clocks.lag(clock, lane_clock);
                    let shown = category.members_bit();
                    let point = movers.member_point(registry, entity, category, presented);
                    regions.visit_containing(point.position, |index| {
                        let history = &links[index].history;
                        if movers.listed_by(&point, index) && history.state_at(clock).shows(shown) {
                            let link = u16::try_from(index).expect("ghost link index fits u16");
                            membership.mark_target(
                                GhostKey { link, entity },
                                category,
                                presented,
                                lag,
                                history.spawn().shows(shown),
                            );
                        }
                    });
                },
            );
        }
        // Each ghost's look, read where its member presents: the link state in
        // force at its read clock, at its look time. O(ghosts), no allocation.
        let now = clocks.script_now();
        membership.finish_frame(ramp_step(frame.elapsed_seconds), |index, lag| {
            let link = links.get(usize::from(index))?;
            let state = link.history.state_at(now - lag.state_ticks);
            Some((
                link.mapping,
                looks.resolve(index, &state.filter, lag.look_seconds()),
            ))
        });
    }

    /// This frame's ghosts, ordered by link then entity.
    pub(crate) fn members(&self) -> &[GhostMember] {
        self.membership.members()
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
