//! Host-authoritative touch evaluation for world wieldables.
//! See: context/lib/entity_model.md §7.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::Vec3;
use parry3d::math::{Isometry, Point, Vector};
use parry3d::query::{ShapeCastOptions, cast_shapes, intersection_test};
use parry3d::shape::Ball;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::touchable::TouchableComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    AmmoReserve, ComponentKind, DeferredEffectComponent, DescriptorProvenance, EntityId,
    EntityRegistry, EntityTypeDescriptor, TouchMode, Transform,
};

use crate::collision::{COS_WALKABLE, CollisionWorld, SKIN_DISTANCE, cast_ray};
use crate::scripting::builtins::data_archetype::{descriptor_mesh_component, find_descriptor};
use crate::scripting::builtins::wieldable_inventory::{acquire_wieldable, release_wieldable};
use crate::sim::weapon_stage::transition_to_idle;
use crate::trigger_system::{
    AuthoritativePlayer, PlayerId, canonical_player_capsules, range_distance,
};

/// Policy inputs computed for every overlapping player/item pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TouchFacts {
    pub(crate) owned_count: u32,
    pub(crate) free_slots: u32,
    pub(crate) magazine: u32,
    pub(crate) reserve: u32,
    pub(crate) pressed: bool,
}

/// Closed effect vocabulary returned by an item-touch policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchEffect {
    Acquire,
}

/// Default policy for the first touchable-wieldable behavior.
pub(crate) fn default_touch_policy(facts: &TouchFacts) -> Vec<TouchEffect> {
    if facts.owned_count == 0 && facts.free_slots > 0 {
        vec![TouchEffect::Acquire]
    } else {
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct OverlapPair {
    entered: bool,
    distance_squared: f32,
}

#[derive(Debug)]
struct TouchEvaluation {
    player: PlayerId,
    pawn: EntityId,
    distance_squared: f32,
    contests: bool,
    effects: Vec<TouchEffect>,
}

/// Observable touch-side changes produced by one authoritative fixed tick.
#[derive(Debug, Default)]
pub(crate) struct TouchTickEvents {
    pub(crate) repointed_pawns: Vec<EntityId>,
    pub(crate) dropped_item_meshes: Vec<EntityId>,
}

/// Per-level state for deterministic item touch, touch edges, and prompts.
///
/// Sorted occupancy keys make edge emission stable across equivalent input
/// orderings. The policy remains pure; the pass mutates the registry only after
/// reducing one item's contestants to at most one winner.
#[derive(Debug)]
pub(crate) struct TouchSystem {
    occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>,
    warned_duplicate_players: HashSet<PlayerId>,
    warned_non_touchable_descriptors: HashSet<String>,
    policy: fn(&TouchFacts) -> Vec<TouchEffect>,
    pub(crate) prompts: Vec<(PlayerId, EntityId)>,
}

impl Default for TouchSystem {
    fn default() -> Self {
        Self {
            occupants: BTreeMap::new(),
            warned_duplicate_players: HashSet::new(),
            warned_non_touchable_descriptors: HashSet::new(),
            policy: default_touch_policy,
            prompts: Vec::new(),
        }
    }
}

impl TouchSystem {
    pub(crate) fn clear(&mut self) {
        self.occupants.clear();
        self.warned_duplicate_players.clear();
        self.warned_non_touchable_descriptors.clear();
        self.prompts.clear();
    }

    /// Runs after trigger dispatch and before AI on every host/single-player
    /// fixed tick. Drop placement shares this pass so release, occupancy
    /// seeding, and touch evaluation use one authoritative ordering.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_authoritative_tick(
        &mut self,
        registry: &mut EntityRegistry,
        collision_world: &CollisionWorld,
        descriptors: &[EntityTypeDescriptor],
        players: &[AuthoritativePlayer],
        use_pressed: &HashMap<PlayerId, bool>,
        drop_pressed: &HashMap<PlayerId, bool>,
    ) -> TouchTickEvents {
        self.prompts.clear();

        let player_capsules =
            canonical_player_capsules(registry, players, &mut self.warned_duplicate_players);
        let mut tick_events = self.drop_wieldables(
            registry,
            collision_world,
            descriptors,
            &player_capsules,
            drop_pressed,
        );
        let mut item_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::Touchable)
            .map(|(id, _)| id)
            .collect();
        item_ids.sort_unstable();

        let current_items: BTreeSet<EntityId> = item_ids.iter().copied().collect();
        self.occupants
            .retain(|item, _| current_items.contains(item));

        let mut overlaps: BTreeMap<EntityId, BTreeMap<PlayerId, OverlapPair>> = BTreeMap::new();
        let mut modes = BTreeMap::new();

        for item in &item_ids {
            if registry
                .is_marked_for_end_of_frame_removal(*item)
                .unwrap_or(true)
            {
                self.occupants.remove(item);
                continue;
            }
            let (Ok(touchable), Ok(transform)) = (
                registry.get_component::<TouchableComponent>(*item),
                registry.get_component::<Transform>(*item),
            ) else {
                self.occupants.remove(item);
                continue;
            };

            modes.insert(*item, touchable.mode);
            let current_overlaps: BTreeMap<PlayerId, f32> = player_capsules
                .iter()
                .filter_map(|(&player, &(_, center, capsule_radius, half_height))| {
                    sphere_overlaps_capsule(
                        transform.position,
                        touchable.radius,
                        center,
                        capsule_radius,
                        half_height,
                    )
                    .then_some((
                        player,
                        sphere_capsule_distance_squared(transform.position, center, half_height),
                    ))
                })
                .collect();

            let occupants = self.occupants.entry(*item).or_default();
            occupants.retain(|player| current_overlaps.contains_key(player));
            let mut item_overlaps = BTreeMap::new();
            for (player, distance_squared) in current_overlaps {
                let entered = occupants.insert(player);
                item_overlaps.insert(
                    player,
                    OverlapPair {
                        entered,
                        distance_squared,
                    },
                );
            }
            overlaps.insert(*item, item_overlaps);
        }

        let claims = press_claims(registry, use_pressed, &modes, &overlaps, &player_capsules);
        let mut repointed = BTreeSet::new();

        for item in item_ids {
            let Some(mode) = modes.get(&item).copied() else {
                continue;
            };
            let Some(item_overlaps) = overlaps.get(&item) else {
                continue;
            };

            let mut evaluations = Vec::with_capacity(item_overlaps.len());
            for (&player, overlap) in item_overlaps {
                let Some(&(pawn, _, _, _)) = player_capsules.get(&player) else {
                    continue;
                };
                let facts = touch_facts(registry, pawn, item, player, use_pressed);
                let effects = (self.policy)(&facts);
                let edge_contests = match mode {
                    TouchMode::Auto => overlap.entered,
                    TouchMode::Press => claims.get(&player) == Some(&item),
                };
                evaluations.push(TouchEvaluation {
                    player,
                    pawn,
                    distance_squared: overlap.distance_squared,
                    // A player whose policy declined the item is not a wanter
                    // and therefore cannot block another player's acquisition.
                    contests: edge_contests && !effects.is_empty(),
                    effects,
                });
            }

            let winner = evaluations
                .iter()
                .filter(|evaluation| evaluation.contests)
                .min_by(|left, right| {
                    left.distance_squared
                        .total_cmp(&right.distance_squared)
                        .then_with(|| left.player.cmp(&right.player))
                });
            let effects_applied = winner.is_some_and(|winner| {
                let applied = apply_effects(registry, winner.pawn, item, &winner.effects);
                if applied {
                    repointed.insert(winner.pawn);
                }
                applied
            });

            // A successfully acquired item no longer exists in the world, so
            // no player may receive a prompt for its removed affordance.
            if !effects_applied && mode == TouchMode::Press {
                for evaluation in &evaluations {
                    if !evaluation.effects.is_empty() {
                        self.prompts.push((evaluation.player, item));
                    }
                }
            }
        }

        tick_events.repointed_pawns.extend(repointed);
        tick_events.repointed_pawns.sort_unstable();
        tick_events.repointed_pawns.dedup();
        tick_events.dropped_item_meshes.sort_unstable();
        tick_events.dropped_item_meshes.dedup();
        tick_events
    }

    fn drop_wieldables(
        &mut self,
        registry: &mut EntityRegistry,
        collision_world: &CollisionWorld,
        descriptors: &[EntityTypeDescriptor],
        player_capsules: &BTreeMap<PlayerId, (EntityId, Vec3, f32, f32)>,
        drop_pressed: &HashMap<PlayerId, bool>,
    ) -> TouchTickEvents {
        let mut events = TouchTickEvents::default();

        for (&player, &(pawn, center, capsule_radius, capsule_half_height)) in player_capsules {
            if !drop_pressed.get(&player).copied().unwrap_or(false) {
                continue;
            }
            let Ok(inventory) = registry.get_component::<Inventory>(pawn) else {
                continue;
            };
            let active_slot = inventory.active_slot;
            let Some(item) = inventory.active_wieldable() else {
                continue;
            };
            let Ok(provenance) = registry.get_component::<DescriptorProvenance>(item) else {
                continue;
            };
            let canonical_name = provenance.canonical_name.clone();
            let Some(descriptor) = find_descriptor(descriptors, &canonical_name) else {
                continue;
            };
            let Some(touchable) = descriptor.touchable.as_ref() else {
                if self
                    .warned_non_touchable_descriptors
                    .insert(canonical_name.clone())
                {
                    log::warn!(
                        "[Touch] refuses dropping wieldable `{canonical_name}`: descriptor has no touchable block"
                    );
                }
                continue;
            };
            let Ok(pawn_transform) = registry.get_component::<Transform>(pawn) else {
                continue;
            };
            if registry.get_component::<Transform>(item).is_err() {
                continue;
            }
            let Some(drop_position) = resolve_drop_position(
                collision_world,
                pawn_transform,
                center,
                capsule_radius,
                capsule_half_height,
                touchable.radius,
            ) else {
                continue;
            };

            if release_wieldable(registry, pawn, active_slot) != Some(item) {
                continue;
            }

            let mut item_transform = *registry
                .get_component::<Transform>(item)
                .expect("checked live wieldables retain their transform");
            item_transform.position = drop_position;
            let _ = registry.set_presentation_transform(item, item_transform);
            let _ = registry.set_component(item, TouchableComponent::from_descriptor(touchable));

            let _ = registry.remove_component::<MeshComponent>(item);
            if let Some(mesh) = descriptor_mesh_component(descriptor, None) {
                let _ = registry.set_component(item, mesh);
                events.dropped_item_meshes.push(item);
            }

            if let Ok(mut weapon) = registry.get_component::<WeaponComponent>(item).cloned() {
                // A dropped weapon must match a fresh component in every
                // live-state field while preserving its descriptor tuning and
                // magazine. Add future live state here with the same rule.
                transition_to_idle(&mut weapon);
                weapon.cooldown_remaining_ms = 0.0;
                weapon.shoot_press_consumed = false;
                weapon.reload_press_consumed = false;
                weapon.reload_feedback = Default::default();
                let _ = registry.set_component(item, weapon);
            }

            let occupants = self.occupants.entry(item).or_default();
            occupants.clear();
            for (&occupant, &(_, occupant_center, occupant_radius, occupant_half_height)) in
                player_capsules
            {
                if sphere_overlaps_capsule(
                    drop_position,
                    touchable.radius,
                    occupant_center,
                    occupant_radius,
                    occupant_half_height,
                ) {
                    occupants.insert(occupant);
                }
            }
            events.repointed_pawns.push(pawn);
        }

        events
    }
}

fn resolve_drop_position(
    collision_world: &CollisionWorld,
    pawn_transform: &Transform,
    capsule_center: Vec3,
    capsule_radius: f32,
    capsule_half_height: f32,
    item_radius: f32,
) -> Option<Vec3> {
    // These are deliberately a small, fixed forward-only search rather than a
    // radial search around the pawn. A drop should read as a gentle toss in the
    // facing direction. Placement candidates are forward-only; final
    // sphere-clearance still considers all nearby static geometry.
    const FORWARD_DISTANCES: [f32; 3] = [1.0, 0.75, 1.25];
    const MAX_FLOOR_BELOW_FEET: f32 = 1.0;

    if !capsule_center.is_finite()
        || !capsule_radius.is_finite()
        || !capsule_half_height.is_finite()
        || capsule_radius < 0.0
        || capsule_half_height < 0.0
        || !item_radius.is_finite()
        || item_radius <= 0.0
    {
        return None;
    }

    let forward = (pawn_transform.rotation * Vec3::NEG_Z)
        .with_y(0.0)
        .normalize_or_zero();
    let forward = if forward == Vec3::ZERO {
        Vec3::NEG_Z
    } else {
        forward
    };

    let max_ground_distance = capsule_half_height + capsule_radius + MAX_FLOOR_BELOW_FEET;
    for forward_distance in FORWARD_DISTANCES {
        let probe_origin = capsule_center + forward * forward_distance;
        let Some(floor) = cast_ray(
            collision_world,
            Point::new(probe_origin.x, probe_origin.y, probe_origin.z),
            Vector::new(0.0, -1.0, 0.0),
            max_ground_distance,
        ) else {
            continue;
        };
        let floor_normal = Vec3::new(floor.normal.x, floor.normal.y, floor.normal.z);
        let normal_length_squared = floor_normal.length_squared();
        if !floor_normal.is_finite() || normal_length_squared <= f32::EPSILON {
            continue;
        }
        let floor_normal = floor_normal / normal_length_squared.sqrt();
        if floor_normal.y < COS_WALKABLE {
            continue;
        }

        // The touchable radius is the item's world-collision radius as well
        // as its pickup volume. Separate it along the surface normal, then retain
        // the existing fail-closed clearance query for surrounding geometry.
        let hit_point = Vec3::new(
            probe_origin.x,
            probe_origin.y - floor.time_of_impact,
            probe_origin.z,
        );
        let position = hit_point + floor_normal * (item_radius + SKIN_DISTANCE);
        if sphere_fits_world(collision_world, position, item_radius) {
            return Some(position);
        }
    }

    None
}

/// A zero-length sphere cast treats contact and penetration as an obstruction;
/// the intersection fallback keeps an unsupported shape pair fail-closed.
fn sphere_fits_world(collision_world: &CollisionWorld, position: Vec3, radius: f32) -> bool {
    if !position.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return false;
    }

    let sphere = Ball::new(radius);
    let sphere_isometry = Isometry::translation(position.x, position.y, position.z);
    let options = ShapeCastOptions {
        max_time_of_impact: 0.0,
        target_distance: 0.0,
        stop_at_penetration: true,
        ..Default::default()
    };
    let cast_hits = cast_shapes(
        &sphere_isometry,
        &Vector::zeros(),
        &sphere,
        &collision_world.isometry,
        &Vector::zeros(),
        &collision_world.mesh,
        options,
    )
    .is_ok_and(|hit| hit.is_some());
    !cast_hits
        && intersection_test(
            &sphere_isometry,
            &sphere,
            &collision_world.isometry,
            &collision_world.mesh,
        )
        .is_ok_and(|intersects| !intersects)
}

/// Squared distance from the sphere centre to the closest point of an upright
/// capsule's axis segment. Capsule radius is deliberately not included here;
/// callers add it when testing overlap.
pub(crate) fn sphere_capsule_distance_squared(
    sphere_center: Vec3,
    capsule_center: Vec3,
    capsule_half_height: f32,
) -> f32 {
    let axis_min_y = capsule_center.y - capsule_half_height.max(0.0);
    let axis_max_y = capsule_center.y + capsule_half_height.max(0.0);
    let horizontal_x = sphere_center.x - capsule_center.x;
    let horizontal_z = sphere_center.z - capsule_center.z;
    let vertical = range_distance(sphere_center.y, axis_min_y, axis_max_y);
    horizontal_x * horizontal_x + vertical * vertical + horizontal_z * horizontal_z
}

/// Exact overlap test for a fixed-radius item sphere and an upright player
/// capsule. This is the canonical pickup-volume geometry used by both touch
/// edges and contender ranking.
pub(crate) fn sphere_overlaps_capsule(
    sphere_center: Vec3,
    sphere_radius: f32,
    capsule_center: Vec3,
    capsule_radius: f32,
    capsule_half_height: f32,
) -> bool {
    if !sphere_center.is_finite()
        || !capsule_center.is_finite()
        || !sphere_radius.is_finite()
        || !capsule_radius.is_finite()
        || !capsule_half_height.is_finite()
        || sphere_radius < 0.0
        || capsule_radius < 0.0
    {
        return false;
    }
    let combined_radius = sphere_radius + capsule_radius;
    sphere_capsule_distance_squared(sphere_center, capsule_center, capsule_half_height)
        <= combined_radius * combined_radius
}

fn press_claims(
    registry: &EntityRegistry,
    use_pressed: &HashMap<PlayerId, bool>,
    modes: &BTreeMap<EntityId, TouchMode>,
    overlaps: &BTreeMap<EntityId, BTreeMap<PlayerId, OverlapPair>>,
    player_capsules: &BTreeMap<PlayerId, (EntityId, Vec3, f32, f32)>,
) -> BTreeMap<PlayerId, EntityId> {
    let mut claims = BTreeMap::new();
    for (&item, item_overlaps) in overlaps {
        if modes.get(&item) != Some(&TouchMode::Press) {
            continue;
        }
        for (&player, overlap) in item_overlaps {
            if !use_pressed.get(&player).copied().unwrap_or(false) {
                continue;
            }
            let Some(&(pawn, _, _, _)) = player_capsules.get(&player) else {
                continue;
            };
            if player_owns_item(registry, pawn, item) {
                continue;
            }
            let replace = claims.get(&player).is_none_or(|current| {
                let current_distance = overlaps
                    .get(current)
                    .and_then(|pairs| pairs.get(&player))
                    .expect("claim candidates remain in the overlap table")
                    .distance_squared;
                overlap
                    .distance_squared
                    .total_cmp(&current_distance)
                    .is_lt()
                    || (overlap
                        .distance_squared
                        .total_cmp(&current_distance)
                        .is_eq()
                        && item < *current)
            });
            if replace {
                claims.insert(player, item);
            }
        }
    }
    claims
}

fn touch_facts(
    registry: &EntityRegistry,
    pawn: EntityId,
    item: EntityId,
    player: PlayerId,
    use_pressed: &HashMap<PlayerId, bool>,
) -> TouchFacts {
    let owned_count = owned_count(registry, pawn, item);
    let free_slots = registry
        .get_component::<Inventory>(pawn)
        .map(|inventory| {
            inventory
                .wieldables
                .iter()
                .filter(|slot| slot.is_none())
                .count() as u32
        })
        .unwrap_or(0);
    let weapon = registry.get_component::<WeaponComponent>(item).ok();
    let magazine = weapon.map_or(0, |weapon| weapon.magazine);
    let reserve = weapon
        .and_then(|weapon| weapon.ammo.as_ref())
        .and_then(|ammo| {
            registry
                .get_component::<AmmoReserve>(pawn)
                .ok()
                .map(|reserve| reserve.available(&ammo.ammo_type))
        })
        .unwrap_or(0);

    TouchFacts {
        owned_count,
        free_slots,
        magazine,
        reserve,
        pressed: use_pressed.get(&player).copied().unwrap_or(false),
    }
}

fn player_owns_item(registry: &EntityRegistry, pawn: EntityId, item: EntityId) -> bool {
    owned_count(registry, pawn, item) > 0
}

fn owned_count(registry: &EntityRegistry, pawn: EntityId, item: EntityId) -> u32 {
    let Ok(item_provenance) = registry.get_component::<DescriptorProvenance>(item) else {
        return 0;
    };
    let item_name = &item_provenance.canonical_name;
    registry
        .get_component::<Inventory>(pawn)
        .map(|inventory| {
            inventory
                .wieldables
                .iter()
                .flatten()
                .filter(|weapon| {
                    registry
                        .get_component::<DescriptorProvenance>(**weapon)
                        .is_ok_and(|provenance| provenance.canonical_name == *item_name)
                })
                .count() as u32
        })
        .unwrap_or(0)
}

fn apply_effects(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    item: EntityId,
    effects: &[TouchEffect],
) -> bool {
    let mut applied = false;
    for effect in effects {
        match effect {
            TouchEffect::Acquire => {
                if acquire_wieldable(registry, pawn, item).is_some() {
                    remove_world_item_components(registry, item);
                    applied = true;
                }
            }
        }
    }
    applied
}

fn remove_world_item_components(registry: &mut EntityRegistry, item: EntityId) {
    let _ = registry.remove_component::<TouchableComponent>(item);
    let _ = registry.remove_component::<MeshComponent>(item);
    if let Ok(deferred) = registry
        .get_component::<DeferredEffectComponent>(item)
        .cloned()
    {
        let mut deferred = deferred;
        deferred.pending.clear();
        deferred.overflow_reported = false;
        let _ = registry.set_component(item, deferred);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    use glam::Quat;
    use log::Level;
    use parry3d::math::Point;
    use parry3d::shape::TriMesh;
    use postretro_entities::ComponentKind;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    use postretro_entities::components::weapon::ReloadFeedback;
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::data_descriptors::MeshDescriptor;
    use postretro_entities::provenance::{
        DescriptorComponentKind, DescriptorMapOverride, DescriptorSpawnPath,
    };
    use postretro_foundation::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        PlayerMovementComponent, PlayerMovementDescriptor, ReloadStyle, ResolutionMode, Seat,
        SpeedParams, TouchableDescriptor, WeaponDescriptor, WeaponResource,
    };
    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::netcode::SeatTable;
    use crate::scripting::builtins::wieldable_inventory::compose_wieldable_inventory_from_slots;
    use crate::scripting::map_entity::MapEntity;

    fn movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        })
    }

    /// Broad walkable floor for drop tests. Keeping it explicit makes the
    /// drop contract physical: a drop only succeeds when a forward floor can
    /// actually receive the touchable sphere.
    fn floor_world() -> CollisionWorld {
        let points = vec![
            Point::new(-100.0, 0.0, -100.0),
            Point::new(100.0, 0.0, -100.0),
            Point::new(100.0, 0.0, 100.0),
            Point::new(-100.0, 0.0, 100.0),
        ];
        CollisionWorld {
            mesh: TriMesh::new(points, vec![[0, 1, 2], [0, 2, 3]]),
            isometry: Isometry::identity(),
        }
    }

    fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        let pawn = registry.spawn(Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry.set_component(pawn, movement()).unwrap();
        registry.set_component(pawn, Inventory::default()).unwrap();
        registry
            .set_component(pawn, AmmoReserve::default())
            .unwrap();
        pawn
    }

    fn weapon(canonical_name: &str, magazine: u32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: Some(canonical_name.to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "cells".to_string(),
                magazine,
                cost_per_shot: 1,
                reserve: 0,
                reload_ms: 1000,
                reload_style: ReloadStyle::Magazine,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        })
    }

    fn provenance(canonical_name: &str) -> DescriptorProvenance {
        DescriptorProvenance {
            canonical_name: canonical_name.to_string(),
            owned_components: BTreeSet::from([
                DescriptorComponentKind::Weapon,
                DescriptorComponentKind::Touchable,
            ]),
            map_overrides: BTreeSet::<DescriptorMapOverride>::new(),
            spawn_path: DescriptorSpawnPath::MapPlacement,
        }
    }

    fn spawn_item(
        registry: &mut EntityRegistry,
        canonical_name: &str,
        position: Vec3,
        mode: TouchMode,
        magazine: u32,
    ) -> EntityId {
        let item = registry.spawn(Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry
            .set_component(item, TouchableComponent { mode, radius: 1.0 })
            .unwrap();
        registry
            .set_component(item, weapon(canonical_name, magazine))
            .unwrap();
        registry
            .set_component(item, provenance(canonical_name))
            .unwrap();
        registry
            .set_component(item, MeshComponent::stateless("test-item.glb".to_string()))
            .unwrap();
        item
    }

    fn players(pairs: &[(PlayerId, EntityId)]) -> Vec<AuthoritativePlayer> {
        pairs
            .iter()
            .map(|&(id, pawn)| AuthoritativePlayer { id, pawn })
            .collect()
    }

    fn tick(
        system: &mut TouchSystem,
        registry: &mut EntityRegistry,
        players: &[AuthoritativePlayer],
        pressed: &[(PlayerId, bool)],
    ) -> Vec<EntityId> {
        let pressed = pressed.iter().copied().collect();
        system
            .run_authoritative_tick(
                registry,
                &floor_world(),
                &[],
                players,
                &pressed,
                &HashMap::new(),
            )
            .repointed_pawns
    }

    fn tick_with_edges(
        system: &mut TouchSystem,
        registry: &mut EntityRegistry,
        collision_world: &CollisionWorld,
        descriptors: &[EntityTypeDescriptor],
        players: &[AuthoritativePlayer],
        use_pressed: &[(PlayerId, bool)],
        drop_pressed: &[(PlayerId, bool)],
    ) -> TouchTickEvents {
        let use_pressed = use_pressed.iter().copied().collect();
        let drop_pressed = drop_pressed.iter().copied().collect();
        system.run_authoritative_tick(
            registry,
            collision_world,
            descriptors,
            players,
            &use_pressed,
            &drop_pressed,
        )
    }

    fn drop_descriptor(canonical_name: &str, mode: TouchMode, radius: f32) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(canonical_name.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 100.0,
                cooldown_ms: 100.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                credit_source: Some(canonical_name.to_string()),
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: Some(WeaponResource::Ammo(AmmoResource {
                    ammo_type: "cells".to_string(),
                    magazine: 7,
                    cost_per_shot: 1,
                    reserve: 0,
                    reload_ms: 1000,
                    reload_style: ReloadStyle::Magazine,
                })),
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: Some(TouchableDescriptor { mode, radius }),
            mesh: Some(MeshDescriptor {
                model: "dropped-item.glb".to_string(),
                shadow_only: false,
                attachments: Default::default(),
                shadow_bias_scale: 1.0,
                animations: Default::default(),
                default_state: None,
                locomotion: None,
            }),
            health: None,
            behavior: None,
        }
    }

    fn held_item(registry: &mut EntityRegistry, pawn: EntityId, item: EntityId) {
        let _ = registry.remove_component::<TouchableComponent>(item);
        let _ = registry.remove_component::<MeshComponent>(item);
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(item);
        registry.set_component(pawn, inventory).unwrap();
    }

    #[test]
    fn auto_touch_acquires_and_removes_world_item_components() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let deferred = registry.deferred_effect_mut(item).unwrap();
        deferred.pending.push(postretro_entities::PendingEffect {
            kind: postretro_entities::DeferredEffectKind::SetHealth,
            remaining_us: 100,
            value: Some(1.0),
        });
        deferred.overflow_reported = true;
        let mut system = TouchSystem::default();

        let repointed = tick(
            &mut system,
            &mut registry,
            &players(&[(PlayerId::Local(pawn), pawn)]),
            &[],
        );

        assert_eq!(repointed, vec![pawn]);
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
        assert!(
            !registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap()
        );
        assert!(
            !registry
                .has_component_kind(item, ComponentKind::Mesh)
                .unwrap()
        );
        let deferred = registry
            .get_component::<DeferredEffectComponent>(item)
            .unwrap();
        assert!(deferred.pending.is_empty());
        assert!(!deferred.overflow_reported);
        assert!(!deferred.inert);
        assert!(system.prompts.is_empty());
    }

    #[test]
    fn simultaneous_auto_contest_prefers_nearest_then_lower_player_id_deterministically() {
        let mut registry = EntityRegistry::new();
        let farther = spawn_player(&mut registry, Vec3::new(-0.6, 0.0, 0.0));
        let nearer = spawn_player(&mut registry, Vec3::new(0.1, 0.0, 0.0));
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let mut system = TouchSystem::default();

        tick(
            &mut system,
            &mut registry,
            &players(&[
                (PlayerId::Remote(3), farther),
                (PlayerId::Remote(7), nearer),
            ]),
            &[],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(nearer)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the nearest contestant wins even when it has the higher PlayerId"
        );
        assert!(
            registry
                .get_component::<Inventory>(farther)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none)
        );

        for player_order_is_reversed in [false, true] {
            let mut registry = EntityRegistry::new();
            let lower_id = spawn_player(&mut registry, Vec3::new(-0.25, 0.0, 0.0));
            let higher_id = spawn_player(&mut registry, Vec3::new(0.25, 0.0, 0.0));
            let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
            let mut system = TouchSystem::default();
            let mut players = players(&[
                (PlayerId::Remote(3), lower_id),
                (PlayerId::Remote(7), higher_id),
            ]);
            if player_order_is_reversed {
                players.reverse();
            }

            tick(&mut system, &mut registry, &players, &[]);

            assert_eq!(
                registry
                    .get_component::<Inventory>(lower_id)
                    .unwrap()
                    .wieldables[0],
                Some(item),
                "an equal-distance contest resolves to the lower PlayerId regardless of input order"
            );
            assert!(
                registry
                    .get_component::<Inventory>(higher_id)
                    .unwrap()
                    .wieldables
                    .iter()
                    .all(Option::is_none)
            );
        }
    }

    #[test]
    fn duplicate_or_full_press_item_is_not_prompt_eligible() {
        let mut registry = EntityRegistry::new();
        let duplicate_owner = spawn_player(&mut registry, Vec3::ZERO);
        let full_owner = spawn_player(&mut registry, Vec3::new(5.0, 0.0, 0.0));
        let owned = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 0.0, 0.0),
            TouchMode::Auto,
            7,
        );
        let duplicate = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Press, 7);
        let full = spawn_item(
            &mut registry,
            "plasma",
            Vec3::new(5.0, 0.0, 0.0),
            TouchMode::Press,
            7,
        );
        registry
            .set_component(
                duplicate_owner,
                Inventory {
                    wieldables: [
                        Some(owned),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ],
                    ..Inventory::default()
                },
            )
            .unwrap();
        let full_slots = [Some(owned); WIELDABLE_SLOT_CAPACITY];
        registry
            .set_component(
                full_owner,
                Inventory {
                    wieldables: full_slots,
                    ..Inventory::default()
                },
            )
            .unwrap();
        let mut system = TouchSystem::default();

        tick(
            &mut system,
            &mut registry,
            &players(&[
                (PlayerId::Local(duplicate_owner), duplicate_owner),
                (PlayerId::Remote(9), full_owner),
            ]),
            &[],
        );

        assert!(system.prompts.is_empty());
        assert!(
            registry
                .has_component_kind(duplicate, ComponentKind::Touchable)
                .unwrap()
        );
        assert!(
            registry
                .has_component_kind(full, ComponentKind::Touchable)
                .unwrap()
        );
    }

    fn observed_facts() -> &'static Mutex<Vec<TouchFacts>> {
        static FACTS: OnceLock<Mutex<Vec<TouchFacts>>> = OnceLock::new();
        FACTS.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn facts_test_guard() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    fn record_and_acquire(facts: &TouchFacts) -> Vec<TouchEffect> {
        observed_facts().lock().unwrap().push(*facts);
        vec![TouchEffect::Acquire]
    }

    fn record_and_apply_default(facts: &TouchFacts) -> Vec<TouchEffect> {
        observed_facts().lock().unwrap().push(*facts);
        default_touch_policy(facts)
    }

    #[test]
    fn policy_seam_receives_world_facts_and_can_acquire_duplicate() {
        let _guard = facts_test_guard().lock().unwrap();
        observed_facts().lock().unwrap().clear();

        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let owned = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 0.0, 0.0),
            TouchMode::Auto,
            3,
        );
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(owned);
        registry.set_component(pawn, inventory).unwrap();
        let mut reserve = AmmoReserve::default();
        reserve.credit("cells", 11);
        registry.set_component(pawn, reserve).unwrap();
        let mut system = TouchSystem {
            policy: record_and_acquire,
            ..TouchSystem::default()
        };

        tick(
            &mut system,
            &mut registry,
            &players(&[(PlayerId::Local(pawn), pawn)]),
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[1],
            Some(item)
        );
        assert_eq!(
            observed_facts().lock().unwrap().as_slice(),
            &[TouchFacts {
                owned_count: 1,
                free_slots: 9,
                magazine: 7,
                reserve: 11,
                pressed: true,
            }]
        );
    }

    #[test]
    fn auto_items_resolve_in_entity_id_order_and_later_items_see_the_filled_slot() {
        let _guard = facts_test_guard().lock().unwrap();
        observed_facts().lock().unwrap().clear();

        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let filler = spawn_item(
            &mut registry,
            "filler",
            Vec3::new(10.0, 0.0, 0.0),
            TouchMode::Auto,
            1,
        );
        let lower_id_item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let higher_id_item = spawn_item(&mut registry, "plasma", Vec3::ZERO, TouchMode::Auto, 4);
        let mut inventory = Inventory::default();
        for slot in &mut inventory.wieldables[..WIELDABLE_SLOT_CAPACITY - 1] {
            *slot = Some(filler);
        }
        registry.set_component(pawn, inventory).unwrap();
        let mut system = TouchSystem {
            policy: record_and_apply_default,
            ..TouchSystem::default()
        };

        tick(
            &mut system,
            &mut registry,
            &players(&[(PlayerId::Local(pawn), pawn)]),
            &[],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[WIELDABLE_SLOT_CAPACITY - 1],
            Some(lower_id_item),
            "the lower EntityId consumes the final free slot"
        );
        assert!(
            registry
                .has_component_kind(higher_id_item, ComponentKind::Touchable)
                .unwrap(),
            "the later item remains a world item after its policy declines the full inventory"
        );
        assert_eq!(
            observed_facts()
                .lock()
                .unwrap()
                .iter()
                .map(|facts| facts.free_slots)
                .collect::<Vec<_>>(),
            vec![1, 0],
            "the second policy evaluation observes the mutation from the lower EntityId item"
        );
    }

    #[test]
    fn press_prompts_until_claim_and_only_claimed_item_is_acquired() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let near = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(0.1, 0.0, 0.0),
            TouchMode::Press,
            7,
        );
        let far = spawn_item(
            &mut registry,
            "plasma",
            Vec3::new(0.8, 0.0, 0.0),
            TouchMode::Press,
            7,
        );
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick(&mut system, &mut registry, &players, &[]);
        assert_eq!(
            system.prompts,
            vec![(PlayerId::Local(pawn), near), (PlayerId::Local(pawn), far)]
        );

        tick(
            &mut system,
            &mut registry,
            &players,
            &[(PlayerId::Local(pawn), true)],
        );
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(near)
        );
        assert_eq!(system.prompts, vec![(PlayerId::Local(pawn), far)]);
    }

    #[test]
    fn press_claim_tie_prefers_the_lower_item_entity_id() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let lower_id_item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(-0.25, 0.0, 0.0),
            TouchMode::Press,
            7,
        );
        let higher_id_item = spawn_item(
            &mut registry,
            "plasma",
            Vec3::new(0.25, 0.0, 0.0),
            TouchMode::Press,
            7,
        );
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick(
            &mut system,
            &mut registry,
            &players,
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(lower_id_item),
            "one press claims only the lower EntityId of equal-distance items"
        );
        assert!(
            registry
                .has_component_kind(higher_id_item, ComponentKind::Touchable)
                .unwrap()
        );
    }

    #[test]
    fn zero_tick_frame_latches_press_and_preserves_prompt_until_the_next_touch_tick() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Press, 7);
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick(&mut system, &mut registry, &players, &[]);
        assert_eq!(system.prompts, vec![(PlayerId::Local(pawn), item)]);

        let mut latch = crate::input::GameplayInputLatch::new();
        let use_press = crate::input::ActionSnapshot::with_button_state(
            crate::input::Action::Use,
            crate::input::ButtonState::Pressed,
        );
        assert!(latch.snapshot_for_ticks(&use_press, 0).is_none());
        assert_eq!(
            system.prompts,
            vec![(PlayerId::Local(pawn), item)],
            "the zero-tick render frame leaves the published prompt intact"
        );

        let latched = latch
            .snapshot_for_ticks(&crate::input::ActionSnapshot::neutral(), 2)
            .expect("the later two-tick frame receives the pending press");
        assert_eq!(
            latched.button(crate::input::Action::Use),
            crate::input::ButtonState::Pressed
        );
        tick(
            &mut system,
            &mut registry,
            &players,
            &[(PlayerId::Local(pawn), true)],
        );
        tick(&mut system, &mut registry, &players, &[]);

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the pending Use edge claims the prompt on the first tick of the later frame"
        );
    }

    #[test]
    fn auto_enter_applies_once_across_two_fixed_ticks_in_one_frame() {
        let _guard = facts_test_guard().lock().unwrap();
        observed_facts().lock().unwrap().clear();

        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem {
            policy: record_and_acquire,
            ..TouchSystem::default()
        };

        tick(&mut system, &mut registry, &players, &[]);
        tick(&mut system, &mut registry, &players, &[]);

        assert_eq!(
            observed_facts().lock().unwrap().len(),
            1,
            "the second fixed tick sees sustained overlap, not a fresh auto touch"
        );
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
    }

    fn record_no_effect(facts: &TouchFacts) -> Vec<TouchEffect> {
        observed_facts().lock().unwrap().push(*facts);
        Vec::new()
    }

    #[test]
    fn no_effect_policy_evaluates_once_per_unbroken_overlap_without_refire() {
        let _guard = facts_test_guard().lock().unwrap();
        observed_facts().lock().unwrap().clear();

        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem {
            policy: record_no_effect,
            ..TouchSystem::default()
        };

        tick(&mut system, &mut registry, &players, &[]);
        tick(&mut system, &mut registry, &players, &[]);

        assert_eq!(observed_facts().lock().unwrap().len(), 2);
        assert!(
            registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap()
        );
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn acquisition_purges_queued_despawn_but_the_held_weapon_admits_later_deferred_effects() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        crate::impact_effects::despawn(&mut registry, item, Some(0.0));
        assert_eq!(
            registry
                .get_component::<DeferredEffectComponent>(item)
                .unwrap()
                .pending
                .len(),
            1,
            "the script-side deferred despawn is queued before pickup"
        );
        let mut system = TouchSystem::default();

        tick(
            &mut system,
            &mut registry,
            &players(&[(PlayerId::Local(pawn), pawn)]),
            &[],
        );

        assert!(
            registry.exists(item),
            "pickup preserves the weapon instance"
        );
        assert!(registry.get_component::<WeaponComponent>(item).is_ok());
        let effects = registry
            .get_component::<DeferredEffectComponent>(item)
            .unwrap();
        assert!(
            effects.pending.is_empty(),
            "pickup purges the queued despawn"
        );
        assert!(
            !effects.inert,
            "pickup is not a terminal lifecycle transition"
        );

        crate::impact_effects::despawn(&mut registry, item, Some(0.0));
        crate::impact_effects::tick_deferred_effects(&mut registry, 1.0 / 60.0);
        assert!(
            registry.is_marked_for_end_of_frame_removal(item).unwrap(),
            "a later deferred despawn is admitted and runs against the held weapon"
        );
    }

    #[test]
    fn marked_item_is_skipped_and_prunes_occupancy() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Press, 7);
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick(&mut system, &mut registry, &players, &[]);
        assert!(
            system
                .occupants
                .get(&item)
                .unwrap()
                .contains(&PlayerId::Local(pawn))
        );

        registry.mark_for_end_of_frame_removal(item).unwrap();
        tick(&mut system, &mut registry, &players, &[]);

        assert!(!system.occupants.contains_key(&item));
        assert!(system.prompts.is_empty());
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn drop_resets_live_weapon_state_preserves_reserve_and_seeds_auto_occupancy() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let standing_player = spawn_player(&mut registry, Vec3::new(0.0, 2.0, -0.4));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            3,
        );
        held_item(&mut registry, pawn, item);
        let mut reserve = AmmoReserve::default();
        reserve.credit("cells", 11);
        registry.set_component(pawn, reserve).unwrap();
        let mut weapon = registry
            .get_component::<WeaponComponent>(item)
            .unwrap()
            .clone();
        weapon.state = WieldableState::Reloading;
        weapon.state_remaining_ms = 800;
        weapon.state_total_ms = 1000;
        weapon.state_elapsed_sub_ms = 0.5;
        weapon.reload_credited = 2;
        weapon.cooldown_remaining_ms = 90.0;
        weapon.shoot_press_consumed = true;
        weapon.reload_press_consumed = true;
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
        registry.set_component(item, weapon).unwrap();
        let deferred = registry.deferred_effect_mut(item).unwrap();
        deferred.pending.push(postretro_entities::PendingEffect {
            kind: postretro_entities::DeferredEffectKind::SetHealth,
            remaining_us: 100,
            value: Some(1.0),
        });
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 1.0)];
        let players = players(&[
            (PlayerId::Local(pawn), pawn),
            (PlayerId::Remote(7), standing_player),
        ]);
        let mut system = TouchSystem::default();

        let events = tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(events.repointed_pawns, vec![pawn]);
        assert_eq!(events.dropped_item_meshes, vec![item]);
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "release_wieldable repairs the inventory immediately"
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("cells"),
            11,
            "dropping must not alter the pawn's ammo-reserve balance"
        );
        assert!(
            registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap()
        );
        assert_eq!(
            registry.get_component::<MeshComponent>(item).unwrap().model,
            "dropped-item.glb"
        );
        assert!(
            registry
                .get_component::<Transform>(item)
                .unwrap()
                .position
                .distance(Vec3::new(0.0, 1.0 + SKIN_DISTANCE, -1.0))
                < 1.0e-5,
            "drop rests on the walkable floor one metre ahead of the player"
        );
        let weapon = registry.get_component::<WeaponComponent>(item).unwrap();
        assert_eq!(weapon.magazine, 3, "drop preserves the live magazine");
        assert_eq!(weapon.state, WieldableState::Idle);
        assert_eq!(weapon.state_remaining_ms, 0);
        assert_eq!(weapon.state_total_ms, 0);
        assert!(
            weapon.state_elapsed_sub_ms.abs() < 1.0e-6,
            "drop resets fractional weapon-state progress"
        );
        assert_eq!(weapon.reload_credited, 0);
        assert!(
            weapon.cooldown_remaining_ms.abs() < 1.0e-6,
            "drop resets weapon cooldown"
        );
        assert!(!weapon.shoot_press_consumed);
        assert!(!weapon.reload_press_consumed);
        assert_eq!(weapon.reload_feedback, Default::default());
        assert_eq!(
            registry
                .get_component::<DeferredEffectComponent>(item)
                .unwrap()
                .pending
                .len(),
            1,
            "drop does not purge deferred effects; only acquisition does"
        );
        assert!(
            system.occupants.get(&item).is_some_and(|occupants| {
                occupants.contains(&PlayerId::Local(pawn))
                    && occupants.contains(&PlayerId::Remote(7))
            }),
            "every player already overlapping the drop point is seeded"
        );

        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[],
        );
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "a seeded auto item is not immediately re-acquired while the player stands still"
        );
        assert!(
            registry
                .get_component::<Inventory>(standing_player)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "another player already at the drop point receives no fresh auto enter edge"
        );
    }

    // Regression: restoring a dropped mesh exposed stale held-item transform history.
    #[test]
    fn drop_snaps_restored_mesh_transform_history_to_resolved_position() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        registry.snapshot_transforms();
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 0.1)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        let events = tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(events.dropped_item_meshes, vec![item]);
        let resolved = registry.get_component::<Transform>(item).unwrap().position;
        for alpha in [0.0, 0.5, 1.0] {
            let presented = registry
                .interpolated_transform(item, alpha)
                .expect("the restored mesh retains transform history");
            assert!(
                presented.position.distance(resolved) < 1.0e-5,
                "drop position must be alpha-invariant on its first visible frame"
            );
        }
    }

    #[test]
    fn auto_drop_reacquires_only_after_the_dropper_leaves_and_reenters() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 1.0)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );
        registry
            .set_component(
                pawn,
                Transform {
                    position: Vec3::new(10.0, 2.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[],
        );
        registry
            .set_component(
                pawn,
                Transform {
                    position: Vec3::new(0.0, 2.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "leaving clears the drop inhibit; the later enter edge acquires the item"
        );
    }

    #[test]
    fn adjacent_simultaneous_drops_seed_both_players_and_do_not_swap_weapons() {
        let mut registry = EntityRegistry::new();
        let first = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let second = spawn_player(&mut registry, Vec3::new(0.1, 2.0, 0.0));
        let first_item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            7,
        );
        let second_item = spawn_item(
            &mut registry,
            "plasma",
            Vec3::new(11.0, 2.0, 0.0),
            TouchMode::Auto,
            4,
        );
        held_item(&mut registry, first, first_item);
        held_item(&mut registry, second, second_item);
        let descriptors = [
            drop_descriptor("ion", TouchMode::Auto, 1.0),
            drop_descriptor("plasma", TouchMode::Auto, 1.0),
        ];
        let players = players(&[
            (PlayerId::Local(first), first),
            (PlayerId::Remote(7), second),
        ]);
        let mut system = TouchSystem::default();

        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(first), true), (PlayerId::Remote(7), true)],
        );
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[],
        );

        for pawn in [first, second] {
            assert!(
                registry
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .wieldables
                    .iter()
                    .all(Option::is_none),
                "occupancy seeding prevents an adjacent dropper from taking either item"
            );
        }
        for item in [first_item, second_item] {
            assert!(
                system.occupants.get(&item).is_some_and(|occupants| {
                    occupants.contains(&PlayerId::Local(first))
                        && occupants.contains(&PlayerId::Remote(7))
                }),
                "each dropped item is seeded with every capsule already at its final point"
            );
        }
    }

    #[test]
    fn zero_hp_player_remains_a_touch_occupant_and_can_drop() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        registry
            .set_component(
                pawn,
                HealthComponent {
                    max: 100.0,
                    current: 0.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        let death = crate::scripting_systems::health::sweep_deaths(&mut registry);
        assert!(
            death.player_died,
            "the player death latches before the touch pass"
        );
        assert!(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .is_ok(),
            "the death sweep retains the pawn capsule as the toucher"
        );

        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 1.0)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[],
        );

        assert!(
            system
                .occupants
                .get(&item)
                .is_some_and(|occupants| occupants.contains(&PlayerId::Local(pawn))),
            "the corpse is seeded into the dropped item's occupancy"
        );
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "a still corpse produces no new auto enter edge"
        );
    }

    #[test]
    fn drop_seed_suppresses_auto_but_allows_a_deliberate_press_reacquire() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Press,
            7,
        );
        held_item(&mut registry, pawn, item);
        let descriptors = [drop_descriptor("ion", TouchMode::Press, 1.0)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "standing still after the drop does not create a fresh auto acquisition"
        );
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[(PlayerId::Local(pawn), true)],
            &[],
        );

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "a deliberate press on a later tick defeats only the automatic re-acquisition inhibit"
        );
        assert!(
            !registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap(),
            "deliberate re-acquisition returns the item to held state"
        );
    }

    #[test]
    fn pickup_and_drop_preserve_every_reserve_balance_including_explicit_zeroes() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let mut reserve = AmmoReserve::default();
        reserve.credit("cells", 11);
        reserve.set_exact("rockets", 0);
        registry.set_component(pawn, reserve).unwrap();
        let reserve_before = registry
            .get_component::<AmmoReserve>(pawn)
            .unwrap()
            .balances()
            .map(|(ammo_type, amount)| (ammo_type.to_owned(), amount))
            .collect::<Vec<_>>();
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 1.0)];
        let mut system = TouchSystem::default();

        tick(&mut system, &mut registry, &players, &[]);
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the reserve assertion covers a genuine pickup before the drop"
        );
        tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .balances()
                .map(|(ammo_type, amount)| (ammo_type.to_owned(), amount))
                .collect::<Vec<_>>(),
            reserve_before,
            "acquiring then dropping moves only the wieldable instance, never pawn-owned reserves"
        );
    }

    #[test]
    fn acquired_wieldable_carries_its_provenance_and_magazine_across_level_unload() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::ZERO);
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 5);
        let mut touch_system = TouchSystem::default();

        tick(
            &mut touch_system,
            &mut registry,
            &players(&[(PlayerId::Local(pawn), pawn)]),
            &[],
        );
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the later carry harvest reads the instance acquired by the touch pass"
        );
        let mut seats = SeatTable::from_test_session_id([0x16; 16]);
        seats.bind_pawn(&mut registry, Seat(0), pawn);
        seats.harvest_pawn(&registry, pawn);
        let carried = seats
            .carried_state(Seat(0))
            .expect("the level-transition ledger harvests the acquired inventory")
            .clone();

        assert_eq!(carried.wieldables[0].as_deref(), Some("ion"));
        assert_eq!(carried.magazines[0], Some(5));
        registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut registry);

        let mut next_level = EntityRegistry::new();
        let next_pawn = next_level.spawn(Transform::default());
        let placement = MapEntity {
            classname: "player_spawn".to_string(),
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            key_values: Default::default(),
            tags: Vec::new(),
        };
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 1.0)];
        compose_wieldable_inventory_from_slots(
            &mut next_level,
            next_pawn,
            &placement,
            &descriptors,
            &carried.wieldables,
            Some(&carried),
        );

        let restored = next_level
            .get_component::<Inventory>(next_pawn)
            .unwrap()
            .wieldables[0]
            .expect("the carried slot materializes on the next level");
        assert_eq!(
            next_level
                .get_component::<DescriptorProvenance>(restored)
                .unwrap()
                .canonical_name,
            "ion"
        );
        assert_eq!(
            next_level
                .get_component::<WeaponComponent>(restored)
                .unwrap()
                .magazine,
            5,
            "the carried instance's live magazine survives the descriptor respawn"
        );
    }

    #[test]
    fn drop_refuses_non_touchable_descriptor_without_releasing_and_warns_once() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let item = spawn_item(
            &mut registry,
            "sealed",
            Vec3::new(10.0, 2.0, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        let mut descriptor = drop_descriptor("sealed", TouchMode::Auto, 1.0);
        descriptor.touchable = None;
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();
        let capture = LogCapture::start();

        for _ in 0..2 {
            tick_with_edges(
                &mut system,
                &mut registry,
                &floor_world(),
                &[descriptor.clone()],
                &players,
                &[],
                &[(PlayerId::Local(pawn), true)],
            );
        }

        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
        capture.assert_logged_once(Level::Warn, "descriptor has no touchable block");
    }

    #[test]
    fn drop_with_empty_inventory_is_a_silent_noop() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 2.0, 0.0));
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        let events = tick_with_edges(
            &mut system,
            &mut registry,
            &floor_world(),
            &[],
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert!(events.repointed_pawns.is_empty());
        assert!(events.dropped_item_meshes.is_empty());
        assert_eq!(
            registry.get_component::<Inventory>(pawn).unwrap(),
            &Inventory::default()
        );
    }

    #[test]
    fn drop_lands_on_walkable_floor_one_metre_ahead() {
        let pawn_transform = Transform {
            position: Vec3::new(0.0, 1.2, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };

        let position = resolve_drop_position(
            &floor_world(),
            &pawn_transform,
            pawn_transform.position,
            0.4,
            0.8,
            0.1,
        )
        .expect("a floor directly ahead receives the dropped item");

        assert!(position.distance(Vec3::new(0.0, 0.1 + SKIN_DISTANCE, -1.0)) < 1.0e-5);
    }

    // Regression: vertical lift left dropped spheres intersecting inclined floors.
    #[test]
    fn drop_on_inclined_walkable_floor_offsets_along_normal_with_clearance() {
        let slope = 0.3_f32;
        let surface_y = |x: f32| slope * x;
        let world = CollisionWorld {
            mesh: TriMesh::new(
                vec![
                    Point::new(-100.0, surface_y(-100.0), -100.0),
                    Point::new(100.0, surface_y(100.0), -100.0),
                    Point::new(100.0, surface_y(100.0), 100.0),
                    Point::new(-100.0, surface_y(-100.0), 100.0),
                ],
                vec![[0, 2, 1], [0, 3, 2]],
            ),
            isometry: Isometry::identity(),
        };
        let pawn_transform = Transform {
            position: Vec3::new(0.0, 1.2, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let item_radius = 0.5;

        let position = resolve_drop_position(
            &world,
            &pawn_transform,
            pawn_transform.position,
            0.4,
            0.8,
            item_radius,
        )
        .expect("a walkable inclined floor receives the dropped item");

        let expected_normal = Vec3::new(-slope, 1.0, 0.0).normalize();
        let expected = Vec3::new(0.0, 0.0, -1.0) + expected_normal * (item_radius + SKIN_DISTANCE);
        assert!(position.distance(expected) < 1.0e-5);
        assert!(sphere_fits_world(&world, position, item_radius));
    }

    #[test]
    fn drop_uses_three_quarter_metre_fallback_when_one_metre_position_is_blocked() {
        let pawn_transform = Transform {
            position: Vec3::new(0.0, 1.2, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let world = CollisionWorld {
            mesh: TriMesh::new(
                vec![
                    Point::new(-100.0, 0.0, -100.0),
                    Point::new(100.0, 0.0, -100.0),
                    Point::new(100.0, 0.0, 100.0),
                    Point::new(-100.0, 0.0, 100.0),
                    Point::new(-1.0, 0.0, -1.05),
                    Point::new(1.0, 0.0, -1.05),
                    Point::new(1.0, 2.0, -1.05),
                    Point::new(-1.0, 2.0, -1.05),
                ],
                vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            ),
            isometry: Isometry::identity(),
        };

        let position = resolve_drop_position(
            &world,
            &pawn_transform,
            pawn_transform.position,
            0.4,
            0.8,
            0.1,
        )
        .expect("the second forward candidate remains clear");

        assert!(position.distance(Vec3::new(0.0, 0.1 + SKIN_DISTANCE, -0.75)) < 1.0e-5);
    }

    #[test]
    fn drop_ignores_wall_behind_pawn_when_forward_floor_is_clear() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 1.2, 0.0));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 1.2, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        let world = CollisionWorld {
            mesh: TriMesh::new(
                vec![
                    Point::new(-100.0, 0.0, -100.0),
                    Point::new(100.0, 0.0, -100.0),
                    Point::new(100.0, 0.0, 100.0),
                    Point::new(-100.0, 0.0, 100.0),
                    Point::new(-2.0, 0.0, 0.25),
                    Point::new(2.0, 0.0, 0.25),
                    Point::new(0.0, 3.0, 0.25),
                ],
                vec![[0, 1, 2], [0, 2, 3], [4, 5, 6]],
            ),
            isometry: Isometry::identity(),
        };
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 0.1)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        tick_with_edges(
            &mut system,
            &mut registry,
            &world,
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        let position = registry.get_component::<Transform>(item).unwrap().position;
        assert!(
            position.distance(Vec3::new(0.0, 0.1 + SKIN_DISTANCE, -1.0)) < 1.0e-5,
            "a wall behind the pawn cannot reject the valid forward floor placement"
        );
        assert!(sphere_fits_world(&world, position, 0.1));
        assert!(
            system
                .occupants
                .get(&item)
                .is_some_and(|occupants| !occupants.contains(&PlayerId::Local(pawn))),
            "the dropper is not seeded when the final forward pickup sphere does not overlap it"
        );
        tick_with_edges(
            &mut system,
            &mut registry,
            &world,
            &descriptors,
            &players,
            &[],
            &[],
        );
        assert!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none),
            "the item remains in the world because the player is outside its final pickup sphere"
        );
    }

    #[test]
    fn drop_without_a_forward_walkable_floor_keeps_inventory_unchanged() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 1.2, 0.0));
        let item = spawn_item(
            &mut registry,
            "ion",
            Vec3::new(10.0, 1.2, 0.0),
            TouchMode::Auto,
            7,
        );
        held_item(&mut registry, pawn, item);
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 0.1)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        let events = tick_with_edges(
            &mut system,
            &mut registry,
            &CollisionWorld::default(),
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert_eq!(events.repointed_pawns, Vec::<EntityId>::new());
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "without a valid forward floor, release must not run"
        );
        assert!(
            !registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap(),
            "the held item stays out of world touch evaluation"
        );
    }

    #[test]
    fn drop_rejects_non_walkable_forward_floor_and_keeps_inventory_held() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_player(&mut registry, Vec3::new(0.0, 1.2, 0.0));
        let item_position = Vec3::new(10.0, 1.2, 0.0);
        let item = spawn_item(&mut registry, "ion", item_position, TouchMode::Auto, 7);
        held_item(&mut registry, pawn, item);
        let slope = 2.0_f32;
        let surface_y = |x: f32| slope * x;
        let world = CollisionWorld {
            mesh: TriMesh::new(
                vec![
                    Point::new(-100.0, surface_y(-100.0), -100.0),
                    Point::new(100.0, surface_y(100.0), -100.0),
                    Point::new(100.0, surface_y(100.0), 100.0),
                    Point::new(-100.0, surface_y(-100.0), 100.0),
                ],
                vec![[0, 2, 1], [0, 3, 2]],
            ),
            isometry: Isometry::identity(),
        };
        let descriptors = [drop_descriptor("ion", TouchMode::Auto, 0.1)];
        let players = players(&[(PlayerId::Local(pawn), pawn)]);
        let mut system = TouchSystem::default();

        let events = tick_with_edges(
            &mut system,
            &mut registry,
            &world,
            &descriptors,
            &players,
            &[],
            &[(PlayerId::Local(pawn), true)],
        );

        assert!(events.dropped_item_meshes.is_empty());
        assert_eq!(
            registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "a non-walkable forward floor must not release the held item"
        );
        assert!(
            registry
                .get_component::<Transform>(item)
                .unwrap()
                .position
                .distance(item_position)
                < 1.0e-5,
            "a rejected floor leaves the held item's transform unchanged"
        );
        assert!(
            !registry
                .has_component_kind(item, ComponentKind::Touchable)
                .unwrap(),
            "the held item stays out of world touch evaluation"
        );
    }

    #[test]
    fn sphere_capsule_helpers_match_vertical_axis_geometry() {
        assert!(sphere_overlaps_capsule(
            Vec3::new(0.0, 1.5, 0.0),
            0.5,
            Vec3::ZERO,
            0.5,
            1.0,
        ));
        assert!(!sphere_overlaps_capsule(
            Vec3::new(0.0, 2.1, 0.0),
            0.5,
            Vec3::ZERO,
            0.5,
            1.0,
        ));
        assert!(
            (sphere_capsule_distance_squared(Vec3::new(3.0, 0.0, 4.0), Vec3::ZERO, 1.0) - 25.0)
                .abs()
                < 1.0e-6
        );
    }
}
