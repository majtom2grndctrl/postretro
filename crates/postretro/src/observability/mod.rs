// Headless observability vocabulary: runspec input, output dumps, and
// deterministic JSON serialization for byte-identical runs. Driver
// (`driver::run_headless`) is wired from `startup::build_session`.
// See: context/plans/done/agentic-observability

mod document;
mod driver;
mod runspec;

pub(crate) use driver::run_headless;

pub(crate) use document::{PawnHealth, PlayerPawnSummary, TickEventRecord, build_output_document};
pub(crate) use runspec::{AimCommand, CommandEntry, parse_runspec};

use postretro_entities::ComponentKind;
use serde::Serialize;
use thiserror::Error;

/// Failure applying a [`runspec::DumpSpec`] against a registry. The only currently
/// possible failure is an unrecognized component-kind filter string; it is a
/// bad *value* (not a malformed document), so it surfaces here at dump time
/// rather than at runspec-parse time. The headless driver exits non-zero on it.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum DumpError {
    #[error("unknown component-kind filter {0:?}")]
    UnknownComponentKind(String),
}

/// Every component kind, in `ComponentKind` discriminant order. The array length
/// is pinned to [`ComponentKind::COUNT`], so adding a variant without extending
/// this list is a compile error (length mismatch) — the dump's kind iteration
/// can never silently skip a new component.
const ALL_KINDS: [ComponentKind; ComponentKind::COUNT] = [
    ComponentKind::Transform,
    ComponentKind::Light,
    ComponentKind::BillboardEmitter,
    ComponentKind::ParticleState,
    ComponentKind::SpriteVisual,
    ComponentKind::FogVolume,
    ComponentKind::PlayerMovement,
    ComponentKind::Weapon,
    ComponentKind::DescriptorProvenance,
    ComponentKind::Mesh,
    ComponentKind::Health,
    ComponentKind::Agent,
    ComponentKind::Brain,
    ComponentKind::KinematicMover,
    ComponentKind::TriggerVolume,
    ComponentKind::AmmoReserve,
    ComponentKind::Spawner,
    ComponentKind::EntityState,
    ComponentKind::DeferredEffect,
    ComponentKind::Inventory,
    ComponentKind::Touchable,
    ComponentKind::Projectile,
];

/// Snake_case name for a component kind, matching `ComponentValue`'s serde
/// envelope `"kind"` tag exactly. `ComponentKind`'s own derive is PascalCase, so
/// this module owns the snake_case mapping rather than touching that derive.
///
/// Exhaustive `match` with no `_` arm on purpose: a new component kind is a
/// compile error here, forcing the author to give it a stable filter string
/// rather than have the dump filter silently miss it.
fn component_kind_snake(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Transform => "transform",
        ComponentKind::Light => "light",
        ComponentKind::BillboardEmitter => "billboard_emitter",
        ComponentKind::ParticleState => "particle_state",
        ComponentKind::SpriteVisual => "sprite_visual",
        ComponentKind::FogVolume => "fog_volume",
        ComponentKind::PlayerMovement => "player_movement",
        ComponentKind::Weapon => "weapon",
        ComponentKind::DescriptorProvenance => "descriptor_provenance",
        ComponentKind::Mesh => "mesh",
        ComponentKind::Health => "health",
        ComponentKind::Agent => "agent",
        ComponentKind::Brain => "brain",
        ComponentKind::KinematicMover => "kinematic_mover",
        ComponentKind::TriggerVolume => "trigger_volume",
        ComponentKind::AmmoReserve => "ammo_reserve",
        ComponentKind::Spawner => "spawner",
        ComponentKind::EntityState => "entity_state",
        ComponentKind::DeferredEffect => "deferred_effect",
        ComponentKind::Inventory => "inventory",
        ComponentKind::Touchable => "touchable",
        ComponentKind::Projectile => "projectile",
    }
}

/// Resolve a snake_case component-kind filter string to its [`ComponentKind`].
/// `None` when the string names no known kind (the caller maps that to a
/// [`DumpError::UnknownComponentKind`]).
fn parse_component_kind(name: &str) -> Option<ComponentKind> {
    ALL_KINDS
        .into_iter()
        .find(|kind| component_kind_snake(*kind) == name)
}

/// Serialize any value to pretty JSON with every map (object) key in sorted
/// order, recursively. This is the determinism guarantee for the dump: several
/// `ComponentValue` payloads carry std `HashMap` fields (health zone
/// multipliers, mesh animation states) whose serde iteration order is randomized
/// per process, so a direct `serde_json::to_string` would differ byte-for-byte
/// across runs. Going through a `serde_json::Value` and sorting object keys makes
/// the output stable regardless of the hasher seed or the serde_json
/// `preserve_order` feature. Array order is data-bearing and is left untouched.
pub(crate) fn to_deterministic_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_value(value)?;
    sort_json_maps(&mut json);
    serde_json::to_string_pretty(&json)
}

/// Recursively reorder every JSON object's entries into ascending key order.
fn sort_json_maps(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> =
                std::mem::take(map).into_iter().collect();
            for (_, child) in entries.iter_mut() {
                sort_json_maps(child);
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, child) in entries {
                sorted.insert(key, child);
            }
            *map = sorted;
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                sort_json_maps(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_entities::components::particle::ParticleState;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::components::projectile::ProjectileComponent;
    use postretro_entities::components::sprite_visual::SpriteVisual;
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_entities::{
        ActionVerb, AirParams, AmmoReserve, AttackParams, BehaviorActivityDescriptor,
        BehaviorGraphDescriptor, BehaviorGraphEnvelope, CapsuleParams, ComponentValue,
        DescriptorProvenance, DescriptorSpawnPath, EntityId, FallParams, FireMode,
        FogVolumeComponent, GroundParams, KinematicMoverComponent, KinematicMoverMode, MotionVerb,
        MoverCommand, PlayerMovementDescriptor, ResolutionMode, SpeedParams, Transform,
        TriggerActivation, TriggerFireMode, TriggerVolumeComponent, WeaponDescriptor,
    };
    use std::collections::{BTreeSet, HashMap};

    /// Minimal valid movement descriptor for materializing a representative
    /// `PlayerMovementComponent` in [`sample_component_value`].
    fn sample_player_movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        }
    }

    /// One representative, validly-shaped `ComponentValue` for each
    /// `ComponentKind`, used to derive the real serde `"kind"` tag. Exhaustive
    /// `match` with no `_` arm on purpose: a new component kind is a compile
    /// error here, so the drift guard below can never silently skip it.
    fn sample_component_value(kind: ComponentKind) -> ComponentValue {
        match kind {
            ComponentKind::Transform => ComponentValue::Transform(Transform::default()),
            ComponentKind::Light => ComponentValue::Light(LightComponent {
                origin: [0.0, 0.0, 0.0],
                light_type: LightKind::Point,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: FalloffKind::Linear,
                falloff_range: 10.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: false,
                animated_slot: None,
                follow_transform: false,
                carrier: None,
                animation: None,
            }),
            ComponentKind::BillboardEmitter => {
                ComponentValue::BillboardEmitter(BillboardEmitterComponent {
                    rate: 1.0,
                    burst: None,
                    spread: 0.0,
                    lifetime: 1.0,
                    velocity: [0.0, 0.0, 0.0],
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_over_lifetime: [1.0].into(),
                    opacity_over_lifetime: [1.0].into(),
                    color: [1.0, 1.0, 1.0],
                    sprite: "sprite".into(),
                    spin_rate: 0.0,
                    spin_animation: None,
                })
            }
            ComponentKind::ParticleState => ComponentValue::ParticleState(ParticleState {
                velocity: [0.0, 0.0, 0.0],
                age: 0.0,
                lifetime: 1.0,
                buoyancy: 0.0,
                drag: 0.0,
                size_curve: [1.0].into(),
                opacity_curve: [1.0].into(),
                emitter: None,
            }),
            ComponentKind::SpriteVisual => ComponentValue::SpriteVisual(SpriteVisual {
                sprite: "sprite".into(),
                size: 1.0,
                opacity: 1.0,
                rotation: 0.0,
                tint: [1.0, 1.0, 1.0],
            }),
            ComponentKind::FogVolume => ComponentValue::FogVolume(FogVolumeComponent {
                density: 0.1,
                glow: 0.0,
                edge_softness: 0.0,
                falloff: 1.0,
                tint: [1.0, 1.0, 1.0],
                saturation: 1.0,
                min_brightness: 0.0,
                light_range: 1.0,
                animation: None,
            }),
            ComponentKind::PlayerMovement => ComponentValue::PlayerMovement(Box::new(
                PlayerMovementComponent::from_descriptor(&sample_player_movement_descriptor()),
            )),
            ComponentKind::Weapon => {
                ComponentValue::Weapon(WeaponComponent::from_descriptor(&WeaponDescriptor {
                    damage: 10.0,
                    pellet_count: 1,
                    spread_degrees: 0.0,
                    range: 20.0,
                    cooldown_ms: 100.0,
                    fire_mode: FireMode::Semi,
                    resolution: ResolutionMode::Hitscan,
                    projectile: None,
                    credit_source: None,
                    third_person_model: None,
                    viewmodel: None,
                    placement: None,
                    muzzle_offset: None,
                    resource: None,
                    lower_ms: 0,
                    raise_ms: 0,
                    block_during_reload: None,
                }))
            }
            ComponentKind::DescriptorProvenance => {
                ComponentValue::DescriptorProvenance(DescriptorProvenance {
                    canonical_name: "test_archetype".into(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                })
            }
            ComponentKind::Mesh => {
                ComponentValue::Mesh(Box::new(MeshComponent::stateless("test_model".into())))
            }
            ComponentKind::Health => ComponentValue::Health(HealthComponent {
                max: 1.0,
                current: 1.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: HashMap::new(),
                contributor_ledger: Default::default(),
            }),
            ComponentKind::Agent => ComponentValue::Agent(AgentComponent::new(0.3, 1.6, 0.35, 5.0)),
            ComponentKind::Brain => {
                ComponentValue::Brain(BrainComponent::from_graph(&BehaviorGraphDescriptor {
                    envelope: BehaviorGraphEnvelope {
                        initial: "idle".to_string(),
                        activities: std::collections::BTreeMap::from([(
                            "idle".to_string(),
                            BehaviorActivityDescriptor {
                                animation: Some("idle".to_string()),
                                motion: Some(MotionVerb::Hold),
                                action: Some(ActionVerb::Attack("attack".to_string())),
                                on_enter: None,
                                layers: Default::default(),
                            },
                        )]),
                        transitions: Default::default(),
                    },
                    candidate_filter: None,
                    patrol: None,
                    attacks: std::collections::BTreeMap::from([(
                        "attack".to_string(),
                        AttackParams {
                            damage: 5.0,
                            max_range: 2.0,
                            cooldown_ms: 500.0,
                            engagement_radius: None,
                            standoff_distance: None,
                        },
                    )]),
                    engagement_radius: None,
                    move_speed: 3.0,
                }))
            }
            ComponentKind::KinematicMover => {
                ComponentValue::KinematicMover(KinematicMoverComponent::new(
                    1,
                    postretro_entities::KinematicMoverConfig {
                        waypoints: vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)],
                        waypoint_names: vec!["a".to_string(), "b".to_string()],
                        speed_mps: 1.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: false,
                        spin_axis: Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ))
            }
            ComponentKind::TriggerVolume => {
                ComponentValue::TriggerVolume(TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    "sample".to_string(),
                    String::new(),
                    String::new(),
                    MoverCommand::Start,
                    TriggerFireMode::Once,
                    0.0,
                    true,
                ))
            }
            ComponentKind::AmmoReserve => ComponentValue::AmmoReserve(AmmoReserve::new()),
            ComponentKind::Spawner => {
                ComponentValue::Spawner(postretro_entities::components::spawner::SpawnerComponent {
                    archetype_name: String::new(),
                    count: 0,
                    resolved: false,
                })
            }
            ComponentKind::EntityState => {
                ComponentValue::EntityState(postretro_entities::EntityStateComponent::default())
            }
            ComponentKind::DeferredEffect => ComponentValue::DeferredEffect(
                postretro_entities::DeferredEffectComponent::default(),
            ),
            ComponentKind::Inventory => ComponentValue::Inventory(
                postretro_entities::components::inventory::Inventory::default(),
            ),
            ComponentKind::Touchable => ComponentValue::Touchable(
                postretro_entities::components::touchable::TouchableComponent {
                    mode: postretro_foundation::data_descriptors::TouchMode::Auto,
                    radius: 40.0,
                },
            ),
            ComponentKind::Projectile => ComponentValue::Projectile(ProjectileComponent {
                direction: [0.0, 0.0, -1.0],
                speed: 20.0,
                radius: 0.1,
                remaining_range: 64.0,
                remaining_lifetime: 1.0,
                damage: 10.0,
                credit_source: "test.projectile".to_string(),
                owner_pawn: EntityId::from_raw(1),
                owner_weapon: EntityId::from_raw(2),
                spawned: false,
                predicted_shot_id: None,
                elapsed_flight_age: 0.0,
                flipbook_active: false,
                impact_light: None,
            }),
        }
    }

    #[test]
    fn component_kind_snake_matches_component_value_serde_tag_for_every_kind() {
        // Drift guard: for every ComponentKind, the hand-written filter string
        // must equal the `kind` tag ComponentValue's own serde envelope emits.
        // `sample_component_value`'s exhaustive match means a new variant is a
        // compile error there, not a silently-passing test here.
        for kind in ALL_KINDS {
            let value = sample_component_value(kind);
            let json = serde_json::to_value(&value).unwrap();
            let tag = json.get("kind").unwrap().as_str().unwrap();
            assert_eq!(tag, component_kind_snake(kind), "kind={kind:?}");
        }
    }

    #[test]
    fn parse_component_kind_round_trips_every_kind() {
        for kind in ALL_KINDS {
            assert_eq!(parse_component_kind(component_kind_snake(kind)), Some(kind));
        }
    }

    #[test]
    fn parse_component_kind_rejects_unknown_string() {
        assert_eq!(parse_component_kind("not_a_component"), None);
        // PascalCase (the raw `ComponentKind` derive) is deliberately NOT a
        // valid filter string — only the snake_case envelope tag is.
        assert_eq!(parse_component_kind("Health"), None);
    }

    fn health_with_multipliers(pairs: &[(&str, f32)]) -> ComponentValue {
        let mut zone_multipliers = HashMap::new();
        for (tag, factor) in pairs {
            zone_multipliers.insert((*tag).to_string(), *factor);
        }
        ComponentValue::Health(HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers,
            contributor_ledger: Default::default(),
        })
    }

    #[test]
    fn deterministic_json_sorts_hashmap_keys_regardless_of_insertion_order() {
        // The HashMap-order determinism constraint: two logically-identical
        // payloads whose `zone_multipliers` were inserted in different orders
        // must serialize byte-for-byte identically.
        let forward = health_with_multipliers(&[("head", 2.0), ("leg", 0.5), ("torso", 1.0)]);
        let reverse = health_with_multipliers(&[("torso", 1.0), ("leg", 0.5), ("head", 2.0)]);

        let a = to_deterministic_json(&forward).unwrap();
        let b = to_deterministic_json(&reverse).unwrap();
        assert_eq!(a, b, "map key order must not leak into serialized output");
    }

    #[test]
    fn deterministic_json_is_stable_across_repeated_calls() {
        let value = health_with_multipliers(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);
        let first = to_deterministic_json(&value).unwrap();
        let second = to_deterministic_json(&value).unwrap();
        assert_eq!(first, second);
    }
}
