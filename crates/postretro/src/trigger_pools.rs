//! Host-side seeded trigger-pool arming during level install.
//! See: context/plans/in-progress/E18--trap-pools-seeded-arming/index.md (Task 2)

use std::collections::HashSet;

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, TriggerPoolArm, TriggerPoolDescriptor,
    TriggerVolumeComponent,
};

use crate::kinematic_mover::MoverCommandDiagnostics;
use crate::trigger_system::{arm_trigger_targets, disarm_trigger_targets};

/// Resolved per-install policy for trigger-pool arming. This intentionally does
/// not use `Option<u64>`: a missing seed is observably different from a seeded
/// roll because headless runs arm every member without consuming RNG.
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

/// Deterministic SplitMix64 PRNG. This is deliberately local to the install
/// pass: pool selection is a load-time decision and never enters tick or wire
/// state. Copied from the net harness rather than adding a `rand` dependency.
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
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
        members.sort_unstable();

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
            (arm, Some(rng)) => {
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

    use glam::Vec3;
    use postretro_entities::{
        MoverCommand, Transform, TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
    };

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
            install_trigger_pools(
                &mut registry,
                &[
                    pool("first", TriggerPoolArm::Count(0)),
                    pool("second", TriggerPoolArm::Count(1)),
                ],
                TriggerPoolSeedPolicy::Seeded(9),
                &Default::default(),
            )
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
        );

        assert_eq!(report.pools[0].selected.len(), 2);
        assert_eq!(armed_members(&registry, &ids).len(), 2);
    }
}
