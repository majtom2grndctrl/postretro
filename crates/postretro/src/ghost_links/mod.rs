// Ghost-link runtime: installed links, their state history and per-frame membership.
// See: context/lib/entity_model.md §7d

mod clocks;
mod eligibility;
mod frame;
mod history;
mod install;
mod membership;

use std::time::Instant;

use glam::Vec3;
use postretro_collision::TeleporterRegions;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::SeamlessMapping;
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_limiter::DEFAULT_FRAME_SECONDS;

use clocks::PresentationClocks;
use eligibility::CandidateView;
use history::{LinkHistory, LinkState};
pub(crate) use install::spawn_loaded_ghost_links;
pub(crate) use membership::GhostMember;
use membership::{GhostKey, MembershipTable, ramp_step};

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
    /// Single player's count of completed fixed ticks: the host-tick stand-in
    /// that stamps history and clocks when no endpoint exists.
    completed_ticks: u32,
    /// The last stamp recorded; a lower one means the stamping clock restarted.
    last_stamp: Option<u32>,
    /// When the previous membership update presented, for the ramp's frame time.
    last_update: Option<Instant>,
}

/// One frame's inputs to [`GhostLinks::update`].
pub(crate) struct GhostFrame<'a> {
    pub clocks: PresentationClocks,
    /// The render sub-tick alpha the registry's presented transforms blend by.
    pub alpha: f32,
    /// Presented-frame seconds since the previous update.
    pub elapsed_seconds: f32,
    pub view: CandidateView<'a>,
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
                    LinkState {
                        armed: record.enabled_on_spawn,
                        members: record.members,
                    },
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
            links,
            regions,
            ..Self::default()
        }
    }

    /// The landmark → target mapping of link `index`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mapping(&self, index: u16) -> Option<SeamlessMapping> {
        self.links.get(usize::from(index)).map(|link| link.mapping)
    }

    /// Link `index`'s state history. Replication pushes received states here and
    /// resets it to spawn state on demotion.
    #[cfg_attr(not(test), allow(dead_code))]
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
            let Some(state) = link.entity.and_then(|id| {
                registry
                    .get_component::<postretro_entities::GhostLinkComponent>(id)
                    .ok()
                    .map(LinkState::from_component)
            }) else {
                continue;
            };
            if restarted {
                link.history.rebase(state);
            } else {
                link.history.push(stamp, state);
            }
        }
    }

    /// Presented-frame seconds since the previous update, measured from the
    /// frame's `now` the way the photosensitivity limiter measures its frames.
    pub(crate) fn frame_elapsed(&mut self, now: Instant) -> f32 {
        let elapsed = self.last_update.map_or(DEFAULT_FRAME_SECONDS, |previous| {
            now.saturating_duration_since(previous).as_secs_f32()
        });
        self.last_update = Some(now);
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
            ..
        } = self;
        membership.begin_frame();
        if !links.is_empty() {
            let clocks = frame.clocks;
            let oldest = clocks.oldest();
            for link in links.iter_mut() {
                link.history.prune_before(oldest);
            }
            eligibility::visit_candidates(registry, frame.view, |entity, category, origin| {
                let Ok(presented) = registry.interpolated_transform(entity, frame.alpha) else {
                    return;
                };
                let clock = clocks.lane(clocks::member_lane(registry, entity, origin, category));
                let shown = category.members_bit();
                // A pawn's presented body point; projectile origins and mover
                // bounds centers join with their categories.
                regions.visit_containing(presented.position, |index| {
                    if links[index].history.state_at(clock).shows(shown) {
                        let link = u16::try_from(index).expect("ghost link index fits u16");
                        membership.mark_target(GhostKey { link, entity }, category, presented);
                    }
                });
            });
        }
        membership.finish_frame(ramp_step(frame.elapsed_seconds), |link| {
            links.get(usize::from(link)).map(|link| link.mapping)
        });
    }

    /// This frame's ghosts, ordered by link then entity.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn members(&self) -> &[GhostMember] {
        self.membership.members()
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
