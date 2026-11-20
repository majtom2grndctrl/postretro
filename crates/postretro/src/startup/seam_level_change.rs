//! Level seam level changes: the frame-end pick of an activated seam, and the
//! arrival harvest at the top of unload.
//! See: context/lib/entity_model.md §7e · context/lib/boot_sequence.md §4

use glam::Vec3;

use crate::App;
use crate::seam_crossing::harvest_crossing;
use crate::startup::{LevelRequest, LevelSource};

impl App {
    /// A script level verb (`loadLevel`, `restartLevel`, `returnToFrontend`)
    /// ran this frame: it beats every seam activated this frame.
    pub(crate) fn note_script_level_request(&mut self) {
        self.script_level_request_this_frame = true;
    }

    /// Frame end, after the frame's last system-command drain: pick one seam
    /// request from this frame's activations. A script level request this
    /// frame drops them all. Otherwise the first activated seam in map order
    /// whose target the catalog knows requests a classic level change carrying
    /// its name; an unknown target warns once and requests nothing. The pick
    /// rides activation order nowhere: the trigger stage walks seams by index,
    /// and a later seam's in-tick request would otherwise replace an earlier
    /// one's.
    pub(crate) fn pick_seam_level_request(&mut self) {
        let script_request = std::mem::take(&mut self.script_level_request_this_frame);
        let mut activated = self.seam_stage.take_activations();
        if activated.is_empty() || script_request || self.seam_stage.is_inert() {
            return;
        }
        if self.session_role() == crate::session::role_state::SessionRole::Client {
            return;
        }
        activated.sort_unstable();
        for index in activated {
            let Some(record) = self.seam_stage.record(index) else {
                continue;
            };
            let (seam, target) = (record.name.clone(), record.target.clone());
            let known = self.session.as_ref().is_some_and(|session| {
                session
                    .scripting
                    .script_ctx
                    .data_registry
                    .borrow()
                    .maps
                    .iter()
                    .any(|entry| entry.id == target)
            });
            if !known {
                if self.seam_stage.warn_unknown_target_once(index) {
                    log::warn!(
                        "[Loader] seam `{seam}` targets unknown map id `{target}`; no level change"
                    );
                }
                continue;
            }
            // Unlike `loadLevel`, a seam clears no pushed tree.
            self.enqueue_level_request(LevelRequest::Seam {
                catalog_id: target,
                seam,
            });
            return;
        }
    }

    /// The top of unload, before its first teardown step: drop any arrival a
    /// previous crossing left unconsumed, then, for a seam load leaving a
    /// played level, read every seat inside the seam into its arrival. The
    /// command queues, the registry and the camera are all intact here.
    pub(super) fn harvest_seam_crossing(&mut self, seam: Option<&str>, harvest: bool) {
        self.seam_crossing = None;
        let (Some(seam), true) = (seam, harvest) else {
            return;
        };
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        let seats = session
            .seat_table
            .as_ref()
            .map(crate::netcode::SeatTable::seats)
            .unwrap_or_default();
        let queues = match session.net_endpoint.as_ref() {
            Some(crate::netcode::NetEndpoint::Host { command_queues, .. }) => Some(command_queues),
            _ => None,
        };
        let seat_table = session.seat_table.as_ref();
        let crossing = harvest_crossing(
            seam,
            &self.seam_stage,
            &registry,
            &seats,
            (self.camera.yaw, self.camera.pitch),
            |seat, accumulated_yaw| {
                let client = seat_table?.client_for_seat(seat)?;
                queues?.resolved_facing(client, accumulated_yaw)
            },
        );
        log::info!(
            "[Loader] seam `{seam}` crossing: {} seat(s) arrive landmark-relative",
            crossing.arrivals.len()
        );
        self.seam_crossing = Some(crossing);
    }

    /// A seam load that ends without committing, suspend, and a superseding
    /// load each clear every arrival.
    pub(crate) fn clear_seam_crossing(&mut self) {
        self.seam_crossing = None;
    }

    /// The crossing the next install's first spawns take, if any.
    #[cfg(test)]
    pub(crate) fn seam_crossing(&self) -> Option<&crate::seam_crossing::SeamCrossing> {
        self.seam_crossing.as_ref()
    }
}

impl App {
    /// After the host's pawn spawns and before any hint or camera reads its
    /// pose: install the seam stage, move seat 0's pawn to its arrival (a
    /// crossing's, or Continue's saved pose at rest), latch every pawn the
    /// install spawned, and hand remote seats' arrivals to the resolver.
    /// Returns the camera's first spawn, the arrival's when seat 0 arrived.
    pub(super) fn install_level_seams(
        &mut self,
        first_spawn: Option<(Vec3, Vec3)>,
    ) -> Option<(Vec3, Vec3)> {
        let crossing = self.seam_crossing.take();
        let backdrop = self.backdrop_installed;
        let client = self.is_connected_client();
        let catalog_id = match &self.active_level_source {
            Some(LevelSource::Catalog(id)) => Some(id.clone()),
            _ => None,
        };
        let (Some(world), Some(session)) = (self.level.as_ref(), self.session.as_mut()) else {
            return first_spawn;
        };
        let continue_pose = session.saves.take_continue_seam_pose(catalog_id.as_deref());
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        self.seam_stage = crate::runtime_seams::install_seam_stage(&registry, world, backdrop);
        self.spawn_poses.set_seam_offers(Default::default());
        if client || backdrop {
            return first_spawn;
        }
        let mut first_spawn = first_spawn;
        let combined = crate::collision::moving::CombinedCollisionWorld::new(
            &self.collision_world,
            &self.kinematic_mover_colliders,
            &self.kinematic_mover_tick_states,
        );
        let local = registry.local_player_pawn();
        let local_seat = local
            .and_then(|pawn| registry.seat_for_pawn(pawn))
            .unwrap_or(postretro_foundation::Seat(0));
        let mut crossing = crossing;
        // A crossing's arrival, else Continue's saved pose: seat 0 takes at
        // most one, and its first spawn consumes it either way.
        let arrival = crossing
            .as_mut()
            .and_then(|crossing| {
                let motion = crossing.arrivals.remove(&local_seat)?;
                Some((crossing.seam.clone(), motion))
            })
            .or_else(|| {
                continue_pose.map(|saved| {
                    (
                        saved.seam,
                        postretro_physics::movement::seam_arrival::LandmarkRelative::at_rest(
                            Vec3::from(saved.pose.offset),
                            saved.pose.view_yaw,
                            saved.pose.view_pitch,
                        ),
                    )
                })
            });
        if let (Some(pawn), Some((seam, motion))) = (local, arrival) {
            match crate::seam_crossing::arrive_pawn(
                &mut registry,
                &combined,
                &self.seam_stage,
                &seam,
                pawn,
                &motion,
            ) {
                Ok((index, pose)) => {
                    first_spawn = Some((pose.body, Vec3::new(pose.view_pitch, pose.view_yaw, 0.0)));
                    self.seam_stage.queue_arrival(index, pawn);
                }
                Err(fallback) => crate::seam_crossing::warn_fallback(&seam, fallback, "the host"),
            }
        }
        // Every pawn this install spawned latches against the seams it
        // overlaps, before any trigger tick sees it.
        let pawns: Vec<_> = registry
            .query_by_component_and_tag(postretro_entities::ComponentKind::PlayerMovement, None)
            .map(|(pawn, _)| pawn)
            .collect();
        for pawn in pawns {
            self.seam_stage.latch_spawned_in(&registry, pawn);
        }
        drop(registry);
        // Every other seat's arrival waits for its client's participation
        // entry through the host's spawn-pose resolver.
        let mut offers = std::collections::BTreeMap::new();
        if let Some(crossing) = crossing {
            for (seat, motion) in crossing.arrivals {
                match crate::seam_crossing::SeamSpawnArrival::resolve(
                    &self.seam_stage,
                    &crossing.seam,
                    motion,
                ) {
                    Some(arrival) => {
                        offers.insert(seat, arrival);
                    }
                    None => crate::seam_crossing::warn_fallback(
                        &crossing.seam,
                        crate::seam_crossing::ArrivalFallback::MissingSeam,
                        &format!("seat {}", seat.0),
                    ),
                }
            }
        }
        self.spawn_poses.set_seam_offers(offers);
        first_spawn
    }
}
