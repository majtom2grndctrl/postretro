//! Host-authoritative touch evaluation for world wieldables.
//! See: context/lib/entity_model.md §7.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::Vec3;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::touchable::TouchableComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    AmmoReserve, ComponentKind, DeferredEffectComponent, DescriptorProvenance, EntityId,
    EntityRegistry, EntityTypeDescriptor, TouchMode, Transform,
};

use crate::collision::CollisionWorld;
use crate::scripting::builtins::wieldable_inventory::acquire_wieldable;
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

/// Per-level state for deterministic item touch, touch edges, and prompts.
///
/// Sorted occupancy keys make edge emission stable across equivalent input
/// orderings. The policy remains pure; the pass mutates the registry only after
/// reducing one item's contestants to at most one winner.
#[derive(Debug)]
pub(crate) struct TouchSystem {
    occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>,
    warned_duplicate_players: HashSet<PlayerId>,
    policy: fn(&TouchFacts) -> Vec<TouchEffect>,
    pub(crate) prompts: Vec<(PlayerId, EntityId)>,
}

impl Default for TouchSystem {
    fn default() -> Self {
        Self {
            occupants: BTreeMap::new(),
            warned_duplicate_players: HashSet::new(),
            policy: default_touch_policy,
            prompts: Vec::new(),
        }
    }
}

impl TouchSystem {
    pub(crate) fn clear(&mut self) {
        self.occupants.clear();
        self.warned_duplicate_players.clear();
        self.prompts.clear();
    }

    /// Runs after trigger dispatch and before AI on every host/single-player
    /// fixed tick. `collision_world` and `descriptors` are pass-owned now so
    /// the drop half can use the same authoritative ordering without widening
    /// this seam later.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_authoritative_tick(
        &mut self,
        registry: &mut EntityRegistry,
        _collision_world: &CollisionWorld,
        _descriptors: &[EntityTypeDescriptor],
        players: &[AuthoritativePlayer],
        use_pressed: &HashMap<PlayerId, bool>,
        _drop_pressed: &HashMap<PlayerId, bool>,
    ) -> Vec<EntityId> {
        self.prompts.clear();

        let player_capsules =
            canonical_player_capsules(registry, players, &mut self.warned_duplicate_players);
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
            let effects_applied = winner.map_or(false, |winner| {
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

        repointed.into_iter().collect()
    }
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
    use postretro_entities::ComponentKind;
    use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    use postretro_entities::provenance::{
        DescriptorComponentKind, DescriptorMapOverride, DescriptorSpawnPath,
    };
    use postretro_foundation::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        PlayerMovementComponent, PlayerMovementDescriptor, ReloadStyle, ResolutionMode,
        SpeedParams, WeaponDescriptor, WeaponResource,
    };

    use super::*;

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
            view_feel: None,
        })
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
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some(canonical_name.to_string()),
            third_person_model: None,
            viewmodel: None,
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
        system.run_authoritative_tick(
            registry,
            &CollisionWorld::default(),
            &[],
            players,
            &pressed,
            &HashMap::new(),
        )
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
    fn simultaneous_auto_contest_has_one_stable_winner() {
        let mut registry = EntityRegistry::new();
        let first = spawn_player(&mut registry, Vec3::new(-0.25, 0.0, 0.0));
        let second = spawn_player(&mut registry, Vec3::new(0.25, 0.0, 0.0));
        let item = spawn_item(&mut registry, "ion", Vec3::ZERO, TouchMode::Auto, 7);
        let mut system = TouchSystem::default();
        let players = players(&[
            (PlayerId::Remote(7), second),
            (PlayerId::Local(first), first),
        ]);

        tick(&mut system, &mut registry, &players, &[]);

        assert_eq!(
            registry
                .get_component::<Inventory>(first)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
        assert!(
            registry
                .get_component::<Inventory>(second)
                .unwrap()
                .wieldables
                .iter()
                .all(Option::is_none)
        );
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
