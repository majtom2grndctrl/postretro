//! Level seam inside test, latches, activation, occupancy and arrival queue.
//! See: context/lib/entity_model.md §7e

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, SeamComponent};
use postretro_level_format::environment_volumes::EnvironmentBrush;
use postretro_level_format::seams::{SeamActivation, SeamRecord, SeamsSection};

use crate::sim::player_life::TriggerStageCapabilities;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, canonical_player_capsules};

/// An installed seam's landmark: the record's resolved origin and yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamLandmark {
    pub origin: Vec3,
    pub yaw: f32,
}

/// The upright capsule one inside test reads: centre, radius and the half
/// height of its axis segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamCapsule {
    pub center: Vec3,
    pub radius: f32,
    pub half_height: f32,
}

/// A level's seams, indexed in map order, with the host-side state the
/// trigger stage keeps for them. Latches never leave the host: no wire field
/// mirrors them, unlike teleporter latches.
#[derive(Debug, Default)]
pub struct SeamStage {
    records: Vec<SeamRecord>,
    entities: Vec<Option<EntityId>>,
    /// Players whose capsule overlapped each seam after the last stage run.
    inside: Vec<BTreeSet<PlayerId>>,
    /// Touch seams armed from disarmed since the last stage run.
    reactivation: Vec<bool>,
    /// `(pawn, seam)` latches: set at spawn, cleared when the capsule leaves.
    latches: BTreeSet<(EntityId, u16)>,
    /// Seams activated since the frame-end pick last took them, each once.
    /// The pick orders them by index, so the first in map order wins.
    activated: Vec<u16>,
    /// Arrivals spawned since the last stage run, each firing `arrive` once.
    arrivals: Vec<(u16, EntityId)>,
    /// A frontend backdrop's seams never activate.
    inert: bool,
    /// Seams whose unknown target already warned this level.
    warned_unknown_target: BTreeSet<u16>,
    warned_duplicate_players: HashSet<PlayerId>,
}

impl SeamStage {
    /// Install a level's seams. `inert` marks a frontend backdrop.
    pub fn install(section: &SeamsSection, inert: bool) -> Self {
        let n = section.seams.len();
        Self {
            records: section.seams.clone(),
            entities: vec![None; n],
            inside: vec![BTreeSet::new(); n],
            reactivation: vec![false; n],
            inert,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_inert(&self) -> bool {
        self.inert
    }

    pub fn records(&self) -> &[SeamRecord] {
        &self.records
    }

    pub fn record(&self, index: u16) -> Option<&SeamRecord> {
        self.records.get(usize::from(index))
    }

    /// The seam of `name`, which the compiler keeps unique per map.
    pub fn index_by_name(&self, name: &str) -> Option<u16> {
        self.records
            .iter()
            .position(|record| record.name == name)
            .and_then(|index| u16::try_from(index).ok())
    }

    pub fn landmark(&self, index: u16) -> Option<SeamLandmark> {
        self.record(index).map(|record| SeamLandmark {
            origin: Vec3::from(record.landmark_origin),
            yaw: record.landmark_yaw,
        })
    }

    pub fn entity(&self, index: u16) -> Option<EntityId> {
        self.entities.get(usize::from(index)).copied().flatten()
    }

    pub fn bind_entity(&mut self, index: u16, entity: EntityId) {
        if let Some(binding) = self.entities.get_mut(usize::from(index)) {
            *binding = Some(entity);
        }
    }

    /// Whether seam `index` is armed now: its bound component, else its
    /// authored spawn state.
    pub fn armed(&self, registry: &EntityRegistry, index: u16) -> bool {
        self.entity(index)
            .and_then(|id| registry.get_component::<SeamComponent>(id).ok())
            .map_or_else(
                || {
                    self.record(index)
                        .is_some_and(|record| record.enabled_on_spawn)
                },
                |seam| seam.armed,
            )
    }

    /// The one inside test: the upright capsule against the union of the
    /// seam's convex brushes. Each brush's planes inflate by the radius and
    /// clip the capsule's axis segment, which over-includes near edges by at
    /// most the radius.
    pub fn contains(&self, index: u16, capsule: SeamCapsule) -> bool {
        self.record(index).is_some_and(|record| {
            record
                .brushes
                .iter()
                .any(|brush| capsule_overlaps_brush(capsule, brush))
        })
    }

    /// Every seam the capsule overlaps, in map order.
    pub fn containing(&self, capsule: SeamCapsule) -> impl Iterator<Item = u16> + '_ {
        (0..self.records.len())
            .filter_map(|index| u16::try_from(index).ok())
            .filter(move |index| self.contains(*index, capsule))
    }

    /// Latch a just-spawned pawn against every seam its capsule overlaps,
    /// armed or not, before any stage run sees it. Every spawn and respawn
    /// calls this; a teleport never does.
    pub fn latch_spawned(&mut self, pawn: EntityId, capsule: SeamCapsule) {
        self.latches.retain(|(latched, _)| *latched != pawn);
        let overlapped: Vec<u16> = self.containing(capsule).collect();
        self.latches
            .extend(overlapped.into_iter().map(|index| (pawn, index)));
    }

    /// `latch_spawned` from the pawn's registry pose; a pawn without a
    /// movement capsule latches nothing.
    pub fn latch_spawned_in(&mut self, registry: &EntityRegistry, pawn: EntityId) {
        if let Some(capsule) = pawn_capsule(registry, pawn) {
            self.latch_spawned(pawn, capsule);
        }
    }

    pub fn is_latched(&self, pawn: EntityId, index: u16) -> bool {
        self.latches.contains(&(pawn, index))
    }

    /// Queue `arrive` for a pawn spawned at seam `index`'s arrival, firing
    /// inside the next stage run.
    pub fn queue_arrival(&mut self, index: u16, pawn: EntityId) {
        self.arrivals.push((index, pawn));
    }

    /// Arrivals to dispatch this tick, in spawn order.
    pub fn take_arrivals(&mut self) -> Vec<(u16, EntityId)> {
        std::mem::take(&mut self.arrivals)
    }

    /// Record seams armed from disarmed at this tick's start: a touch seam
    /// then admits its unlatched standing occupants, as a trigger does.
    pub fn note_newly_armed(&mut self, newly_armed: &[u16]) {
        for index in newly_armed {
            if let Some(pending) = self.reactivation.get_mut(usize::from(*index)) {
                *pending = true;
            }
        }
    }

    /// True the first time seam `index`'s unknown target is reported.
    pub fn warn_unknown_target_once(&mut self, index: u16) -> bool {
        self.warned_unknown_target.insert(index)
    }

    /// Seams activated since the last take, for the frame-end pick.
    pub fn take_activations(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.activated)
    }

    /// Evaluate every seam for this tick's players: clear latches whose
    /// capsule left, record activations, and return each seam with a counted
    /// pawn inside, armed or not, for the occupancy families.
    pub fn run_tick(
        &mut self,
        registry: &EntityRegistry,
        players: &[AuthoritativePlayer],
        capabilities: &TriggerStageCapabilities,
        use_pressed: &HashMap<PlayerId, bool>,
    ) -> Vec<(EntityId, Vec<EntityId>)> {
        // Most levels have no seam: the stage costs nothing there.
        if self.records.is_empty() {
            return Vec::new();
        }
        let capsules: BTreeMap<PlayerId, (EntityId, SeamCapsule)> =
            canonical_player_capsules(registry, players, &mut self.warned_duplicate_players)
                .into_iter()
                .map(|(player, (pawn, center, radius, half_height))| {
                    (
                        player,
                        (
                            pawn,
                            SeamCapsule {
                                center,
                                radius,
                                half_height,
                            },
                        ),
                    )
                })
                .collect();
        self.latches.retain(|(pawn, _)| registry.exists(*pawn));
        let mut occupied = Vec::new();
        for slot in 0..self.records.len() {
            let Ok(index) = u16::try_from(slot) else {
                break;
            };
            let armed = self.armed(registry, index);
            let activation = self.records[slot].activation;
            let reactivate =
                std::mem::take(&mut self.reactivation[slot]) && activation == SeamActivation::Touch;
            let mut now_inside = BTreeSet::new();
            let mut counted = Vec::new();
            let mut fires = false;
            for (&player, &(pawn, capsule)) in &capsules {
                if !self.contains(index, capsule) {
                    self.latches.remove(&(pawn, index));
                    continue;
                }
                now_inside.insert(player);
                if capabilities.counts_occupant(player) {
                    counted.push(pawn);
                }
                if self.inert || !armed || self.latches.contains(&(pawn, index)) {
                    continue;
                }
                fires |= match activation {
                    SeamActivation::Touch => {
                        (!self.inside[slot].contains(&player) || reactivate)
                            && capabilities.permits_seam_touch(player)
                    }
                    SeamActivation::Use => {
                        use_pressed.get(&player).copied().unwrap_or(false)
                            && capabilities.permits_seam_use(player)
                    }
                };
            }
            self.inside[slot] = now_inside;
            if fires && !self.activated.contains(&index) {
                self.activated.push(index);
            }
            if !counted.is_empty()
                && let Some(entity) = self.entity(index)
            {
                counted.sort_unstable();
                occupied.push((entity, counted));
            }
        }
        occupied
    }
}

/// A movement pawn's upright capsule at its registry pose.
pub fn pawn_capsule(registry: &EntityRegistry, pawn: EntityId) -> Option<SeamCapsule> {
    let movement = registry
        .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
        .ok()?;
    let transform = registry
        .get_component::<postretro_entities::Transform>(pawn)
        .ok()?;
    Some(SeamCapsule {
        center: transform.position,
        radius: movement.capsule.radius,
        half_height: movement.capsule.half_height,
    })
}

/// The capsule's axis segment clipped by every plane of the brush, each moved
/// out by the radius: a non-empty remainder overlaps.
fn capsule_overlaps_brush(capsule: SeamCapsule, brush: &EnvironmentBrush) -> bool {
    let r = capsule.radius;
    let c = capsule.center;
    let min = Vec3::from(brush.aabb_min) - Vec3::splat(r);
    let max = Vec3::from(brush.aabb_max) + Vec3::splat(r);
    let axis = Vec3::Y * capsule.half_height;
    if c.x < min.x
        || c.x > max.x
        || c.z < min.z
        || c.z > max.z
        || c.y + capsule.half_height < min.y
        || c.y - capsule.half_height > max.y
    {
        return false;
    }
    let (start, end) = (c - axis, c + axis);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for plane in &brush.planes {
        let normal = Vec3::from(plane.normal);
        let distance = plane.distance + r;
        let d0 = normal.dot(start) - distance;
        let d1 = normal.dot(end) - distance;
        if d0 > 0.0 && d1 > 0.0 {
            return false;
        }
        if d0 > 0.0 {
            t0 = t0.max(d0 / (d0 - d1));
        } else if d1 > 0.0 {
            t1 = t1.min(d0 / (d0 - d1));
        }
        if t0 > t1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;
