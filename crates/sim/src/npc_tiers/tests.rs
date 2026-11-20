// Tier choice tests on the three-arena synthetic level: a player pawn in arena
// 0 (cells 0..10) perceives that arena; arena 2 (cells 20..30) is unseen.
// See: context/lib/entity_model.md §7c

use super::*;
use crate::npc_perception::test_support::{Row, centre, spawn_at, three_arenas};
use crate::npc_perception::{GhostLinkCells, resolve_ai_lead};
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
use postretro_entities::components::brain::AlwaysActive;
use postretro_entities::provenance::DescriptorSpawnPath;

const ARCHETYPE: &str = "tier_grunt";

struct Harness {
    row: Row,
    registry: EntityRegistry,
    scheduler: NpcScheduler,
    descriptors: Vec<EntityTypeDescriptor>,
    generation: u64,
    pawn: EntityId,
}

impl Harness {
    fn new() -> Self {
        let row = three_arenas();
        let mut registry = EntityRegistry::new();
        let pawn = spawn_at(&mut registry, centre(&row, 3));
        let perception = NpcPerception::new(
            30,
            resolve_ai_lead(None, row.residency.as_ref().map(|set| set.max_lead)),
            GhostLinkCells::default(),
        );
        Self {
            row,
            registry,
            scheduler: NpcScheduler::new(perception),
            descriptors: vec![behavior_enemy_descriptor(ARCHETYPE)],
            generation: 1,
            pawn,
        }
    }

    /// An NPC of the test archetype at `cell`, with an identity.
    fn npc(&mut self, cell: usize, identity: u64) -> EntityId {
        let id = spawn_at(&mut self.registry, centre(&self.row, cell));
        let graph = self.descriptors[0].behavior.clone().unwrap();
        let mut brain = BrainComponent::from_graph(&graph);
        brain.schedule_identity = Some(identity);
        self.registry.set_component(id, brain).unwrap();
        self.registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: ARCHETYPE.to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        id
    }

    fn choose(&mut self) {
        let NpcScheduler { perception, tiers } = &mut self.scheduler;
        perception.step(&self.row, &self.registry, [self.pawn]);
        tiers.choose(
            &self.registry,
            perception,
            &TierInputs {
                level: &self.row,
                descriptors: &self.descriptors,
                descriptor_generation: self.generation,
            },
        );
    }

    fn tick(&self) -> u64 {
        self.scheduler.tiers.tick()
    }

    fn tier(&self, id: EntityId) -> Option<NpcTier> {
        self.scheduler.tiers.tier(id)
    }

    fn place(&mut self, id: EntityId, cell: usize) {
        self.registry
            .set_component(
                id,
                Transform {
                    position: centre(&self.row, cell),
                    ..Transform::default()
                },
            )
            .unwrap();
    }

    fn brain(&mut self, id: EntityId, edit: impl FnOnce(&mut BrainComponent)) {
        let mut brain = self
            .registry
            .get_component::<BrainComponent>(id)
            .unwrap()
            .clone();
        edit(&mut brain);
        self.registry.set_component(id, brain).unwrap();
    }

    /// Run until `id`'s next dormant run; return how many ticks were skipped.
    fn until_dormant_run(&mut self, id: EntityId) -> u64 {
        for skipped in 0..DORMANT_INTERVAL_TICKS {
            self.choose();
            if matches!(self.scheduler.tiers.ai_run(id), AiRun::RunDormant { .. }) {
                return skipped;
            }
            assert_eq!(self.scheduler.tiers.ai_run(id), AiRun::Skip);
            assert_eq!(self.scheduler.tiers.agent_step(id), AgentStep::CarryOnly);
        }
        panic!("no dormant run within one interval");
    }
}

#[test]
fn dormant_runs_once_per_interval_and_decides_every_fourth_run() {
    for identity in [0, 1, 7, 31, 1 << 40, u64::MAX] {
        for start in 0..4 * DORMANT_INTERVAL_TICKS {
            let window = start..start + DORMANT_INTERVAL_TICKS;
            let runs = window
                .clone()
                .filter(|&tick| dormant_runs_on(tick, identity))
                .count();
            assert_eq!(runs, 1, "identity {identity} from {start}");
            let window = start..start + MOVE_DOWN_HOLD_TICKS;
            let decides = window
                .filter(|&tick| dormant_decides_on(tick, identity))
                .count();
            assert_eq!(decides, 1, "identity {identity} from {start}");
        }
    }
}

#[test]
fn unseen_npcs_start_dormant_and_seen_ones_idle() {
    let mut h = Harness::new();
    let seen = h.npc(5, 1);
    let unseen = h.npc(25, 2);
    h.choose();
    assert_eq!(h.tier(seen), Some(NpcTier::Idle));
    assert_eq!(h.tier(unseen), Some(NpcTier::Dormant));
    assert_eq!(h.scheduler.tiers.ai_run(seen), AiRun::Run);
    assert_eq!(h.scheduler.tiers.agent_step(seen), AgentStep::Ticks(1));
}

#[test]
fn move_up_on_the_tick_it_becomes_perceivable_mid_step_with_one_ordinary_step() {
    let mut h = Harness::new();
    let npc = h.npc(25, 3);
    h.choose();
    h.until_dormant_run(npc);
    // Ten ticks into its dormant interval it walks into view.
    for _ in 0..10 {
        h.choose();
    }
    h.place(npc, 9);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    assert_eq!(h.scheduler.tiers.ai_run(npc), AiRun::Run);
    // It moves one ordinary tick, never the ticks it did not run.
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
}

#[test]
fn move_up_on_damage_or_an_opened_aggro_gate() {
    let mut h = Harness::new();
    let hurt = h.npc(25, 4);
    let gated = h.npc(26, 5);
    h.brain(gated, |brain| brain.aggro_armed = false);
    h.choose();
    h.choose();
    assert_eq!(h.tier(hurt), Some(NpcTier::Dormant));
    assert_eq!(h.tier(gated), Some(NpcTier::Dormant));

    // A hit zeroes its damage clock; the trigger opens the gate.
    h.brain(hurt, |brain| brain.time_since_damage_ms = 0.0);
    h.brain(gated, |brain| brain.aggro_armed = true);
    h.choose();
    assert_eq!(h.tier(hurt), Some(NpcTier::Idle));
    assert_eq!(h.tier(gated), Some(NpcTier::Idle));
    assert_eq!(h.scheduler.tiers.ai_run(hurt), AiRun::Run);
}

#[test]
fn move_down_waits_the_hold_and_steps_exactly_the_ticks_since_its_last_run() {
    let mut h = Harness::new();
    let npc = h.npc(5, 6);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    let last_seen = h.tick();
    h.place(npc, 25);
    loop {
        h.choose();
        if h.tier(npc) == Some(NpcTier::Dormant) {
            break;
        }
        assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
    }
    // It moves down on the first tick 2 s after it was last perceived.
    assert_eq!(h.tick() - last_seen, MOVE_DOWN_HOLD_TICKS);
    let last_step = h.tick() - 1;
    let mut skipped = 0;
    if !matches!(h.scheduler.tiers.ai_run(npc), AiRun::RunDormant { .. }) {
        skipped = 1 + h.until_dormant_run(npc);
    }
    // Its first dormant run steps exactly the ticks since its last step,
    // never a whole interval by default.
    assert_eq!(
        h.scheduler.tiers.agent_step(npc),
        AgentStep::Ticks((h.tick() - last_step) as u32)
    );
    assert!(skipped < DORMANT_INTERVAL_TICKS);
}

#[test]
fn move_down_never_happens_to_one_crossing_a_set_edge_every_half_second() {
    let mut h = Harness::new();
    let npc = h.npc(9, 7);
    for half_second in 0..20 {
        let cell = if half_second % 2 == 0 { 9 } else { 10 };
        h.place(npc, cell);
        for _ in 0..DORMANT_INTERVAL_TICKS {
            h.choose();
            assert_eq!(h.tier(npc), Some(NpcTier::Idle));
        }
    }
}

#[test]
fn move_down_waits_for_a_retained_target_to_drop_and_then_the_hold() {
    let mut h = Harness::new();
    let npc = h.npc(25, 8);
    let target = h.pawn;
    h.brain(npc, |brain| brain.acquired_target = Some(target));
    for _ in 0..3 * MOVE_DOWN_HOLD_TICKS {
        h.choose();
        assert_eq!(h.tier(npc), Some(NpcTier::Engaged));
        assert_eq!(h.scheduler.tiers.ai_run(npc), AiRun::Run);
    }
    h.brain(npc, |brain| brain.acquired_target = None);
    for _ in 0..MOVE_DOWN_HOLD_TICKS - 1 {
        h.choose();
        assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    }
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));
}

#[test]
fn a_dormant_run_that_acquires_a_target_steps_one_ordinary_tick() {
    let mut h = Harness::new();
    let npc = h.npc(25, 9);
    h.choose();
    h.until_dormant_run(npc);
    h.until_dormant_run(npc);
    assert!(matches!(
        h.scheduler.tiers.agent_step(npc),
        AgentStep::Ticks(n) if n > 1
    ));
    // The AI stage acquired on that run.
    let target = h.pawn;
    h.brain(npc, |brain| brain.acquired_target = Some(target));
    h.scheduler.tiers.settle_after_ai(&h.registry);
    assert_eq!(h.tier(npc), Some(NpcTier::Engaged));
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
    h.choose();
    assert_eq!(h.scheduler.tiers.ai_run(npc), AiRun::Run);
}

#[test]
fn a_dead_npc_leaves_the_tiers_and_rejoins_on_its_revive_tick() {
    let mut h = Harness::new();
    let npc = h.npc(25, 10);
    h.registry
        .set_component(
            npc,
            postretro_entities::components::health::HealthComponent {
                max: 10.0,
                current: 10.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));
    let mut health = h
        .registry
        .get_component::<postretro_entities::components::health::HealthComponent>(npc)
        .unwrap()
        .clone();
    health.current = 0.0;
    h.registry.set_component(npc, health.clone()).unwrap();
    h.choose();
    assert_eq!(h.tier(npc), None, "a dead NPC is outside the tiers");
    // Outside the tiers its corpse steps every tick.
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));

    health.current = 10.0;
    h.registry.set_component(npc, health).unwrap();
    h.place(npc, 4);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    // A fresh entry: no step owed for the time dead.
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
}

/// Full health for a tier test NPC.
fn alive() -> postretro_entities::components::health::HealthComponent {
    postretro_entities::components::health::HealthComponent {
        max: 10.0,
        current: 10.0,
        hitbox: None,
        death_handled: false,
        pending_kill_credit: None,
        zone_multipliers: Default::default(),
        contributor_ledger: Default::default(),
    }
}

/// A hit through the damage chokepoint, as every producer lands one.
fn hit(registry: &mut EntityRegistry, id: EntityId, amount: f32) {
    use postretro_entities::components::health::{DamageContext, DamageProducer};
    assert!(
        postretro_entities::components::health::apply_damage_with_context(
            registry,
            id,
            &postretro_foundation::DamagePayload {
                amount,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            DamageContext::new("test", DamageProducer::InTick),
        ),
        "the hit landed"
    );
}

/// Kill `npc` with a hit and take it out of the tiers, then revive it as a
/// script's health write does.
fn kill_and_revive(h: &mut Harness, npc: EntityId) {
    hit(&mut h.registry, npc, 10.0);
    h.choose();
    assert_eq!(h.tier(npc), None, "dead, it leaves the tiers");
    postretro_entities::components::health::set_health_absolute(&mut h.registry, npc, 10.0);
}

// Regression: a revived NPC's damage clock still read its lethal hit (a
// quiescent brain is never aged), so it rejoined held idle for the whole 2 s.
#[test]
fn a_revived_npc_unseen_goes_dormant_after_its_first_run_whatever_its_lethal_hit() {
    let mut h = Harness::new();
    let npc = h.npc(25, 18);
    h.registry.set_component(npc, alive()).unwrap();
    h.choose();
    kill_and_revive(&mut h, npc);

    // Revived out of sight: its first tick runs, as any NPC joining does.
    h.choose();
    assert_eq!(h.scheduler.tiers.ai_run(npc), AiRun::Run);
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));
    for _ in 0..DORMANT_INTERVAL_TICKS {
        h.choose();
        assert_eq!(
            h.tier(npc),
            Some(NpcTier::Dormant),
            "no hold for a hit from before it died"
        );
    }
}

// Regression: the tiers ignored a zero damage clock on an NPC's join tick to
// skip a revived NPC's lethal hit, which also dropped a real hit landing then.
#[test]
fn an_npc_hit_on_its_join_tick_holds_idle_for_the_hold() {
    for revived in [false, true] {
        let mut h = Harness::new();
        // Spawned out of sight, or revived there.
        let npc = h.npc(25, 21);
        h.registry.set_component(npc, alive()).unwrap();
        if revived {
            h.choose();
            kill_and_revive(&mut h, npc);
        }

        // Hit before the tier choice on the tick it joins.
        hit(&mut h.registry, npc, 1.0);
        h.choose();
        let joined = h.tick();
        assert_eq!(
            h.tier(npc),
            Some(NpcTier::Idle),
            "revived {revived}: the hit holds it idle"
        );
        // Its run ages the clock one tick, as the AI does.
        h.brain(npc, |brain| brain.time_since_damage_ms = 1000.0 / 60.0);
        loop {
            h.choose();
            if h.tier(npc) == Some(NpcTier::Dormant) {
                break;
            }
            assert_eq!(h.tier(npc), Some(NpcTier::Idle), "revived {revived}");
        }
        assert_eq!(
            h.tick() - joined,
            MOVE_DOWN_HOLD_TICKS,
            "revived {revived}: held the whole 2 s"
        );
    }
}

// An NPC acquiring on any run ranks engaged for this tick's replans: a fresh
// spawn on its first run and an idle NPC alike. Only a dormant run's step is
// rewritten to one ordinary tick.
#[test]
fn any_run_that_acquires_a_target_ranks_engaged_on_that_tick() {
    let mut h = Harness::new();
    let seen = h.npc(5, 19);
    h.choose();
    assert_eq!(h.tier(seen), Some(NpcTier::Idle));
    let spawned = h.npc(25, 20);
    h.choose();
    assert_eq!(h.tier(spawned), Some(NpcTier::Dormant));
    assert_eq!(
        h.scheduler.tiers.ai_run(spawned),
        AiRun::Run,
        "its first run"
    );
    assert_eq!(h.scheduler.tiers.replan_rank(seen), 1);
    assert_eq!(h.scheduler.tiers.replan_rank(spawned), 0);

    // The AI stage acquired on both runs.
    let target = h.pawn;
    h.brain(seen, |brain| brain.acquired_target = Some(target));
    h.brain(spawned, |brain| brain.acquired_target = Some(target));
    h.scheduler.tiers.settle_after_ai(&h.registry);
    for npc in [seen, spawned] {
        assert_eq!(h.tier(npc), Some(NpcTier::Engaged));
        assert_eq!(h.scheduler.tiers.replan_rank(npc), 2);
        assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
    }

    // A skipped dormant NPC ran nothing this tick: it stays where it was.
    let skipped = h.npc(26, 21);
    h.choose();
    h.until_dormant_run(skipped);
    h.choose();
    assert_eq!(h.scheduler.tiers.ai_run(skipped), AiRun::Skip);
    h.brain(skipped, |brain| brain.acquired_target = Some(target));
    h.scheduler.tiers.settle_after_ai(&h.registry);
    assert_eq!(h.tier(skipped), Some(NpcTier::Dormant));
    assert_eq!(h.scheduler.tiers.agent_step(skipped), AgentStep::CarryOnly);
}

#[test]
fn an_inventory_owned_npc_leaves_the_tiers() {
    let mut h = Harness::new();
    let npc = h.npc(5, 22);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    let holder = spawn_at(&mut h.registry, centre(&h.row, 4));
    let mut inventory = postretro_entities::components::inventory::Inventory::default();
    inventory.wieldables[0] = Some(npc);
    h.registry.set_component(holder, inventory).unwrap();
    h.choose();
    assert_eq!(h.tier(npc), None, "held, as the AI skips it");
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
}

#[test]
fn moved_npcs_reclassify_next_tick_and_inherit_nothing() {
    let mut h = Harness::new();
    let npc = h.npc(25, 11);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));
    // A script, mover or knockback move into view: the next tier choice.
    h.place(npc, 6);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));

    // Despawned and its slot reused by an unseen NPC: no hold carried over.
    h.registry.despawn(npc).unwrap();
    let reused = h.npc(25, 12);
    assert_ne!(reused, npc);
    h.choose();
    assert_eq!(h.tier(npc), None);
    assert_eq!(h.tier(reused), Some(NpcTier::Dormant));
}

#[test]
fn always_active_archetype_keeps_unseen_npcs_idle_and_placement_wins() {
    let mut h = Harness::new();
    h.descriptors[0].behavior.as_mut().unwrap().ai.always_active = true;
    let archetype = h.npc(25, 13);
    let forced_off = h.npc(26, 14);
    h.brain(forced_off, |brain| brain.always_active = AlwaysActive::Off);
    for _ in 0..2 * MOVE_DOWN_HOLD_TICKS {
        h.choose();
        assert_eq!(h.tier(archetype), Some(NpcTier::Idle));
        assert_eq!(h.scheduler.tiers.ai_run(archetype), AiRun::Run);
    }
    assert_eq!(h.tier(forced_off), Some(NpcTier::Dormant));

    // A placement forcing it on wins over an archetype without it.
    let mut h = Harness::new();
    let forced_on = h.npc(25, 15);
    h.brain(forced_on, |brain| brain.always_active = AlwaysActive::On);
    h.choose();
    assert_eq!(h.tier(forced_on), Some(NpcTier::Idle));
}

#[test]
fn always_active_reload_moves_up_at_once_and_down_after_the_hold() {
    let mut h = Harness::new();
    let npc = h.npc(25, 16);
    let forced_off = h.npc(26, 17);
    h.brain(forced_off, |brain| brain.always_active = AlwaysActive::Off);
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));

    // A reload commits `alwaysActive: true`: the next tier choice moves it up.
    h.descriptors[0].behavior.as_mut().unwrap().ai.always_active = true;
    h.generation += 1;
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    assert_eq!(h.scheduler.tiers.agent_step(npc), AgentStep::Ticks(1));
    assert_eq!(
        h.tier(forced_off),
        Some(NpcTier::Dormant),
        "a placement key is never overridden"
    );

    // Set false again: it moves down only after the hold.
    h.descriptors[0].behavior.as_mut().unwrap().ai.always_active = false;
    h.generation += 1;
    for _ in 0..MOVE_DOWN_HOLD_TICKS - 1 {
        h.choose();
        assert_eq!(h.tier(npc), Some(NpcTier::Idle));
    }
    h.choose();
    assert_eq!(h.tier(npc), Some(NpcTier::Dormant));
}

#[test]
fn choosing_tiers_allocates_nothing_once_warm() {
    let mut h = Harness::new();
    let npcs: Vec<EntityId> = (0..30).map(|cell| h.npc(cell, cell as u64)).collect();
    h.choose();
    // Warm: NPCs move between cells and tiers.
    for tick in 0..MOVE_DOWN_HOLD_TICKS {
        if tick % 7 == 0 {
            let moved = npcs[(tick as usize) % npcs.len()];
            h.place(moved, (tick as usize * 11) % 30);
        }
        h.choose();
    }
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for _ in 0..MOVE_DOWN_HOLD_TICKS {
        h.choose();
    }
    assert_eq!(probe.allocs_since(), 0);
}
