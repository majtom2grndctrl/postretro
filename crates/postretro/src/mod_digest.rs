//! Compatibility digest for the mod-global trigger lanes that both peers evaluate.
//!
//! The recipe is intentionally narrow. Entity descriptors are replicated as tuning
//! values, while per-level declarations, reactions, and events stay outside this
//! digest by design.

use postretro_entities::{
    CrossingCondition, CrossingDescriptor, DataRegistry, ScopedCrossing, TriggerEventDescriptor,
    TriggerPoolArm, TriggerPoolDescriptor,
};

use crate::content_hash::{hash_f32, hash_f64, hash_ir_node, hash_len, hash_str, hash_u32};

/// Produce a deterministic digest over the three mod-global trigger lanes.
///
/// Every entry receives an independent canonical hash before lane ordering is
/// erased. The exhaustive walks below are a denylist: adding a field or enum
/// variant in the reached domain fails compilation until its representation is
/// chosen here.
pub(crate) fn mod_compatibility_digest(
    trigger_events: &[TriggerEventDescriptor],
    trigger_pools: &[TriggerPoolDescriptor],
    crossings: &[ScopedCrossing],
) -> [u8; 32] {
    const MOD_DIGEST_EPOCH: u32 = 1;

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"postretro-mod-compatibility");
    hasher.update(&MOD_DIGEST_EPOCH.to_le_bytes());
    hash_lane(&mut hasher, trigger_events, hash_trigger_event_descriptor);
    hash_lane(&mut hasher, trigger_pools, hash_trigger_pool_descriptor);
    hash_lane(&mut hasher, crossings, hash_scoped_crossing);
    *hasher.finalize().as_bytes()
}

/// Produce the compatibility digest from committed mod-global registry state.
pub(crate) fn mod_compatibility_digest_from_registry(registry: &DataRegistry) -> [u8; 32] {
    mod_compatibility_digest(
        &registry.global_trigger_events,
        &registry.global_trigger_pools,
        &registry.global_crossings,
    )
}

fn hash_lane<T>(
    hasher: &mut blake3::Hasher,
    entries: &[T],
    hash_entry: fn(&mut blake3::Hasher, &T),
) {
    let mut digests: Vec<[u8; 32]> = entries
        .iter()
        .map(|entry| {
            let mut entry_hasher = blake3::Hasher::new();
            hash_entry(&mut entry_hasher, entry);
            *entry_hasher.finalize().as_bytes()
        })
        .collect();
    digests.sort_unstable();

    hash_len(hasher, digests.len());
    for digest in digests {
        hasher.update(&digest);
    }
}

fn hash_trigger_event_descriptor(hasher: &mut blake3::Hasher, descriptor: &TriggerEventDescriptor) {
    let TriggerEventDescriptor {
        tag,
        event,
        fire,
        levels,
    } = descriptor;
    hash_str(hasher, tag);
    hash_str(hasher, event);
    hash_strings(hasher, fire);
    hash_strings(hasher, levels);
}

fn hash_trigger_pool_descriptor(hasher: &mut blake3::Hasher, descriptor: &TriggerPoolDescriptor) {
    let TriggerPoolDescriptor { tag, arm, levels } = descriptor;
    hash_str(hasher, tag);
    hash_trigger_pool_arm(hasher, arm);
    hash_strings(hasher, levels);
}

fn hash_trigger_pool_arm(hasher: &mut blake3::Hasher, arm: &TriggerPoolArm) {
    match arm {
        TriggerPoolArm::Count(count) => {
            hasher.update(&[0]);
            hash_u32(hasher, *count);
        }
        TriggerPoolArm::Percentage(percentage) => {
            hasher.update(&[1]);
            hash_f64(hasher, *percentage);
        }
    }
}

fn hash_scoped_crossing(hasher: &mut blake3::Hasher, scoped: &ScopedCrossing) {
    let ScopedCrossing { crossing, levels } = scoped;
    hash_crossing_descriptor(hasher, crossing);
    hash_strings(hasher, levels);
}

fn hash_crossing_descriptor(hasher: &mut blake3::Hasher, descriptor: &CrossingDescriptor) {
    let CrossingDescriptor {
        slot,
        condition,
        max,
        edge,
        fire,
    } = descriptor;
    hash_option_string(hasher, slot);
    hash_crossing_condition(hasher, condition);
    hash_f32(hasher, *max);
    hash_option_string(hasher, edge);
    hash_strings(hasher, fire);
}

fn hash_crossing_condition(hasher: &mut blake3::Hasher, condition: &CrossingCondition) {
    match condition {
        CrossingCondition::Below { threshold } => {
            hasher.update(&[0]);
            hash_f32(hasher, *threshold);
        }
        CrossingCondition::Above { threshold } => {
            hasher.update(&[1]);
            hash_f32(hasher, *threshold);
        }
        CrossingCondition::Ir(node) => {
            hasher.update(&[2]);
            hash_ir_node(hasher, node);
        }
    }
}

fn hash_option_string(hasher: &mut blake3::Hasher, value: &Option<String>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hash_str(hasher, value);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn hash_strings(hasher: &mut blake3::Hasher, values: &[String]) {
    hash_len(hasher, values.len());
    for value in values {
        hash_str(hasher, value);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::fmt::Write as _;
    use std::fs;
    use std::path::PathBuf;

    use postretro_entities::slot_table::StoreDeclarationSet;
    use postretro_entities::{
        AirParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
        CapsuleParams, EntityTypeDescriptor, FallParams, FireMode, GroundParams, HealthDescriptor,
        ImpactEventDescriptor, MeshDescriptor, MotionVerb, PlayerMovementDescriptor,
        PrimitiveDescriptor, ReactionDescriptor, ScopedReaction, SpeedParams, WeaponDescriptor,
    };
    use postretro_foundation::ir::{IrNode, IrValue};
    use postretro_scripting_core::data_descriptors::{ModFontAssets, ModThemeTokens};
    use postretro_scripting_core::runtime::{ModManifestResult, ModRenderProfile};

    use super::*;

    const BLESS_ENV: &str = "POSTRETRO_BLESS_COMPATIBILITY_FIXTURES";
    const FIXTURE_DIGEST_HEX: &str =
        "7e92147feab99827d4824b740070f164d94963fa211852a460b2c8017a5c19e0";

    fn events() -> Vec<TriggerEventDescriptor> {
        vec![
            TriggerEventDescriptor {
                tag: "level-start".to_string(),
                event: "entered".to_string(),
                fire: vec!["raise-gate".to_string(), "play-sting".to_string()],
                levels: vec!["arena".to_string()],
            },
            TriggerEventDescriptor {
                tag: "level-start".to_string(),
                event: "after-entered".to_string(),
                fire: vec!["arm-pool".to_string()],
                levels: Vec::new(),
            },
        ]
    }

    fn pools() -> Vec<TriggerPoolDescriptor> {
        vec![
            TriggerPoolDescriptor {
                tag: "ambushes".to_string(),
                arm: TriggerPoolArm::Percentage(0.125),
                levels: vec!["arena".to_string()],
            },
            TriggerPoolDescriptor {
                tag: "rewards".to_string(),
                arm: TriggerPoolArm::Count(3),
                levels: Vec::new(),
            },
        ]
    }

    fn crossing(condition: CrossingCondition) -> ScopedCrossing {
        ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: Some("player.shield".to_string()),
                condition,
                max: 100.0,
                edge: Some("both".to_string()),
                fire: vec!["shield-warning".to_string()],
            },
            levels: vec!["arena".to_string()],
        }
    }

    fn crossings() -> Vec<ScopedCrossing> {
        vec![
            crossing(CrossingCondition::Below { threshold: 0.25 }),
            crossing(CrossingCondition::Ir(IrNode::Gt {
                a: Box::new(IrNode::Input {
                    name: "player.speed".to_string(),
                    owner: None,
                }),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(3.5),
                }),
            })),
        ]
    }

    fn digest() -> [u8; 32] {
        mod_compatibility_digest(&events(), &pools(), &crossings())
    }

    fn manifest() -> ModManifestResult {
        ModManifestResult {
            name: "Digest fixture".to_string(),
            id: "com.postretro.digest-fixture".to_string(),
            version: "1.0.0".to_string(),
            render: ModRenderProfile::default(),
            movers: Default::default(),
            switching: Default::default(),
            default_weapon_placement: None,
            entities: vec![entity_descriptor()],
            ui_trees: Vec::new(),
            presentation_templates: Vec::new(),
            presentation_overlays: Vec::new(),
            theme: ModThemeTokens::default(),
            frontend: None,
            fonts: ModFontAssets::default(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: crossings(),
            events: Vec::new(),
            trigger_events: events(),
            trigger_pools: pools(),
            store_declarations: StoreDeclarationSet::default(),
        }
    }

    fn manifest_digest(manifest: &ModManifestResult) -> [u8; 32] {
        let mut registry = DataRegistry::new();
        for entity in manifest.entities.clone() {
            registry.upsert_entity_type(entity);
        }
        registry.replace_global_reactions(manifest.reactions.clone());
        registry.replace_global_crossings(manifest.crossings.clone());
        registry.replace_global_trigger_events(manifest.trigger_events.clone());
        registry.replace_global_trigger_pools(manifest.trigger_pools.clone());
        mod_compatibility_digest_from_registry(&registry)
    }

    fn entity_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("fixture-entity".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 4.0,
                    run: 7.0,
                    crouch: 2.0,
                },
                accel: 18.0,
                step_height: 0.35,
                max_slope: 48.0,
            },
            air: AirParams {
                forward_steer: 0.25,
                accel: 3.0,
                max_control_speed: 8.0,
                bunny_hop: true,
                jumps: 2,
                jump_velocity: 5.5,
                jump_ceiling: 1.5,
            },
            fall: FallParams {
                terminal_velocity: 42.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        }
    }

    fn behavior_descriptor() -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                )]),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed: 3.0,
        }
    }

    fn entity_descriptor_edits() -> Vec<(&'static str, EntityTypeDescriptor)> {
        let mut movement = entity_descriptor();
        movement.movement = Some(movement_descriptor());

        let mut weapon = entity_descriptor();
        weapon.weapon = Some(WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: postretro_entities::ResolutionMode::Hitscan,
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
        });

        let mut inventory = entity_descriptor();
        inventory.inventory = Some(postretro_entities::InventoryDescriptor {
            loadout: vec!["fixture-weapon".to_string()],
        });

        let mut health = entity_descriptor();
        health.health = Some(HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });

        let mut behavior = entity_descriptor();
        behavior.behavior = Some(behavior_descriptor());

        let mut canonical_name = entity_descriptor();
        canonical_name.canonical_name = Some("changed-fixture-entity".to_string());

        let mut presentation = entity_descriptor();
        presentation.mesh = Some(MeshDescriptor {
            model: "models/fixture.glb".to_string(),
            shadow_only: false,
            attachments: HashMap::new(),
            shadow_bias_scale: 1.0,
            animations: HashMap::new(),
            default_state: None,
            locomotion: None,
        });

        vec![
            ("movement", movement),
            ("weapon", weapon),
            ("inventory", inventory),
            ("health", health),
            ("behavior", behavior),
            ("canonical_name", canonical_name),
            ("presentation", presentation),
        ]
    }

    #[test]
    fn digest_is_order_independent_within_each_lane() {
        let events = events();
        let pools = pools();
        let crossings = crossings();
        let expected = mod_compatibility_digest(&events, &pools, &crossings);

        let mut reversed_events = events;
        let mut reversed_pools = pools;
        let mut reversed_crossings = crossings;
        reversed_events.reverse();
        reversed_pools.reverse();
        reversed_crossings.reverse();

        assert_eq!(
            expected,
            mod_compatibility_digest(&reversed_events, &reversed_pools, &reversed_crossings)
        );
    }

    #[test]
    fn digest_changes_for_crossing_event_and_pool_edits() {
        let baseline = digest();

        let mut changed_crossings = crossings();
        changed_crossings[0].crossing.condition = CrossingCondition::Below { threshold: 0.5 };
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_crossings = crossings();
        changed_crossings[0].crossing.edge = None;
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_crossings = crossings();
        changed_crossings[1].crossing.condition = CrossingCondition::Ir(IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(3.5),
            }),
        });
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &pools(), &changed_crossings)
        );

        let mut changed_events = events();
        changed_events[0].event = "entered-late".to_string();
        assert_ne!(
            baseline,
            mod_compatibility_digest(&changed_events, &pools(), &crossings())
        );

        let mut changed_pools = pools();
        changed_pools[0].arm = TriggerPoolArm::Percentage(0.25);
        assert_ne!(
            baseline,
            mod_compatibility_digest(&events(), &changed_pools, &crossings())
        );
    }

    #[test]
    fn digest_ignores_declared_entity_descriptor_lanes() {
        let baseline = manifest();
        let expected = manifest_digest(&baseline);

        for (field, entity) in entity_descriptor_edits() {
            let mut changed = baseline.clone();
            changed.entities = vec![entity];
            assert_eq!(
                expected,
                manifest_digest(&changed),
                "entity descriptor {field} is intentionally outside the mod digest"
            );
        }
    }

    #[test]
    fn digest_ignores_declared_reaction_and_impact_event_lanes() {
        let baseline = manifest();
        let expected = manifest_digest(&baseline);

        let mut reaction_changed = baseline.clone();
        reaction_changed.reactions.push(ScopedReaction {
            reaction: postretro_entities::NamedReaction {
                name: "fixture-reaction".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "playSound".to_string(),
                    target: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "sound": "fixture" }),
                }),
            },
            levels: vec!["arena".to_string()],
        });
        assert_eq!(expected, manifest_digest(&reaction_changed));

        let mut impact_event_changed = baseline;
        impact_event_changed.events.push(ImpactEventDescriptor {
            id: "fixture-impact".to_string(),
            is_override: false,
            levels: vec!["arena".to_string()],
            filter_tag: Some("enemy".to_string()),
            policy: vec![serde_json::json!({ "kind": "damage", "amount": 5 })],
        });
        assert_eq!(expected, manifest_digest(&impact_event_changed));
    }

    #[test]
    fn digest_ir_is_structural() {
        let first = crossing(CrossingCondition::Ir(IrNode::Add {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        }));
        let equal = first.clone();
        let different = crossing(CrossingCondition::Ir(IrNode::Sub {
            a: Box::new(IrNode::Input {
                name: "player.speed".to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        }));

        assert_eq!(
            mod_compatibility_digest(&[], &[], &[first]),
            mod_compatibility_digest(&[], &[], &[equal])
        );
        assert_ne!(
            mod_compatibility_digest(
                &[],
                &[],
                &[crossing(CrossingCondition::Ir(IrNode::Add {
                    a: Box::new(IrNode::Input {
                        name: "player.speed".to_string(),
                        owner: None,
                    }),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(1.0),
                    }),
                }))]
            ),
            mod_compatibility_digest(&[], &[], &[different])
        );
    }

    #[test]
    fn committed_digest_fixture_is_stable() {
        let actual = hex(digest());
        if std::env::var_os(BLESS_ENV).is_some() {
            bless_digest_constant(&actual);
            return;
        }

        assert_eq!(
            actual, FIXTURE_DIGEST_HEX,
            "mod digest changed; if intentional, bump MOD_DIGEST_EPOCH and re-bless with {BLESS_ENV}=1"
        );
    }

    fn hex(digest: [u8; 32]) -> String {
        let mut output = String::with_capacity(64);
        for byte in digest {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
        }
        output
    }

    fn bless_digest_constant(actual: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mod_digest.rs");
        let source = fs::read_to_string(&path).expect("read mod digest source for bless");
        let old = format!("const FIXTURE_DIGEST_HEX: &str = \"{FIXTURE_DIGEST_HEX}\";");
        let new = format!("const FIXTURE_DIGEST_HEX: &str = \"{actual}\";");
        assert!(
            source.contains(&old),
            "digest bless marker missing from {}",
            path.display()
        );
        fs::write(&path, source.replacen(&old, &new, 1)).expect("write mod digest bless result");
    }

    // This is deliberately inert. It breaks when a reached descriptor field or
    // enum variant is added; update the recipe and this sentinel together, never
    // widen either destructuring pattern with `..` or a wildcard arm.
    #[allow(dead_code)]
    fn exhaustive_domain_sentinel(
        scoped: ScopedCrossing,
        crossing: CrossingDescriptor,
        condition: CrossingCondition,
        event: TriggerEventDescriptor,
        pool: TriggerPoolDescriptor,
        arm: TriggerPoolArm,
        node: IrNode,
        value: IrValue,
    ) {
        let ScopedCrossing {
            crossing: _,
            levels: _,
        } = scoped;
        let CrossingDescriptor {
            slot: _,
            condition: _,
            max: _,
            edge: _,
            fire: _,
        } = crossing;
        let TriggerEventDescriptor {
            tag: _,
            event: _,
            fire: _,
            levels: _,
        } = event;
        let TriggerPoolDescriptor {
            tag: _,
            arm: _,
            levels: _,
        } = pool;
        match condition {
            CrossingCondition::Below { threshold: _ } => {}
            CrossingCondition::Above { threshold: _ } => {}
            CrossingCondition::Ir(_) => {}
        }
        match arm {
            TriggerPoolArm::Count(_) => {}
            TriggerPoolArm::Percentage(_) => {}
        }
        match node {
            IrNode::Const { value: _ } => {}
            IrNode::Input { name: _, owner: _ } => {}
            IrNode::Add { a: _, b: _ } => {}
            IrNode::Sub { a: _, b: _ } => {}
            IrNode::Mul { a: _, b: _ } => {}
            IrNode::Div { a: _, b: _ } => {}
            IrNode::Clamp { x: _, lo: _, hi: _ } => {}
            IrNode::Lerp { a: _, b: _, t: _ } => {}
            IrNode::Lt { a: _, b: _ } => {}
            IrNode::Le { a: _, b: _ } => {}
            IrNode::Gt { a: _, b: _ } => {}
            IrNode::Ge { a: _, b: _ } => {}
            IrNode::Eq { a: _, b: _ } => {}
            IrNode::Ne { a: _, b: _ } => {}
            IrNode::And { a: _, b: _ } => {}
            IrNode::Or { a: _, b: _ } => {}
            IrNode::Not { x: _ } => {}
            IrNode::Select {
                cond: _,
                a: _,
                b: _,
            } => {}
        }
        match value {
            IrValue::Bool(_) => {}
            IrValue::Number(_) => {}
        }
    }
}
