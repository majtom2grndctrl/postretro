//! Host-side seeded trigger-pool arming during level install.
//! See: context/plans/in-progress/E18--trap-pools-seeded-arming/index.md (Task 2)

use std::collections::HashSet;

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, TriggerPoolArm, TriggerPoolDescriptor,
    TriggerVolumeComponent,
};

use crate::kinematic_mover::MoverCommandDiagnostics;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_system::{arm_trigger_targets, disarm_trigger_targets};

/// Resolved per-install policy for trigger-pool arming. This intentionally does
/// not use `Option<u64>`: a missing seed is observably different from a seeded
/// roll because headless runs arm every member without consuming RNG.
#[cfg_attr(not(feature = "observability"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerPoolSeedPolicy {
    Seeded(u64),
    ArmAll,
}

/// Host-local result of one trigger-pool install pass. It is not replicated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TriggerPoolInstallReport {
    /// The seed used for this roll. `None` means the deterministic arm-all
    /// bypass ran instead of a roll.
    pub(crate) seed: Option<u64>,
    pub(crate) pools: Vec<TriggerPoolOutcome>,
}

/// One declared pool's sorted membership and final selected set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerPoolOutcome {
    pub(crate) tag: String,
    pub(crate) members: Vec<EntityId>,
    pub(crate) selected: Vec<EntityId>,
}

/// Deterministic SplitMix64 PRNG shared by trigger-pool arming and weapon
/// pellet spread. Trigger-pool selection remains a load-time decision, while
/// pellet sampling derives a fresh stream from replay-stable shell state.
/// Copied from the net harness rather than adding a `rand` dependency.
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance the stream for trigger-pool selection or pellet sampling.
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Derive a windowed-session seed without adding an RNG dependency. A new call
/// deliberately produces a fresh install seed, including for restarts.
pub(crate) fn entropy_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mixed_input = nanos as u64 ^ (nanos >> 64) as u64;
    SplitMix64::new(mixed_input).next_u64()
}

/// Resolve and apply the composed trigger-pool declarations for one level
/// install. Pools are processed in descriptor order so later overlapping pools
/// deliberately decide the final state for shared members.
pub(crate) fn install_trigger_pools(
    registry: &mut EntityRegistry,
    pools: &[TriggerPoolDescriptor],
    policy: TriggerPoolSeedPolicy,
    command_diagnostics: &MoverCommandDiagnostics,
    trigger_volume_bridge: &TriggerVolumeBridge,
) -> TriggerPoolInstallReport {
    let (seed, mut rng) = match policy {
        TriggerPoolSeedPolicy::Seeded(seed) => {
            log::info!("[TriggerPools] seed={seed}");
            (Some(seed), Some(SplitMix64::new(seed)))
        }
        TriggerPoolSeedPolicy::ArmAll => {
            log::info!("[TriggerPools] arm-all bypass active; every pool member will arm");
            (None, None)
        }
    };

    let mut report = TriggerPoolInstallReport {
        seed,
        pools: Vec::with_capacity(pools.len()),
    };
    let mut seen_members = HashSet::new();
    let mut overlap_warned = HashSet::new();
    let mut enabled_on_spawn_warned = HashSet::new();

    for pool in pools {
        let mut members: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(&pool.tag))
            .map(|(id, _)| id)
            .collect();
        members.sort_by(|a, b| trigger_volume_bridge.stable_order(*a, *b));

        if members.is_empty() {
            log::warn!(
                "[TriggerPools] pool '{}' matches no trigger volumes; skipping",
                pool.tag
            );
            continue;
        }

        for &member in &members {
            if !seen_members.insert(member) && overlap_warned.insert(member) {
                log::warn!(
                    "[TriggerPools] trigger {member} belongs to multiple pools; '{}' is the later decision",
                    pool.tag
                );
            }
            if enabled_on_spawn_warned.insert(member)
                && registry
                    .get_component::<TriggerVolumeComponent>(member)
                    .is_ok_and(|trigger| trigger.enabled_on_spawn)
            {
                log::warn!(
                    "[TriggerPools] trigger {member} in pool '{}' has enabled_on_spawn=true; install arming overrides it",
                    pool.tag
                );
            }
        }

        let selected = match (&pool.arm, rng.as_mut()) {
            (_, None) => members.clone(),
            (_, Some(rng)) => {
                select_members(&members, resolve_target_count(pool, members.len()), rng)
            }
        };
        let selected_set: HashSet<EntityId> = selected.iter().copied().collect();
        let disarmed: Vec<EntityId> = members
            .iter()
            .copied()
            .filter(|member| !selected_set.contains(member))
            .collect();

        arm_trigger_targets(registry, &selected, command_diagnostics);
        disarm_trigger_targets(registry, &disarmed, command_diagnostics);
        log::info!(
            "[TriggerPools] pool '{}' members={members:?} armed={selected:?}",
            pool.tag
        );
        report.pools.push(TriggerPoolOutcome {
            tag: pool.tag.clone(),
            members,
            selected,
        });
    }

    report
}

fn resolve_target_count(pool: &TriggerPoolDescriptor, member_count: usize) -> usize {
    match pool.arm {
        TriggerPoolArm::Count(requested) => {
            let requested = requested as usize;
            if requested > member_count {
                log::warn!(
                    "[TriggerPools] pool '{}' requested {requested} armed trigger(s), but has {member_count}; arming all",
                    pool.tag
                );
            }
            requested.min(member_count)
        }
        TriggerPoolArm::Percentage(percentage) => {
            // Keep this expression in the specified order. The descriptor drain
            // guarantees the finite [0, 100] input domain.
            ((percentage / 100.0) * member_count as f64).floor() as usize
        }
    }
}

fn select_members(members: &[EntityId], target: usize, rng: &mut SplitMix64) -> Vec<EntityId> {
    let mut shuffled = members.to_vec();
    for index in 0..target {
        let remaining = shuffled.len() - index;
        let choice = index + (rng.next_u64() % remaining as u64) as usize;
        shuffled.swap(index, choice);
    }
    let mut selected = shuffled[..target].to_vec();
    selected.sort_unstable();
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use glam::Vec3;
    use postretro_entities::{
        MoverCommand, Transform, TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
        PlayerMovementDescriptor, SpeedParams,
    };
    use postretro_level_format::trigger_volumes::TriggerVolumeRecord;

    use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerEventEdge, TriggerSystem};

    fn pool(tag: &str, arm: TriggerPoolArm) -> TriggerPoolDescriptor {
        TriggerPoolDescriptor {
            tag: tag.to_string(),
            arm,
            levels: Vec::new(),
        }
    }

    fn spawn_trigger(
        registry: &mut EntityRegistry,
        tags: &[&str],
        enabled_on_spawn: bool,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position: Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry
            .set_tags(id, tags.iter().map(|tag| (*tag).to_string()).collect())
            .expect("fresh trigger id accepts tags");
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    String::new(),
                    String::new(),
                    MoverCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    enabled_on_spawn,
                ),
            )
            .expect("fresh trigger id accepts component");
        id
    }

    fn authored_trigger_record(name: &str) -> TriggerVolumeRecord {
        TriggerVolumeRecord {
            name: name.to_string(),
            tags: vec!["closet".to_string()],
            aabb_min: [0.0, 0.0, 0.0],
            aabb_max: [1.0, 1.0, 1.0],
            activation: 0,
            target_tag: String::new(),
            command: 0,
            command_arg: String::new(),
            fire_mode: 0,
            rearm_ms: 0.0,
            enabled_on_spawn: false,
            on_fire: String::new(),
            on_exit: String::new(),
        }
    }

    fn selected_authored_names(
        report: &TriggerPoolInstallReport,
        bridge: &TriggerVolumeBridge,
    ) -> Vec<String> {
        let mut names: Vec<String> = report.pools[0]
            .selected
            .iter()
            .map(|id| {
                bridge
                    .name(*id)
                    .expect("selected authored trigger keeps its name")
                    .to_string()
            })
            .collect();
        names.sort();
        names
    }

    fn armed_members(registry: &EntityRegistry, ids: &[EntityId]) -> Vec<EntityId> {
        ids.iter()
            .copied()
            .filter(|id| {
                registry
                    .get_component::<TriggerVolumeComponent>(*id)
                    .expect("test trigger remains live")
                    .armed
            })
            .collect()
    }

    fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry
            .set_component(
                id,
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
                }),
            )
            .expect("fresh player accepts movement component");
        id
    }

    #[test]
    fn seeded_install_selects_a_stable_subset_and_overrides_authored_state() {
        let install = || {
            let mut registry = EntityRegistry::new();
            let ids = [
                spawn_trigger(&mut registry, &["closet"], true),
                spawn_trigger(&mut registry, &["closet"], false),
                spawn_trigger(&mut registry, &["closet"], true),
                spawn_trigger(&mut registry, &["closet"], false),
            ];
            let report = install_trigger_pools(
                &mut registry,
                &[pool("closet", TriggerPoolArm::Count(2))],
                TriggerPoolSeedPolicy::Seeded(17),
                &Default::default(),
                &Default::default(),
            );
            (report, armed_members(&registry, &ids))
        };

        let (first_report, first_armed) = install();
        let (second_report, second_armed) = install();

        assert_eq!(first_report.seed, Some(17));
        assert_eq!(first_report.pools[0].members.len(), 4);
        assert_eq!(first_armed.len(), 2);
        assert_eq!(first_report, second_report);
        assert_eq!(first_armed, second_armed);
        assert_eq!(first_report.pools[0].selected, first_armed);
    }

    // Regression: runtime trigger field changes reordered seeded pool members.
    #[test]
    fn runtime_only_pool_order_uses_entity_id_despite_field_changes() {
        let mut registry = EntityRegistry::new();
        let bridge = TriggerVolumeBridge::new();
        let ids = [
            spawn_trigger(&mut registry, &["runtime"], false),
            spawn_trigger(&mut registry, &["runtime"], false),
            spawn_trigger(&mut registry, &["runtime"], false),
        ];

        for (&id, x) in ids.iter().zip([3.0, 2.0, 1.0]) {
            let mut transform = *registry
                .get_component::<Transform>(id)
                .expect("runtime trigger transform remains attached");
            transform.position.x = x;
            registry
                .set_component(id, transform)
                .expect("runtime trigger accepts transform update");
        }
        let first = install_trigger_pools(
            &mut registry,
            &[pool("runtime", TriggerPoolArm::Count(1))],
            TriggerPoolSeedPolicy::Seeded(17),
            &Default::default(),
            &bridge,
        );

        for (&id, x) in ids.iter().zip([1.0, 3.0, 2.0]) {
            let mut transform = *registry
                .get_component::<Transform>(id)
                .expect("runtime trigger transform remains attached");
            transform.position.x = x;
            registry
                .set_component(id, transform)
                .expect("runtime trigger accepts transform update");
            let mut trigger = registry
                .get_component::<TriggerVolumeComponent>(id)
                .expect("runtime trigger component remains attached")
                .clone();
            trigger.on_fire = format!("changed-{x}");
            registry
                .set_component(id, trigger)
                .expect("runtime trigger accepts reaction-field update");
        }
        let second = install_trigger_pools(
            &mut registry,
            &[pool("runtime", TriggerPoolArm::Count(1))],
            TriggerPoolSeedPolicy::Seeded(17),
            &Default::default(),
            &bridge,
        );

        assert_eq!(first.pools[0].members, ids);
        assert_eq!(second.pools[0].members, ids);
        assert_eq!(first.pools[0].selected, second.pools[0].selected);
    }

    // Regression: unload recycled slots in reverse order, so tied authored
    // triggers could select a different map member with the same seed.
    #[test]
    fn same_registry_unload_reinstall_selects_same_authored_trigger_identity() {
        let records = ["alpha", "bravo", "charlie", "delta"].map(authored_trigger_record);
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();

        bridge.populate_from_level(&mut registry, &records);
        let first_report = install_trigger_pools(
            &mut registry,
            &[pool("closet", TriggerPoolArm::Count(1))],
            TriggerPoolSeedPolicy::Seeded(0),
            &Default::default(),
            &bridge,
        );
        let first_ids = first_report.pools[0].selected.clone();
        let first_names = selected_authored_names(&first_report, &bridge);

        registry.clear_for_level_unload();
        bridge.populate_from_level(&mut registry, &records);
        let second_report = install_trigger_pools(
            &mut registry,
            &[pool("closet", TriggerPoolArm::Count(1))],
            TriggerPoolSeedPolicy::Seeded(0),
            &Default::default(),
            &bridge,
        );
        let second_names = selected_authored_names(&second_report, &bridge);

        assert_ne!(
            first_ids, second_report.pools[0].selected,
            "the regression must exercise generation-bearing recycled IDs",
        );
        assert_eq!(first_names, second_names);
        assert_eq!(first_names, ["delta"]);
    }

    #[test]
    fn arm_all_bypass_ignores_declared_count_and_reports_no_seed() {
        let mut registry = EntityRegistry::new();
        let ids = [
            spawn_trigger(&mut registry, &["closet"], false),
            spawn_trigger(&mut registry, &["closet"], false),
        ];

        let report = install_trigger_pools(
            &mut registry,
            &[pool("closet", TriggerPoolArm::Count(0))],
            TriggerPoolSeedPolicy::ArmAll,
            &Default::default(),
            &Default::default(),
        );

        assert_eq!(report.seed, None);
        assert_eq!(report.pools[0].selected, ids);
        assert_eq!(armed_members(&registry, &ids), ids);
    }

    #[test]
    fn later_pool_overrides_an_overlapping_member() {
        let mut registry = EntityRegistry::new();
        let shared = spawn_trigger(&mut registry, &["first", "second"], false);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let _ = install_trigger_pools(
                &mut registry,
                &[
                    pool("first", TriggerPoolArm::Count(0)),
                    pool("second", TriggerPoolArm::Count(1)),
                ],
                TriggerPoolSeedPolicy::Seeded(9),
                &Default::default(),
                &Default::default(),
            );
        });

        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(shared)
                .expect("shared trigger remains live")
                .armed,
            "the later pool must make the final arm decision"
        );
        assert!(
            captured.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("multiple pools")
                    && message.contains("second")
            }),
            "expected one overlap warning, got {captured:?}"
        );
    }

    #[test]
    fn percentage_target_floors_before_selection() {
        let mut registry = EntityRegistry::new();
        let ids = [
            spawn_trigger(&mut registry, &["ambush"], false),
            spawn_trigger(&mut registry, &["ambush"], false),
            spawn_trigger(&mut registry, &["ambush"], false),
            spawn_trigger(&mut registry, &["ambush"], false),
        ];

        let report = install_trigger_pools(
            &mut registry,
            &[pool("ambush", TriggerPoolArm::Percentage(50.0))],
            TriggerPoolSeedPolicy::Seeded(3),
            &Default::default(),
            &Default::default(),
        );

        assert_eq!(report.pools[0].selected.len(), 2);
        assert_eq!(armed_members(&registry, &ids).len(), 2);
    }

    #[test]
    fn degradation_warns_and_keeps_empty_oversized_and_zero_pools_deterministic() {
        let mut registry = EntityRegistry::new();
        let oversized = [
            spawn_trigger(&mut registry, &["oversized"], true),
            spawn_trigger(&mut registry, &["oversized"], true),
        ];
        let zero_count = [
            spawn_trigger(&mut registry, &["zero-count"], false),
            spawn_trigger(&mut registry, &["zero-count"], false),
        ];
        let zero_percentage = [
            spawn_trigger(&mut registry, &["zero-percentage"], false),
            spawn_trigger(&mut registry, &["zero-percentage"], false),
        ];
        let exact = [
            spawn_trigger(&mut registry, &["exact"], false),
            spawn_trigger(&mut registry, &["exact"], false),
        ];
        let mut report = None;
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            report = Some(install_trigger_pools(
                &mut registry,
                &[
                    pool("missing", TriggerPoolArm::Count(1)),
                    pool("oversized", TriggerPoolArm::Count(3)),
                    pool("zero-count", TriggerPoolArm::Count(0)),
                    pool("zero-percentage", TriggerPoolArm::Percentage(25.0)),
                    pool("exact", TriggerPoolArm::Count(2)),
                ],
                TriggerPoolSeedPolicy::Seeded(17),
                &Default::default(),
                &Default::default(),
            ));
        });
        let report = report.expect("install pass returns its degradation report");

        assert_eq!(report.pools.len(), 4, "the empty pool stays inert");
        assert_eq!(armed_members(&registry, &oversized), oversized);
        assert!(armed_members(&registry, &zero_count).is_empty());
        assert!(armed_members(&registry, &zero_percentage).is_empty());
        assert_eq!(armed_members(&registry, &exact), exact);
        assert!(
            captured.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("matches no trigger volumes")
                    && message.contains("missing")
            }),
            "an empty tag must warn and skip; logs were {captured:?}",
        );
        assert!(
            captured.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("requested 3")
                    && message.contains("oversized")
            }),
            "an oversized count must clamp with a warning; logs were {captured:?}",
        );
        assert!(
            captured.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("enabled_on_spawn=true")
                    && message.contains("oversized")
            }),
            "pool members authored enabled must be diagnosed; logs were {captured:?}",
        );
        assert!(
            !captured.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("exact")
                    && message.contains("requested")
            }),
            "a count exactly equal to membership arms all without an over-count warning",
        );
    }

    #[test]
    fn unselected_pool_member_stays_silent_until_runtime_arm_reopens_it() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, &["quiet"], false);
        let mut component = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("fixture trigger remains live")
            .clone();
        component.on_fire = "trapPools.rearmed".to_string();
        registry
            .set_component(trigger, component)
            .expect("fixture trigger accepts an event name");
        let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
        let player_id = PlayerId::Local(player);
        let players = [AuthoritativePlayer {
            id: player_id,
            pawn: player,
        }];
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(
            trigger,
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 2.0, 1.0),
        );
        let report = install_trigger_pools(
            &mut registry,
            &[pool("quiet", TriggerPoolArm::Count(0))],
            TriggerPoolSeedPolicy::Seeded(17),
            &Default::default(),
            &bridge,
        );
        assert!(report.pools[0].selected.is_empty());

        let mut system = TriggerSystem::default();
        let first = system.run_authoritative_tick(
            &mut registry,
            &bridge,
            &players,
            &HashMap::new(),
            1.0 / 60.0,
        );
        assert!(
            first.fires.is_empty(),
            "the unselected member observes occupancy but must not fire",
        );

        arm_trigger_targets(&mut registry, &[trigger], &Default::default());
        let rearmed = system.run_authoritative_tick(
            &mut registry,
            &bridge,
            &players,
            &HashMap::new(),
            1.0 / 60.0,
        );
        assert_eq!(
            rearmed.fires.len(),
            1,
            "runtime armTrigger re-admits the standing player on the following tick",
        );
        assert_eq!(rearmed.fires[0].edge, TriggerEventEdge::Enter);
        assert_eq!(rearmed.fires[0].fire.trigger, trigger);
        assert_eq!(rearmed.fires[0].fire.player, player_id);
        assert_eq!(rearmed.fires[0].fire.event_name, "trapPools.rearmed");
    }
}
