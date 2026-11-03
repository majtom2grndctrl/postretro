// Teammate revive markers: which assistable teammates carry a world-anchored
// marker, which one a hold of `assist` would revive (`inRange`), and where each
// marker floats. Every machine derives them from state it already holds.
// See: context/lib/ui.md §1 (presentation layer) · context/lib/scripting.md §12 (Assist)

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, PresentationFact, Transform};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::data_descriptors::ASSISTABLE_TEAMMATES_IN_RANGE_FACT;
use postretro_scripting_core::group_resolution::extend_with_player_pawns;

use super::glue::{gather_candidates, local_helper};
use crate::impact_policy::AssistableTeammatesOverlayConfig;
use crate::netcode::{self, ClientReplication};
use crate::presentation_pool::{OverlayKind, PresentationPool};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::assist::{AssistCandidate, AssistHelper, AssistState, PawnLife, assist_candidate};

const KIND: OverlayKind = OverlayKind::AssistableTeammates;

/// Per-frame buffers, reused so the candidate lists and pair table keep their
/// allocations across frames. Each marked pawn still clones its template handle.
#[derive(Debug, Default)]
pub(crate) struct MarkerScratch {
    pawns: Vec<(EntityId, netcode::RemoteAssistFacts)>,
    table: Vec<netcode::AssistPairEntry>,
    group: Vec<EntityId>,
    candidates: Vec<AssistCandidate>,
    marked: Vec<EntityId>,
}

/// Mark up to `max_visible` assistable candidates and drop every other
/// marker. The candidate the shared target choice picks for the local helper
/// always holds a slot; the rest fill in candidate order. `inRange` is true
/// only on that chosen candidate, so the prompt names the one teammate a hold
/// would revive even when the cap would otherwise cut it.
fn select(
    pool: &mut PresentationPool,
    config: &AssistableTeammatesOverlayConfig,
    helper: Option<&AssistHelper>,
    candidates: &[AssistCandidate],
    marked: &mut Vec<EntityId>,
) {
    let local = helper.map(|helper| helper.id);
    let chosen = helper
        .and_then(|helper| assist_candidate(helper, candidates))
        .map(|candidate| candidate.id);
    marked.clear();
    if config.max_visible > 0 {
        marked.extend(chosen);
    }
    for candidate in candidates {
        if marked.len() >= config.max_visible {
            break;
        }
        if candidate.assistable && Some(candidate.id) != local && Some(candidate.id) != chosen {
            marked.push(candidate.id);
        }
    }
    pool.retain_overlays(KIND, |entity| marked.contains(&entity));
    for &entity in marked.iter() {
        if pool.upsert_authoritative_overlay(
            KIND,
            entity,
            config.template.clone(),
            config.max_visible,
        ) {
            pool.stamp_overlay_fact(
                KIND,
                entity,
                ASSISTABLE_TEAMMATES_IN_RANGE_FACT,
                PresentationFact::Bool(chosen == Some(entity)),
            );
        }
    }
}

/// One marker's anchor: a posed socket when the template names one the model
/// has, else the pawn's presented position; either way lifted by `offsetY`.
/// The hitbox top is never used: a downed pawn's capsule sits lower than the
/// standing hitbox it keeps.
fn marker_anchor(
    registry: &EntityRegistry,
    hit_zones: &HitZoneStore,
    config: &AssistableTeammatesOverlayConfig,
    entity: EntityId,
    anim_time: f64,
    position: impl FnOnce() -> Option<Vec3>,
) -> Option<Vec3> {
    let base = config
        .world_anchor
        .socket
        .as_deref()
        .and_then(|socket| hit_zones.posed_socket_world(registry, entity, socket, anim_time))
        .or_else(position)?;
    let anchor = base + Vec3::Y * config.world_anchor.offset_y;
    anchor.is_finite().then_some(anchor)
}

/// Re-anchor every live marker. A marker with no resolvable anchor is hidden
/// until one resolves.
fn anchor_markers(
    pool: &mut PresentationPool,
    registry: &EntityRegistry,
    hit_zones: &HitZoneStore,
    config: &AssistableTeammatesOverlayConfig,
    anim_time: f64,
    marked: &mut Vec<EntityId>,
    position: impl Fn(EntityId) -> Option<Vec3>,
) {
    marked.clear();
    marked.extend(pool.tracked_overlay_ids_iter(KIND));
    for &entity in marked.iter() {
        let anchor = marker_anchor(registry, hit_zones, config, entity, anim_time, || {
            position(entity)
        });
        pool.update_overlay_anchor(KIND, entity, anchor, false);
    }
}

/// The listen host's and single player's selection, over the `players()`
/// group in its order: each pawn's `assistable` bool and the pair values the
/// last assist pass evaluated for the local pawn. Reads registry transforms,
/// so it must run while they hold the authoritative pose the assist pass read.
/// With no marker overlay declared, drops only markers.
pub(crate) fn select_host_markers(
    registry: &EntityRegistry,
    assist: &AssistState,
    pool: &mut PresentationPool,
    config: Option<&AssistableTeammatesOverlayConfig>,
    scratch: &mut MarkerScratch,
) {
    let Some(config) = config else {
        pool.clear_overlays(KIND);
        return;
    };
    let helper = local_helper(registry, registry.local_player_movement_pawn());
    let local = helper.as_ref().map(|helper| helper.id);
    scratch.group.clear();
    extend_with_player_pawns(registry, None, &mut scratch.group);
    scratch.candidates.clear();
    scratch
        .candidates
        .extend(scratch.group.iter().filter_map(|&id| {
            if Some(id) == local {
                return None;
            }
            let movement = registry.get_component::<PlayerMovementComponent>(id).ok()?;
            let position = registry.get_component::<Transform>(id).ok()?.position;
            Some(AssistCandidate {
                id,
                position,
                life: PawnLife {
                    life_state: movement.life_state,
                    entry: Some(movement.life_state_entry),
                },
                assistable: movement.assistable,
                pair: local.and_then(|helper| assist.pair_values(helper, id)),
            })
        }));
    select(
        pool,
        config,
        helper.as_ref(),
        &scratch.candidates,
        &mut scratch.marked,
    );
}

/// The listen host's and single player's anchors, from the registry pose:
/// run after connected-client pawns take their presented (delayed) pose, so a
/// marker rides the pawn the host draws.
pub(crate) fn anchor_host_markers(
    registry: &EntityRegistry,
    pool: &mut PresentationPool,
    hit_zones: &HitZoneStore,
    config: Option<&AssistableTeammatesOverlayConfig>,
    anim_time: f64,
    scratch: &mut MarkerScratch,
) {
    let Some(config) = config else {
        return;
    };
    anchor_markers(
        pool,
        registry,
        hit_zones,
        config,
        anim_time,
        &mut scratch.marked,
        |entity| {
            registry
                .get_component::<Transform>(entity)
                .ok()
                .map(|transform| transform.position)
        },
    );
}

/// A connected client's markers: retained remote assist facts, interpolated
/// remote positions at `render_server_tick`, and the host's pair table for
/// the local pawn, the same candidates the client assist step reads. With no
/// marker overlay declared, drops only markers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_client_markers(
    registry: &EntityRegistry,
    replication: &ClientReplication,
    render_server_tick: Option<f64>,
    pool: &mut PresentationPool,
    hit_zones: &HitZoneStore,
    config: Option<&AssistableTeammatesOverlayConfig>,
    anim_time: f64,
    scratch: &mut MarkerScratch,
) {
    let Some(config) = config else {
        pool.clear_overlays(KIND);
        return;
    };
    // The marked pawn only: before the local record arms it there is no
    // helper, so every marker is plain.
    let helper = local_helper(registry, registry.local_player_pawn());
    gather_candidates(
        replication,
        render_server_tick,
        helper.as_ref().map(|helper| helper.id),
        &mut scratch.pawns,
        &mut scratch.table,
        &mut scratch.candidates,
    );
    select(
        pool,
        config,
        helper.as_ref(),
        &scratch.candidates,
        &mut scratch.marked,
    );
    anchor_markers(
        pool,
        registry,
        hit_zones,
        config,
        anim_time,
        &mut scratch.marked,
        |entity| replication.remote_assist_position(entity, render_server_tick),
    );
}

impl crate::App {
    /// Host and single-player marker selection, after the frame's overlay
    /// updates and before connected-client pawns take their presented pose.
    pub(crate) fn select_host_revive_markers(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if matches!(
            session.net_endpoint,
            Some(netcode::NetEndpoint::Client { .. })
        ) {
            return;
        }
        let registry = session.scripting.script_ctx.registry.borrow();
        select_host_markers(
            &registry,
            &session.assist_state,
            &mut session.presentation_pool,
            session
                .scripting
                .impact_policy_runtime
                .assistable_teammates_overlay_config(),
            &mut self.client_assist.markers,
        );
    }

    /// Host and single-player marker anchors, after connected-client pawns
    /// take their presented pose and before the render collectors read it.
    pub(crate) fn anchor_host_revive_markers(&mut self, anim_time: f64) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if matches!(
            session.net_endpoint,
            Some(netcode::NetEndpoint::Client { .. })
        ) {
            return;
        }
        let registry = session.scripting.script_ctx.registry.borrow();
        anchor_host_markers(
            &registry,
            &mut session.presentation_pool,
            &session.hit_zone_store,
            session
                .scripting
                .impact_policy_runtime
                .assistable_teammates_overlay_config(),
            anim_time,
            &mut self.client_assist.markers,
        );
    }

    /// A connected client's markers, after remote interpolation wrote this
    /// frame's presented poses.
    pub(crate) fn update_client_revive_markers(&mut self, anim_time: f64) {
        let render_server_tick = self.remote_player_presentation.render_server_tick;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(netcode::NetEndpoint::Client { replication, .. }) = session.net_endpoint.as_ref()
        else {
            return;
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        update_client_markers(
            &registry,
            replication,
            render_server_tick,
            &mut session.presentation_pool,
            &session.hit_zone_store,
            session
                .scripting
                .impact_policy_runtime
                .assistable_teammates_overlay_config(),
            anim_time,
            &mut self.client_assist.markers,
        );
    }
}

#[cfg(test)]
#[path = "markers_tests.rs"]
mod tests;
