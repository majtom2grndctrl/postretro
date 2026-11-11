//! Host spawn-pose resolver: each remote seat's spawn pose, the placement
//! reservation resolving makes, the hint naming it, and the pose promotion
//! spawns at.
//! See: context/lib/networking.md §Slot lifecycle

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use glam::Vec3;
use postretro_entities::{EntityRegistry, EntityTypeDescriptor};
use postretro_foundation::{NavAgentParams, Seat};
use postretro_net::wire::{ServerControlMessage, SpawnPoseHint};
use postretro_sim::placement_occupancy::PlacementSite;

use crate::netcode::SeatTable;
use crate::scripting::builtins::data_archetype::{find_descriptor, spawn_origin_shift};
use crate::scripting::builtins::player_spawn::{
    joinable_sites, spawn_descriptor, standing_position,
};
use crate::scripting::map_entity::MapEntity;
use postretro_net::transport::NetServer;

/// Eye position plus yaw and pitch, radians in engine convention (YXZ: the
/// placement's `angles.y` and `angles.x`), as host install seeds its own camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpawnPose {
    pub(crate) eye: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

/// A pose no placement backs, with the descriptor class its pawn spawns as.
/// Its pawn spawns with an empty key bag.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UnbackedSpawn {
    pub(crate) pose: SpawnPose,
    pub(crate) entity_class: String,
    /// Consumed by the seat's spawn: the seat then resolves afresh when it
    /// next leaves participation, to a player start unless the source offers
    /// again.
    pub(crate) single_use: bool,
}

/// Where a seat spawns, as resolved and recorded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedSpawn {
    /// A `player_spawn` the seat's placement assignment reserves.
    Placement { map_index: usize, pose: SpawnPose },
    /// A pose no placement backs; the seat holds no placement.
    Unbacked(UnbackedSpawn),
}

/// Poses no placement backs. A source answers per seat before any player start
/// is considered; a seat it declines resolves to a player start.
pub(crate) trait UnbackedSpawnSource {
    /// The pose `seat` spawns at instead of a player start, or `None`.
    fn pose_for(&self, seat: Seat) -> Option<UnbackedSpawn>;
    /// `seat`'s pawn spawned at the single-use pose this source offered it.
    fn consume(&mut self, seat: Seat);
}

/// The installed level's player starts as the resolver reads them: the spawn
/// placements, the descriptors their classes resolve against, and the
/// caller-resolved sites a joiner may be assigned.
pub(crate) struct SpawnPlacements<'a> {
    spawn_points: &'a [MapEntity],
    descriptors: &'a [EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
    sites: Vec<PlacementSite>,
}

impl<'a> SpawnPlacements<'a> {
    /// The installed level's starts, or `None` for a level with no
    /// `player_spawn`. Its joiners spawn by the slot's default accept, which
    /// reads no resolved pose, so no seat resolves there and no hint is sent:
    /// every site that names a level or promotes a seat gates on this.
    pub(crate) fn of_level(
        spawn_points: &'a [MapEntity],
        descriptors: &'a [EntityTypeDescriptor],
        agent_params: Option<NavAgentParams>,
    ) -> Option<Self> {
        (!spawn_points.is_empty()).then(|| Self::joinable(spawn_points, descriptors, agent_params))
    }

    /// Every spawn whose class resolves, in map order.
    pub(crate) fn joinable(
        spawn_points: &'a [MapEntity],
        descriptors: &'a [EntityTypeDescriptor],
        agent_params: Option<NavAgentParams>,
    ) -> Self {
        Self {
            spawn_points,
            descriptors,
            agent_params,
            sites: joinable_sites(spawn_points, descriptors),
        }
    }

    /// The pose a pawn spawned at `map_index` presents: its standing position
    /// plus its class's eye height, facing the placement's yaw and pitch.
    fn pose_at(&self, map_index: usize) -> Option<SpawnPose> {
        let spawn = self.spawn_points.get(map_index)?;
        let eye_height = spawn_descriptor(spawn, self.descriptors)?
            .movement
            .as_ref()
            .map_or(0.0, |movement| movement.capsule.eye_height);
        Some(SpawnPose {
            eye: standing_position(spawn, self.descriptors, self.agent_params)
                + Vec3::Y * eye_height,
            yaw: spawn.angles.y,
            pitch: spawn.angles.x,
        })
    }
}

impl ResolvedSpawn {
    pub(crate) fn pose(&self) -> SpawnPose {
        match self {
            Self::Placement { pose, .. } => *pose,
            Self::Unbacked(unbacked) => unbacked.pose,
        }
    }

    /// The hint naming this pose on the install `level_identity` and
    /// `level_life` describe: the pair `Revealed` declares.
    pub(crate) fn hint(&self, level_identity: String, level_life: Option<u64>) -> SpawnPoseHint {
        let pose = self.pose();
        SpawnPoseHint {
            level_identity,
            level_life,
            eye: pose.eye.to_array(),
            yaw: pose.yaw,
            pitch: pose.pitch,
        }
    }

    /// The placement a net-slot spawn materializes this pawn from. A
    /// placement-backed spawn borrows its `player_spawn`. An unbacked one is a
    /// synthetic placement whose standing position puts the eye at the pose
    /// and whose angles carry its yaw and pitch; `None` when its class is not
    /// registered.
    pub(crate) fn spawn_placement<'a>(
        &self,
        spawn_points: &'a [MapEntity],
        descriptors: &[EntityTypeDescriptor],
        agent_params: Option<NavAgentParams>,
    ) -> Option<Cow<'a, MapEntity>> {
        match self {
            Self::Placement { map_index, .. } => spawn_points.get(*map_index).map(Cow::Borrowed),
            Self::Unbacked(unbacked) => {
                let descriptor = find_descriptor(descriptors, &unbacked.entity_class)?;
                let eye_height = descriptor
                    .movement
                    .as_ref()
                    .map_or(0.0, |movement| movement.capsule.eye_height);
                let pose = unbacked.pose;
                Some(Cow::Owned(MapEntity {
                    map_ordinal: None,
                    classname: "player_spawn".to_string(),
                    origin: pose.eye
                        - Vec3::Y * eye_height
                        - spawn_origin_shift(descriptor, agent_params),
                    angles: Vec3::new(pose.pitch, pose.yaw, 0.0),
                    key_values: [("entity_class".to_string(), unbacked.entity_class.clone())]
                        .into_iter()
                        .collect(),
                    tags: Vec::new(),
                }))
            }
        }
    }
}

/// Each remote seat's resolved spawn on the installed level. Level-scoped: an
/// unload clears it with the seat table's placement assignments.
#[derive(Default)]
pub(crate) struct SpawnPoseResolver {
    resolved: BTreeMap<Seat, ResolvedSpawn>,
    /// Seats whose spawn consumed a single-use pose and that have not resolved
    /// since. Level-scoped, like `resolved`.
    awaiting_resolution: BTreeSet<Seat>,
    unbacked_source: Option<Box<dyn UnbackedSpawnSource>>,
}

/// The spawn promotion materializes, and whether its client's hint must be
/// sent again: the seat resolved only now, or moved off its reserved start.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PromotionSpawn {
    pub(crate) spawn: ResolvedSpawn,
    pub(crate) rehint: bool,
}

/// The installed level a hint names, as one host poll or install reads it.
pub(crate) struct NamedLevel<'a> {
    pub(crate) placements: &'a SpawnPlacements<'a>,
    /// The parity identity install published. No level is named while none
    /// is installed, or while the host's frontend backdrop is.
    pub(crate) level_identity: &'a str,
    pub(crate) script_ctx: &'a postretro_entities::ScriptCtx,
}

impl NamedLevel<'_> {
    /// Respawn reads seat assignments from this snapshot, so every resolution
    /// republishes it.
    pub(crate) fn publish_reservations(&self, seats: &SeatTable) {
        crate::respawn_placements::publish(
            self.script_ctx,
            self.placements.spawn_points,
            self.placements.descriptors,
            Some(seats),
            None,
        );
    }

    /// Send `spawn`'s hint to `client_id` on Control, tagged with this install
    /// and the host level life (`None` on an uncatalogued level).
    pub(crate) fn send_hint(&self, server: &mut NetServer, client_id: u64, spawn: &ResolvedSpawn) {
        let hint = spawn.hint(
            self.level_identity.to_owned(),
            server.installed_level_life(),
        );
        server.send_control(
            client_id,
            postretro_net::wire::encode(&ServerControlMessage::SpawnPoseHint(hint)),
        );
    }
}

impl SpawnPoseResolver {
    /// Resolve `seat` afresh and record it. The unbacked source answers first,
    /// and a seat it answers holds no placement; otherwise the seat's placement
    /// assignment is made here, which reserves that start for it until unload.
    /// `None` when no start can be assigned (none resolves, or the seat has no
    /// carried record yet).
    pub(crate) fn resolve(
        &mut self,
        seat: Seat,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        placements: &SpawnPlacements<'_>,
    ) -> Option<ResolvedSpawn> {
        self.awaiting_resolution.remove(&seat);
        if let Some(unbacked) = self
            .unbacked_source
            .as_ref()
            .and_then(|source| source.pose_for(seat))
        {
            seats.release_placement(seat);
            let spawn = ResolvedSpawn::Unbacked(unbacked);
            self.resolved.insert(seat, spawn.clone());
            return Some(spawn);
        }
        self.resolve_placement(seat, seats, registry, placements)
    }

    /// Resolve `seat` on `level`, republish respawn's reservations and send
    /// its client the hint: every site that first names an installed level to
    /// a seat, and a seat whose spawn consumed its single-use pose once it is
    /// demoted.
    pub(crate) fn resolve_and_hint(
        &mut self,
        level: &NamedLevel<'_>,
        server: &mut NetServer,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        seat: Seat,
        client_id: u64,
    ) -> Option<ResolvedSpawn> {
        let spawn = self.resolve(seat, seats, registry, level.placements);
        level.publish_reservations(seats);
        let spawn = spawn?;
        level.send_hint(server, client_id, &spawn);
        Some(spawn)
    }

    /// The spawn promotion materializes. A recorded unbacked pose stays as
    /// recorded. A recorded placement goes through the seat's sticky
    /// assignment, which keeps it unless a live pawn now stands on it; then
    /// the seat takes a free start and needs a fresh hint. A seat with no
    /// record resolves now.
    pub(crate) fn resolve_for_promotion(
        &mut self,
        seat: Seat,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        placements: &SpawnPlacements<'_>,
    ) -> Option<PromotionSpawn> {
        let recorded = self.resolved.get(&seat).cloned();
        let spawn = match &recorded {
            Some(spawn @ ResolvedSpawn::Unbacked(_)) => spawn.clone(),
            Some(ResolvedSpawn::Placement { .. }) => {
                self.resolve_placement(seat, seats, registry, placements)?
            }
            None => self.resolve(seat, seats, registry, placements)?,
        };
        Some(PromotionSpawn {
            rehint: recorded.as_ref() != Some(&spawn),
            spawn,
        })
    }

    fn resolve_placement(
        &mut self,
        seat: Seat,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        placements: &SpawnPlacements<'_>,
    ) -> Option<ResolvedSpawn> {
        let map_index = seats.assign_joiner_placement(registry, seat, &placements.sites)?;
        let spawn = ResolvedSpawn::Placement {
            map_index,
            pose: placements.pose_at(map_index)?,
        };
        self.resolved.insert(seat, spawn.clone());
        Some(spawn)
    }

    /// The pose `seat` last resolved to on this level.
    #[cfg(test)]
    pub(crate) fn recorded(&self, seat: Seat) -> Option<&ResolvedSpawn> {
        self.resolved.get(&seat)
    }

    /// `seat`'s pawn spawned from its recorded pose. A single-use pose is
    /// consumed: its source stops offering it, the record goes, and the seat
    /// awaits a fresh resolution.
    pub(crate) fn spawned(&mut self, seat: Seat) {
        let Some(ResolvedSpawn::Unbacked(UnbackedSpawn {
            single_use: true, ..
        })) = self.resolved.get(&seat)
        else {
            return;
        };
        self.resolved.remove(&seat);
        self.awaiting_resolution.insert(seat);
        if let Some(source) = self.unbacked_source.as_mut() {
            source.consume(seat);
        }
    }

    /// After promotion spawned and bound `seat`'s pawn: a single-use pose is
    /// consumed, and a promotion that resolved the seat only now, or moved it
    /// off a start a live pawn stood on, hints that spawn after this
    /// promotion's marker. With no level named, nothing is sent.
    ///
    /// A consumed seat's next spawn is not resolved here: its client is still
    /// unarmed, and a hint now would move its view off the pose its pawn
    /// spawned at. [`Self::demoted`] resolves and hints it when the seat is
    /// demoted, and a rejoin within the hold resolves it at admission.
    pub(crate) fn finish_promotion(
        &mut self,
        promotion: &PromotionSpawn,
        level: Option<&NamedLevel<'_>>,
        server: &mut NetServer,
        seat: Seat,
        client_id: u64,
    ) {
        self.spawned(seat);
        if let Some(level) = level
            && promotion.rehint
        {
            level.send_hint(server, client_id, &promotion.spawn);
        }
    }

    /// `seat`, still connected as `client_id`, was demoted. A seat whose spawn
    /// consumed its single-use pose resolves afresh now, reserving its next
    /// spawn, and its client is hinted before any re-promotion's marker. Any
    /// other seat keeps its record and is sent nothing.
    pub(crate) fn demoted(
        &mut self,
        level: &NamedLevel<'_>,
        server: &mut NetServer,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        seat: Seat,
        client_id: u64,
    ) {
        if self.awaiting_resolution.contains(&seat) {
            self.resolve_and_hint(level, server, seats, registry, seat, client_id);
        }
    }

    pub(crate) fn clear_for_level_unload(&mut self) {
        self.resolved.clear();
        self.awaiting_resolution.clear();
    }

    /// No production source exists until a consumer supplies one; tests
    /// substitute theirs here.
    #[cfg(test)]
    pub(crate) fn set_unbacked_source(&mut self, source: Option<Box<dyn UnbackedSpawnSource>>) {
        self.unbacked_source = source;
    }
}

impl crate::App {
    /// Host install, after the host's pawn binds its start and the spawn points
    /// are retained: resolve every client-bound seat against this install and
    /// send each its hint on Control, after the Relevel install's identity
    /// stage sent. A level with no `player_spawn`, and a host frontend
    /// backdrop (which names no level), send none.
    pub(crate) fn send_install_spawn_pose_hints(&mut self) {
        let Some(level_identity) = self.published_level_identity.as_deref() else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (Some(crate::netcode::NetEndpoint::Host { server, .. }), Some(seats)) =
            (session.net_endpoint.as_mut(), session.seat_table.as_mut())
        else {
            return;
        };
        let ctx = &session.scripting.script_ctx;
        let data_registry = ctx.data_registry.borrow();
        let registry = ctx.registry.borrow();
        let Some(placements) = SpawnPlacements::of_level(
            &self.host_spawn_points,
            &data_registry.entities,
            self.nav_graph.as_ref().map(|graph| graph.agent_params()),
        ) else {
            return;
        };
        let level = NamedLevel {
            placements: &placements,
            level_identity,
            script_ctx: ctx,
        };
        let mut bound: Vec<(Seat, u64)> = server
            .connected_clients()
            .into_iter()
            .filter_map(|client_id| Some((seats.seat_for_client(client_id)?, client_id)))
            .collect();
        // Seat order, so lower seats take lower starts whatever the
        // transport's client order.
        bound.sort_unstable_by_key(|(seat, _)| *seat);
        for (seat, client_id) in bound {
            self.spawn_poses
                .resolve_and_hint(&level, server, seats, &registry, seat, client_id);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
