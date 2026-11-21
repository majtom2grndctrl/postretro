// The AI under the NPC tick tiers: the sim's tier choice picks who runs, and
// the AI credits a skipped brain's clocks once when it next runs. A move-up
// steps one ordinary tick; a dormant NPC decides on every fourth dormant run;
// one that gains a target on its own run is engaged from the next tick. A
// pawn in arena 0 of a synthetic three-arena level perceives that arena;
// arena 2 is unseen.
// See: context/lib/entity_model.md §7c

use super::*;
use postretro_sim::agent_steering::AgentStep;
use postretro_sim::npc_perception::test_support::{Row, centre, spawn_at, three_arenas};
use postretro_sim::npc_perception::{GhostLinkCells, NpcPerception, resolve_ai_lead};
use postretro_sim::npc_tiers::{
    DECIDE_INTERVAL_TICKS, DORMANT_INTERVAL_TICKS, NpcScheduler, NpcTier, TierInputs,
};

const DT: f32 = 1.0 / 60.0;

struct Tiered {
    reg: EntityRegistry,
    runtime: AiRuntime,
    scheduler: NpcScheduler,
    level: Row,
    pawn: EntityId,
    /// A wall between arena 0 and arena 2, standing while `screened`.
    wall: CollisionWorld,
    open: CollisionWorld,
    screened: bool,
    /// Movers at their pose; none unless a test adds a door.
    movers: Vec<crate::collision::moving::MoverCollider>,
    poses: crate::collision::test_movers::DoorPoses,
    ticks: u64,
}

impl Tiered {
    fn new() -> Self {
        Self::with_pawn(spawn_at)
    }

    /// The perceiving pawn is a player NPCs can acquire.
    fn with_player() -> Self {
        Self::with_pawn(spawn_player)
    }

    fn with_pawn(spawn: impl FnOnce(&mut EntityRegistry, Vec3) -> EntityId) -> Self {
        let level = three_arenas();
        let mut reg = EntityRegistry::new();
        let pawn = spawn(&mut reg, centre(&level, 3));
        let lead = resolve_ai_lead(None, level.residency.as_ref().map(|set| set.max_lead));
        Self {
            reg,
            runtime: AiRuntime::new(),
            scheduler: NpcScheduler::new(NpcPerception::new(30, lead, GhostLinkCells::default())),
            level,
            pawn,
            wall: wall_world(30.0, -50.0, 50.0),
            open: CollisionWorld::new(),
            screened: false,
            movers: Vec::new(),
            poses: Default::default(),
            ticks: 0,
        }
    }

    fn npc(&mut self, cell: usize, identity: u64, graph: &BehaviorGraphDescriptor) -> EntityId {
        let mut brain = BrainComponent::from_graph(graph);
        brain.schedule_identity = Some(identity);
        spawn_enemy(&mut self.reg, centre(&self.level, cell), brain, 100.0)
    }

    fn place(&mut self, id: EntityId, cell: usize) {
        let mut transform = *self.reg.get_component::<Transform>(id).unwrap();
        transform.position = centre(&self.level, cell);
        self.reg.set_component(id, transform).unwrap();
    }

    /// One tick: tier choice, the AI under it, then the post-AI settle.
    fn tick(&mut self) {
        let NpcScheduler { perception, tiers } = &mut self.scheduler;
        perception.step(&self.level, &self.reg, [self.pawn]);
        tiers.choose(
            &self.reg,
            perception,
            &TierInputs {
                level: &self.level,
                descriptors: &[],
                descriptor_generation: 0,
            },
        );
        let lookup = |id| tiers.ai_run(id);
        run_ai_tick_with_navigation_and_impact(
            &mut self.reg,
            &mut self.runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav {
                    gate: crate::nav::NavGate::OPEN,
                    movers: &self.movers,
                    poses: &self.poses,
                },
                nav_graph: None,
                collision_world: Some(if self.screened {
                    &self.wall
                } else {
                    &self.open
                }),
                descriptors: &[],
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: None,
                schedule: crate::ai_host::AiSchedule::scheduled(&lookup),
                cpu: None,
            },
            |_| {},
        );
        tiers.settle_after_ai(&self.reg);
        self.ticks += 1;
    }

    fn brain(&self, id: EntityId) -> &BrainComponent {
        self.reg.get_component::<BrainComponent>(id).unwrap()
    }

    fn tier(&self, id: EntityId) -> Option<NpcTier> {
        self.scheduler.tiers.tier(id)
    }
}

/// One hold activity and nothing else: its clock only ever advances.
fn resting_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([(
            "rest".to_string(),
            authored_state("idle", MotionVerb::Hold, None),
        )]),
        transitions: BTreeMap::new(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// `rest` → `hurt` once `@brain.timeSinceDamageMs` is at most 100.
fn hurt_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([
            ("rest".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            ("hurt".to_string(), authored_state("idle", MotionVerb::Hold, None)),
        ]),
        transitions: BTreeMap::from([(
            "rest".to_string(),
            vec![edge(
                "hurt",
                IrNode::Le {
                    a: Box::new(brain_input(BRAIN_TIME_SINCE_DAMAGE_MS_INPUT)),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(100.0),
                    }),
                },
            )],
        )]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

fn activity_clock(tiered: &Tiered, id: EntityId) -> f32 {
    tiered.brain(id).time_in_activity_ms[0]
}

// P3 / P11 / AC 45: an unseen NPC moved up mid-interval runs on that tick,
// its clocks credited by every tick it did not run, once; its steering moves
// one ordinary tick. Its 60 Hz twin, seen throughout, reads the same clock.
#[test]
fn move_up_credits_timers_once_and_steps_once() {
    let graph = resting_graph();
    for into_interval in [1, 7, 16, 29] {
        let mut tiered = Tiered::new();
        let seen = tiered.npc(5, 1, &graph);
        let unseen = tiered.npc(25, 2, &graph);
        // Spawn tick, then dormant runs and skips.
        for _ in 0..2 * DORMANT_INTERVAL_TICKS + into_interval {
            tiered.tick();
        }
        assert_eq!(tiered.tier(unseen), Some(NpcTier::Dormant));
        tiered.place(unseen, 6);
        tiered.tick();
        assert_eq!(tiered.tier(unseen), Some(NpcTier::Idle));
        assert_eq!(
            tiered.scheduler.tiers.agent_step(unseen),
            AgentStep::Ticks(1),
            "a move up never catches up movement"
        );
        let twin = activity_clock(&tiered, seen);
        assert!(
            (activity_clock(&tiered, unseen) - twin).abs() < 0.05,
            "{into_interval} ticks in: dormant {} ms, 60 Hz {twin} ms",
            activity_clock(&tiered, unseen)
        );
        for _ in 0..DORMANT_INTERVAL_TICKS {
            tiered.tick();
        }
        let twin = activity_clock(&tiered, seen);
        assert!(
            (activity_clock(&tiered, unseen) - twin).abs() < 0.05,
            "credited twice: dormant {} ms, 60 Hz {twin} ms",
            activity_clock(&tiered, unseen)
        );
    }
}

// P4: a dormant NPC decides on every fourth dormant run and on no other tick.
#[test]
fn a_dormant_npc_decides_on_every_fourth_dormant_run_only() {
    let graph = super::decide::toggling_graph();
    let mut tiered = Tiered::new();
    let npc = tiered.npc(25, 3, &graph);
    tiered.tick();
    let mut previous = tiered.brain(npc).active_activity_path;
    let mut decides = 0;
    for _ in 0..8 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
        let run = tiered.scheduler.tiers.ai_run(npc);
        let path = tiered.brain(npc).active_activity_path;
        let flipped = path != previous;
        previous = path;
        let deciding = run == crate::ai_host::AiRun::RunDormant { decide: true };
        assert_eq!(
            flipped, deciding,
            "a dormant decide flips; nothing else does"
        );
        decides += usize::from(deciding);
    }
    assert_eq!(decides, 2, "two decides in 8 dormant runs");
}

// AC 10: a dormant NPC hurt by contextless damage (a trap, a script) moves up
// and reads the hit in its guards at its next decide, never aged by the time
// it spent dormant before the hit.
#[test]
fn idle_npc_reads_contextless_damage_at_next_decide() {
    let graph = hurt_graph();
    for into_interval in [3, 17, 28] {
        let mut tiered = Tiered::new();
        let npc = tiered.npc(25, 4, &graph);
        for _ in 0..DORMANT_INTERVAL_TICKS + into_interval {
            tiered.tick();
        }
        assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
        // The damage chokepoint zeroes the clock, as a trap's hit does.
        let mut brain = tiered.brain(npc).clone();
        brain.time_since_damage_ms = 0.0;
        tiered.reg.set_component(npc, brain).unwrap();
        let mut decided_within = None;
        for tick in 1..=DECIDE_INTERVAL_TICKS + 1 {
            tiered.tick();
            if tiered.brain(npc).active_path_names() == ["hurt"] {
                decided_within = Some(tick);
                break;
            }
        }
        assert!(
            decided_within.is_some(),
            "{into_interval} ticks in: the hit was not read within a decide interval"
        );
        assert_eq!(tiered.tier(npc), Some(NpcTier::Idle));
    }
}

// P39 / AC 28: an unseen `alwaysActive` NPC runs exactly as its seen twin,
// every tick: the same AI run and steering step, the same decides, so a walk
// already under way crosses a trigger on the same tick as when perceived.
#[test]
fn unseen_always_active_npc_runs_every_tick_as_its_seen_twin() {
    let graph = super::decide::toggling_graph();
    let mut tiered = Tiered::new();
    let seen = tiered.npc(5, 9, &graph);
    let unseen = tiered.npc(25, 9, &graph);
    let mut brain = tiered.brain(unseen).clone();
    brain.always_active = postretro_entities::components::brain::AlwaysActive::On;
    tiered.reg.set_component(unseen, brain).unwrap();
    for _ in 0..4 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
        let tiers = &tiered.scheduler.tiers;
        assert_eq!(tiers.tier(unseen), Some(NpcTier::Idle));
        assert_eq!(tiers.ai_run(unseen), tiers.ai_run(seen));
        assert_eq!(tiers.agent_step(unseen), tiers.agent_step(seen));
        assert_eq!(
            tiered.brain(unseen).active_activity_path,
            tiered.brain(seen).active_activity_path
        );
        assert_eq!(
            tiered.brain(unseen).time_in_activity_ms,
            tiered.brain(seen).time_in_activity_ms
        );
    }
}

// ---------------------------------------------------------------------------
// Awareness under the tiers (AC 9, 16, 49; P20, P44, P57, P61)
// ---------------------------------------------------------------------------

fn dormant_check(tiered: &Tiered, id: EntityId) -> bool {
    tiered.scheduler.tiers.ai_run(id) == crate::ai_host::AiRun::RunDormant { decide: true }
}

fn meter(tiered: &Tiered, id: EntityId) -> f32 {
    tiered.brain(id).awareness.value(tiered.pawn)
}

/// Tick until `id`'s next dormant check has run; its tick.
fn to_dormant_check(tiered: &mut Tiered, id: EntityId) -> u64 {
    for _ in 0..8 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
        if dormant_check(tiered, id) {
            return tiered.ticks;
        }
    }
    panic!("no dormant check");
}

/// Whether `id`, deciding at 15 Hz on `identity`, checks on `tick`.
fn idle_check(identity: u64, tick: u64) -> bool {
    postretro_sim::npc_tiers::decides_on(tick, identity, DECIDE_INTERVAL_TICKS)
}

// P20 / AC 49: an unperceived NPC whose candidacy admits a player, with line
// of sight to them from outside the player's set, notices them through its
// meter on its dormant checks: a first sighting gains nothing, so with the
// default `fillMs` the second dormant check that sees them acquires; then it
// moves up.
#[test]
fn unperceived_npc_with_sight_notices_on_dormant_checks() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut tiered = Tiered::with_player();
    tiered.screened = true;
    let npc = tiered.npc(25, 5, &graph);
    for _ in 0..3 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
    }
    assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
    assert!(
        tiered.brain(npc).awareness.is_empty(),
        "screened, unmetered"
    );
    tiered.screened = false;
    let first = to_dormant_check(&mut tiered, npc);
    assert_eq!(meter(&tiered, npc), 0.0, "a first sighting gains nothing");
    assert_eq!(tiered.brain(npc).acquired_target, None);
    assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
    let second = to_dormant_check(&mut tiered, npc);
    assert!(second - first >= 60, "a dormant check every 2 s");
    assert_eq!(
        tiered.brain(npc).acquired_target,
        Some(tiered.pawn),
        "the second dormant check that sees them"
    );
    assert_eq!(tiered.tier(npc), Some(NpcTier::Engaged), "then it moves up");
}

// A dormant NPC's checks see a closed door as they see a wall: no sighting
// while it stands, then the second dormant check after it opens acquires.
#[test]
fn dormant_checks_do_not_see_through_a_closed_door() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut tiered = Tiered::with_player();
    tiered.movers = vec![crate::collision::test_movers::door_slab(
        5, 0.1, 100.0, 50.0,
    )];
    let door_at = |y: f32| {
        crate::collision::test_movers::DoorPoses::default().place(
            5,
            Vec3::new(30.0, y, 0.0),
            glam::Quat::IDENTITY,
        )
    };
    tiered.poses = door_at(-50.0);
    let npc = tiered.npc(25, 5, &graph);
    for _ in 0..3 {
        to_dormant_check(&mut tiered, npc);
        assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
        assert!(
            tiered.brain(npc).awareness.is_empty(),
            "the door hides them"
        );
    }
    tiered.poses = door_at(200.0);
    to_dormant_check(&mut tiered, npc);
    assert_eq!(meter(&tiered, npc), 0.0, "a first sighting gains nothing");
    to_dormant_check(&mut tiered, npc);
    assert_eq!(tiered.brain(npc).acquired_target, Some(tiered.pawn));
}

// P44 / AC 9: a dormant NPC with `fillMs` 3000 sights a player on a dormant
// check and moves up 0.5 s later: moving up adds nothing to the meter, and
// it acquires on the first check at least 3 s after that dormant check.
#[test]
fn move_up_adds_nothing_to_a_meter() {
    let graph = super::awareness::noticing_graph(3_000.0, 2_000.0);
    let identity = 6;
    let mut tiered = Tiered::with_player();
    tiered.screened = true;
    let npc = tiered.npc(25, identity, &graph);
    for _ in 0..3 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
    }
    tiered.screened = false;
    let sighted = to_dormant_check(&mut tiered, npc);
    assert_eq!(meter(&tiered, npc), 0.0);
    for _ in 0..30 {
        tiered.tick();
    }
    // Move up off a check: the first idle check after it gains the time since
    // the dormant check (P44), which would hide whether the move-up added any.
    while idle_check(identity, tiered.ticks + 1) {
        tiered.tick();
    }
    tiered.place(npc, 8);
    tiered.tick();
    assert_eq!(tiered.tier(npc), Some(NpcTier::Idle));
    assert_eq!(meter(&tiered, npc), 0.0, "moving up adds nothing");
    let mut acquired = None;
    for _ in 0..240 {
        tiered.tick();
        if tiered.brain(npc).acquired_target.is_some() {
            acquired = Some(tiered.ticks);
            break;
        }
    }
    let acquired = acquired.expect("noticed");
    assert!(idle_check(identity, acquired), "on a check");
    assert!(
        acquired >= sighted + 180,
        "no sooner than 3 s after the sighting"
    );
    assert!(
        acquired - DECIDE_INTERVAL_TICKS < sighted + 180,
        "the first check at least 3 s after the sighting"
    );
}

/// A dormant NPC hit from out of sight about 1.9 s after its last dormant
/// check: the hit's tick, its last dormant check, and the tiered world.
fn grenade_on_a_dormant_npc(identity: u64) -> (Tiered, EntityId, u64) {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut tiered = Tiered::with_player();
    tiered.screened = true;
    let npc = tiered.npc(25, identity, &graph);
    for _ in 0..3 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
    }
    let last = to_dormant_check(&mut tiered, npc);
    // About 1.9 s later, after a tick its idle phase checks on, so later
    // checks fall a whole number of intervals after the hit.
    while tiered.ticks < last + 114 || !idle_check(identity, tiered.ticks) {
        tiered.tick();
        assert!(!dormant_check(&tiered, npc), "no check before the hit");
    }
    let mut context = DamageContext::new("test.grenade", DamageProducer::InTick);
    context.attacker = Some(tiered.pawn);
    assert!(apply_damage_with_context(
        &mut tiered.reg,
        npc,
        &DamagePayload {
            amount: 1.0,
            impulse: glam::Vec3::ZERO,
            damage_type: None,
        },
        context,
    ));
    (tiered, npc, last)
}

// P57 / AC 16: the thrower's meter drains from the hit's tick, not from the
// NPC's last dormant check; it holds near 1 at the first idle check.
// Reappearing 0.1 s after the hit, the thrower is acquired on the second check
// in a row that sees them; reappearing on the first check after a last missed
// check 1 s after the hit (meter 0.5), on the first check at least 0.5 s after
// that first sighting.
#[test]
fn dormant_hit_drains_from_the_hit_tick() {
    let identity = 7;
    // The first idle check: near 1, drained from the hit.
    let (mut tiered, npc, last) = grenade_on_a_dormant_npc(identity);
    let hit = tiered.ticks;
    assert_eq!(meter(&tiered, npc), 1.0);
    tiered.tick();
    while !idle_check(identity, tiered.ticks) {
        tiered.tick();
    }
    assert_eq!(tiered.tier(npc), Some(NpcTier::Idle), "the hit moved it up");
    let drained = 1.0 - (tiered.ticks - hit) as f32 / 120.0;
    assert!(
        (meter(&tiered, npc) - drained).abs() < 1.0e-3,
        "drained from the hit: {} != {drained} (from the last check: {})",
        meter(&tiered, npc),
        1.0 - (tiered.ticks - last) as f32 / 120.0
    );

    // Back in view 0.1 s after the hit: the second check in a row.
    let (mut tiered, npc, _) = grenade_on_a_dormant_npc(identity);
    let hit = tiered.ticks;
    while tiered.ticks < hit + 6 {
        tiered.tick();
    }
    tiered.screened = false;
    let mut sightings = 0;
    loop {
        tiered.tick();
        if idle_check(identity, tiered.ticks) {
            sightings += 1;
        }
        if tiered.brain(npc).acquired_target.is_some() {
            break;
        }
        assert!(sightings < 2, "not acquired on the second sighting");
    }
    assert_eq!(sightings, 2, "acquired on the second check in a row");

    // Back in view after a last missed check 1 s after the hit.
    let (mut tiered, npc, _) = grenade_on_a_dormant_npc(identity);
    let hit = tiered.ticks;
    while tiered.ticks < hit + 60 {
        tiered.tick();
    }
    assert!(
        idle_check(identity, tiered.ticks),
        "a check 1 s after the hit"
    );
    assert!((meter(&tiered, npc) - 0.5).abs() < 1.0e-3, "0.5 at it");
    tiered.screened = false;
    tiered.tick();
    while !idle_check(identity, tiered.ticks) {
        tiered.tick();
    }
    let first_sighting = tiered.ticks;
    assert!(
        (meter(&tiered, npc) - 0.5).abs() < 1.0e-3,
        "no gain, no drain"
    );
    loop {
        tiered.tick();
        if tiered.brain(npc).acquired_target.is_some() {
            break;
        }
    }
    let acquired = tiered.ticks;
    assert!(acquired >= first_sighting + 30, "at least 0.5 s after");
    assert!(
        acquired < first_sighting + 30 + DECIDE_INTERVAL_TICKS,
        "on the first check at least 0.5 s after"
    );
}

// P61 / AC 46 (meter credit): an NPC moving down is credited, at its first
// dormant check, by the time since its last idle check, once.
#[test]
fn move_down_credits_meters_once_by_the_time_since_the_last_idle_check() {
    let graph = super::awareness::noticing_graph(60_000.0, 2_000.0);
    let identity = 8;
    let mut tiered = Tiered::with_player();
    let npc = tiered.npc(6, identity, &graph);
    for _ in 0..20 {
        tiered.tick();
    }
    tiered.place(npc, 25);
    let mut last_idle = (0, 0.0);
    while tiered.tier(npc) != Some(NpcTier::Dormant) {
        tiered.tick();
        if tiered.tier(npc) == Some(NpcTier::Idle) && idle_check(identity, tiered.ticks) {
            last_idle = (tiered.ticks, meter(&tiered, npc));
        }
        assert!(tiered.ticks < 400, "moved down after the hold");
    }
    assert!(last_idle.1 > 0.0, "the meter was filling while idle");
    let first_dormant = to_dormant_check(&mut tiered, npc);
    let expected = last_idle.1 + (first_dormant - last_idle.0) as f32 / 3_600.0;
    assert!(
        (meter(&tiered, npc) - expected).abs() < 1.0e-4,
        "credited once: {} != {expected}",
        meter(&tiered, npc)
    );
}

// P47: the respawn drop is a stored difference, so a dormant NPC that sighted
// a player on its last check forgets that sighting at its next run after the
// respawn: its next dormant check is a first sighting again, where without
// the respawn it would acquire.
#[test]
fn a_dormant_npc_runs_the_respawn_drop_at_its_next_run() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut tiered = Tiered::with_player();
    tiered.screened = true;
    let npc = tiered.npc(25, 5, &graph);
    for _ in 0..3 * DORMANT_INTERVAL_TICKS {
        tiered.tick();
    }
    tiered.screened = false;
    to_dormant_check(&mut tiered, npc);
    assert!(tiered.brain(npc).awareness.meter(tiered.pawn).is_some());
    for _ in 0..10 {
        tiered.tick();
    }
    assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
    let pawn = tiered.pawn;
    respawn_in_place(&mut tiered.reg, pawn);
    to_dormant_check(&mut tiered, npc);
    assert_eq!(
        tiered.brain(npc).acquired_target,
        None,
        "a first sighting again"
    );
    assert_eq!(meter(&tiered, npc), 0.0);
    assert_eq!(tiered.tier(npc), Some(NpcTier::Dormant));
}
