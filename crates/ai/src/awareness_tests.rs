// Awareness meters at production cadence: an NPC without a target notices each
// candidate through its own meter, filled and drained at its 15 Hz acquisition
// checks; hits, the aggro gate, target loss and switches set or clear meters.
// A screen at x = 5 hides whatever stands behind it from an NPC at the origin.
// See: context/plans/in-progress/E10--ai-tick-scheduling/index.md (An NPC
//      notices through a per-candidate meter; P31–P37, P41, P43, P53, P55,
//      P57, P58, P63, P65)
//      context/lib/entity_model.md §7c

use super::*;
use crate::decide::DECIDE_INTERVAL_TICKS;
use postretro_entities::components::brain::AWARENESS_METER_CAPACITY;
use postretro_foundation::{AwarenessDescriptor, BRAIN_AWARENESS_INPUT};

const DT: f32 = 1.0 / 60.0;
/// Milliseconds between two acquisition checks in view.
const CHECK_MS: f32 = DECIDE_INTERVAL_TICKS as f32 * 1_000.0 / 60.0;
/// Checks a default 1000 ms fill takes after a first sighting.
const FILL_CHECKS: usize = 15;
const HOSTILE_PEER_FACTION: f32 = 7.0;

/// Behind the screen from an NPC at the origin.
fn hidden(x: f32) -> Vec3 {
    Vec3::new(x, 0.0, 0.0)
}

/// In plain view of an NPC at the origin, `distance` away.
fn in_view(distance: f32) -> Vec3 {
    Vec3::new(0.0, 0.0, distance)
}

/// A wall at x = 5, z in [-3, 3]: it hides what stands behind it on the x
/// axis from an NPC at the origin, and nothing along z.
fn screen() -> CollisionWorld {
    let points = vec![
        Vec3::new(5.0, -50.0, -3.0),
        Vec3::new(5.0, 50.0, -3.0),
        Vec3::new(5.0, 50.0, 3.0),
        Vec3::new(5.0, -50.0, 3.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0u32, 1, 2], [0, 2, 3]])
}

/// `watch` holds without engaging and moves to `chase` once it has a target;
/// `chase` engages and drops its target back to `watch` when it dies. Dead
/// candidates fail candidacy.
pub(super) fn noticing_graph(fill_ms: f32, drain_ms: f32) -> BehaviorGraphDescriptor {
    let mut graph = test_behavior_graph!({
        initial: "watch".to_string(),
        activities: BTreeMap::from([
            ("watch".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            (
                "chase".to_string(),
                authored_state("walk", MotionVerb::ChaseTarget, None),
            ),
        ]),
        transitions: BTreeMap::from([
            (
                "watch".to_string(),
                vec![edge("chase", brain_input(BRAIN_HAS_TARGET_INPUT))],
            ),
            (
                "chase".to_string(),
                vec![edge("watch", brain_input(BRAIN_TARGET_DIED_INPUT))],
            ),
        ]),
        candidate_filter: Some(candidate_is_alive_within(1_000.0)),
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    });
    graph.ai.awareness = Some(AwarenessDescriptor { fill_ms, drain_ms });
    graph
}

fn default_noticing() -> BehaviorGraphDescriptor {
    noticing_graph(
        AwarenessDescriptor::DEFAULT_FILL_MS,
        AwarenessDescriptor::DEFAULT_DRAIN_MS,
    )
}

/// One activity that holds and never engages: a resting NPC keeps no target
/// between checks and selects one on each check.
fn resting_graph() -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: "rest".to_string(),
        activities: BTreeMap::from([(
            "rest".to_string(),
            authored_state("idle", MotionVerb::Hold, None),
        )]),
        transitions: BTreeMap::new(),
        candidate_filter: Some(candidate_is_alive_within(1_000.0)),
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

fn number(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

fn awareness_at_least(value: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(BRAIN_AWARENESS_INPUT)),
        b: Box::new(number(value)),
    }
}

/// An NPC at the origin, a production-cadence runtime, and a screen it can
/// stand behind or not, tick by tick.
struct Watch {
    reg: EntityRegistry,
    runtime: AiRuntime,
    screen: CollisionWorld,
    ticks: u64,
}

impl Watch {
    fn new() -> Self {
        Self {
            reg: EntityRegistry::new(),
            runtime: AiRuntime::new(),
            screen: screen(),
            ticks: 0,
        }
    }

    /// An NPC at `position` deciding on the first tick and every fourth after.
    fn npc_at(&mut self, graph: &BehaviorGraphDescriptor, position: Vec3) -> EntityId {
        let mut brain = BrainComponent::from_graph(graph);
        brain.schedule_identity = Some(identity_deciding_on(1));
        spawn_enemy(&mut self.reg, position, brain, 1_000.0)
    }

    fn npc(&mut self, graph: &BehaviorGraphDescriptor) -> EntityId {
        self.npc_at(graph, Vec3::ZERO)
    }

    fn player(&mut self, position: Vec3) -> EntityId {
        spawn_player(&mut self.reg, position)
    }

    /// A brain-bearing pawn of a faction hostile to the NPCs.
    fn hostile_peer(&mut self, position: Vec3) -> EntityId {
        let peer = spawn_enemy(
            &mut self.reg,
            position,
            BrainComponent::from_graph(&resting_graph()),
            50.0,
        );
        self.reg
            .entity_state_mut(peer)
            .unwrap()
            .set(FACTION_STATE_FIELD, HOSTILE_PEER_FACTION);
        peer
    }

    fn place(&mut self, entity: EntityId, position: Vec3) {
        set_enemy_position(&mut self.reg, entity, position);
    }

    /// One tick, with the screen standing.
    fn tick(&mut self) {
        run_ai_tick_with_navigation(
            &mut self.reg,
            &mut self.runtime,
            DT,
            None,
            Some(&self.screen),
        );
        self.ticks += 1;
    }

    /// One tick with nothing in the way.
    fn tick_clear(&mut self) {
        run_ai_tick_with_navigation(&mut self.reg, &mut self.runtime, DT, None, None);
        self.ticks += 1;
    }

    /// Run to and through the NPCs' next acquisition check, with the screen
    /// standing (`screened`) or not on every tick of it.
    fn check_with(&mut self, screened: bool) {
        loop {
            if screened {
                self.tick();
            } else {
                self.tick_clear();
            }
            if self.ticks % DECIDE_INTERVAL_TICKS == 1 {
                break;
            }
        }
    }

    /// The next check, positions as they stand behind the screen.
    fn check(&mut self) {
        self.check_with(true);
    }

    fn checks(&mut self, count: usize) {
        for _ in 0..count {
            self.check();
        }
    }

    fn brain(&self, npc: EntityId) -> &BrainComponent {
        self.reg.get_component::<BrainComponent>(npc).unwrap()
    }

    fn brain_mut(&mut self, npc: EntityId, edit: impl FnOnce(&mut BrainComponent)) {
        let mut brain = self.brain(npc).clone();
        edit(&mut brain);
        self.reg.set_component(npc, brain).unwrap();
    }

    fn value(&self, npc: EntityId, candidate: EntityId) -> f32 {
        self.brain(npc).awareness.value(candidate)
    }

    fn metered(&self, npc: EntityId, candidate: EntityId) -> bool {
        self.brain(npc).awareness.meter(candidate).is_some()
    }

    fn target(&self, npc: EntityId) -> Option<EntityId> {
        self.brain(npc).acquired_target
    }

    /// What a resting NPC selected on its last check.
    fn selected(&self, npc: EntityId) -> Option<EntityId> {
        self.brain(npc).awareness.selected
    }

    fn state(&self, npc: EntityId) -> String {
        enemy_state_name(&self.reg, npc)
    }

    fn hit(&mut self, npc: EntityId, attacker: Option<EntityId>) {
        let mut context = DamageContext::new("test.awareness", DamageProducer::InTick);
        context.attacker = attacker;
        assert!(apply_damage_with_context(
            &mut self.reg,
            npc,
            &DamagePayload {
                amount: 1.0,
                impulse: Vec3::ZERO,
                damage_type: None,
            },
            context,
        ));
    }

    /// Close or open the aggro gate as the `updateNpcState` reaction does.
    fn aggro(&mut self, npc: EntityId, armed: bool) {
        self.brain_mut(npc, |brain| brain.set_aggro_armed(armed));
    }

    /// Kill `pawn`: the death sweep's latch, which candidacy and
    /// `@brain.targetDied` read.
    fn kill(&mut self, pawn: EntityId) {
        let mut health = self
            .reg
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .clone();
        health.current = 0.0;
        health.death_handled = true;
        self.reg.set_component(pawn, health).unwrap();
    }

    /// Checks until `npc` targets someone, at most `limit`; the count run.
    fn checks_until_target(&mut self, npc: EntityId, limit: usize) -> Option<usize> {
        (1..=limit).find(|_| {
            self.check();
            self.target(npc).is_some()
        })
    }
}

fn assert_near(actual: f32, expected: f32, message: &str) {
    assert!(
        (actual - expected).abs() < 1.0e-3,
        "{message}: {actual} != {expected}"
    );
}

// ---------------------------------------------------------------------------
// Fill and drain (AC 5, P31)
// ---------------------------------------------------------------------------

// A player seen on every check is acquired on the first check at least
// `fillMs` after the first check that saw them, and on no earlier one: a first
// sighting gains nothing.
#[test]
fn continuous_sight_acquires_at_fill() {
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&default_noticing());
    watch.check();
    assert_eq!(
        watch.value(npc, player),
        0.0,
        "a first sighting gains nothing"
    );
    for check in 1..FILL_CHECKS {
        watch.check();
        assert_eq!(watch.target(npc), None, "check {check}");
        assert_near(
            watch.value(npc, player),
            check as f32 * CHECK_MS / 1_000.0,
            &format!("check {check}"),
        );
    }
    watch.check();
    assert_eq!(watch.target(npc), Some(player), "1 s after the first check");
    assert_eq!(watch.state(npc), "chase");
}

// Seen 0.6 s, lost just after, regained 0.5 s later: the meter drains from the
// last check that saw them, and refills from the first check that sees them
// again, acquiring at the first check 0.65 s or more after it.
#[test]
fn lost_and_regained_sight_resumes_from_drained_value() {
    let mut watch = Watch::new();
    let player = watch.player(hidden(10.0));
    let npc = watch.npc(&default_noticing());
    // Checks 0..=9 see them: 0.6 at the check 0.6 s after the first.
    for _ in 0..10 {
        watch.check_with(false);
    }
    assert_near(watch.value(npc, player), 0.6, "0.6 s filled");
    // Lost just after; regained 0.5 s later, at the first check 1.1 s or
    // more after the first: checks 10..=16 miss them.
    for _ in 10..17 {
        watch.check();
    }
    watch.check_with(false);
    let drained = 0.6 - 7.0 * CHECK_MS / 2_000.0;
    assert_near(
        watch.value(npc, player),
        drained,
        "drained from the last check that saw them; a first sighting gains nothing",
    );
    let needed_checks = ((1.0 - drained) * 1_000.0 / CHECK_MS).ceil() as usize;
    assert!(needed_checks as f32 * CHECK_MS >= 650.0);
    for check in 1..needed_checks {
        watch.check_with(false);
        assert_eq!(watch.target(npc), None, "{check} checks after regaining");
    }
    watch.check_with(false);
    assert_eq!(watch.target(npc), Some(player));
    assert!(
        (needed_checks - 1) as f32 * CHECK_MS < 650.0,
        "never on a check before 0.65 s"
    );
}

// A player teleporting into view of an idle NPC is noticed no sooner than
// `fillMs` after the first check that sees them.
#[test]
fn teleport_into_view_waits_a_full_fill() {
    let mut watch = Watch::new();
    let player = watch.player(hidden(30.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(10);
    assert!(!watch.metered(npc, player), "unseen, unmetered");
    watch.place(player, in_view(3.0));
    watch.check();
    assert_eq!(watch.value(npc, player), 0.0);
    for check in 1..FILL_CHECKS {
        watch.check();
        assert_eq!(watch.target(npc), None, "check {check} after the teleport");
    }
    watch.check();
    assert_eq!(watch.target(npc), Some(player));
}

// Two players: the one in view fills its own meter; the other, stepping into
// view later, starts from zero.
#[test]
fn per_player_meters_are_independent() {
    let mut watch = Watch::new();
    let seen = watch.player(in_view(10.0));
    let unseen = watch.player(hidden(10.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(10);
    assert!(
        !watch.metered(npc, unseen),
        "only the seen player is metered"
    );
    assert_near(watch.value(npc, seen), 9.0 * CHECK_MS / 1_000.0, "seen");
    watch.place(unseen, Vec3::new(10.0, 0.0, -20.0));
    watch.check();
    assert_eq!(
        watch.value(npc, unseen),
        0.0,
        "the later one starts from zero"
    );
    watch.checks(4);
    assert_near(
        watch.value(npc, unseen),
        4.0 * CHECK_MS / 1_000.0,
        "its own fill",
    );
    assert_eq!(watch.target(npc), None);
    watch.check();
    assert_eq!(watch.target(npc), Some(seen), "the first full meter");
    assert!(
        !watch.metered(npc, unseen),
        "acquiring clears every other meter"
    );
}

// ---------------------------------------------------------------------------
// Hits (AC 6, P57)
// ---------------------------------------------------------------------------

// A hit from a player in view is acquired on the next decide, whatever its
// meter.
#[test]
fn hit_in_view_acquires_on_the_next_decide() {
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&default_noticing());
    watch.check();
    watch.tick();
    watch.hit(npc, Some(player));
    assert_eq!(watch.value(npc, player), 1.0);
    watch.check();
    assert_eq!(watch.target(npc), Some(player));
}

// Hit from out of sight two ticks after a check: the meter drains from the
// hit's tick, never the previous check; it refills from the attacker's second
// sighting in a row and acquires them once back at 1.
#[test]
fn hit_out_of_sight_drains_from_the_hit_and_refills_from_the_second_sighting() {
    let mut watch = Watch::new();
    let player = watch.player(hidden(10.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(2);
    watch.tick();
    watch.tick();
    watch.hit(npc, Some(player));
    watch.tick();
    watch.tick();
    assert_eq!(watch.ticks % DECIDE_INTERVAL_TICKS, 1, "that tick checked");
    let drained = 1.0 - 2.0 * (1_000.0 / 60.0) / 2_000.0;
    assert_near(
        watch.value(npc, player),
        drained,
        "two ticks of drain since the hit",
    );
    watch.check_with(false);
    assert_eq!(watch.target(npc), None, "a first sighting gains nothing");
    assert_near(watch.value(npc, player), drained, "no gain, no drain");
    watch.check_with(false);
    assert_eq!(
        watch.target(npc),
        Some(player),
        "the second sighting in a row"
    );
}

// Trap and contextless script damage carry no attacker and change no meter;
// the hit still resets damage recency.
#[test]
fn hit_with_no_attacker_changes_no_meter() {
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(3);
    let before = watch.brain(npc).awareness;
    watch.hit(npc, None);
    assert_eq!(watch.brain(npc).awareness, before);
    assert_eq!(watch.brain(npc).time_since_damage_ms, 0.0);
    watch.check();
    assert_eq!(watch.target(npc), None);
    assert_near(
        watch.value(npc, player),
        3.0 * CHECK_MS / 1_000.0,
        "the fill runs on unchanged",
    );
}

// An NPC with a target ranks and switches as today, without a meter, and a
// hit on it changes no meter.
#[test]
fn hit_on_a_targeted_npc_changes_no_meter_and_it_switches_by_ranking() {
    let mut watch = Watch::new();
    let first = watch.player(in_view(20.0));
    let nearer = watch.player(hidden(30.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(FILL_CHECKS + 1);
    assert_eq!(watch.target(npc), Some(first));
    watch.hit(npc, Some(nearer));
    assert!(
        !watch.metered(npc, nearer),
        "a targeted NPC changes no meter"
    );
    watch.place(nearer, Vec3::new(4.0, 0.0, -4.0));
    watch.check();
    assert_eq!(
        watch.target(npc),
        Some(nearer),
        "switched on the decide it came into view, by ranking"
    );
}

// ---------------------------------------------------------------------------
// The aggro gate (AC 7, P34, P41; AC 15, P63)
// ---------------------------------------------------------------------------

/// Fill to `meter` over `fill_ms`, close the gate, hold it closed
/// `closed_ticks` ticks, reopen, and count the checks from the first one after
/// reopening to acquisition.
fn checks_to_acquire_after_the_gate(fill_ms: f32, meter: f32, closed_ticks: usize) -> usize {
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&noticing_graph(fill_ms, 2_000.0));
    watch.check();
    while watch.value(npc, player) < meter - 1.0e-3 {
        watch.check();
    }
    assert_near(watch.value(npc, player), meter, "the meter at the close");
    watch.aggro(npc, false);
    assert!(
        watch.brain(npc).awareness.is_empty(),
        "closing the gate empties the meters"
    );
    for _ in 0..closed_ticks {
        watch.tick();
    }
    // Reopen just before a check tick: the next check is the first after it.
    while !watch.ticks.is_multiple_of(DECIDE_INTERVAL_TICKS) {
        watch.tick();
    }
    watch.aggro(npc, true);
    watch.check();
    assert_eq!(
        watch.value(npc, player),
        0.0,
        "a first sighting after the gate"
    );
    watch.checks_until_target(npc, 40).expect("noticed again") + 1
}

// Seen on the last check before the gate closed and on the first after it
// reopens, minutes later or within one tick: a full fill from that check.
// A meter at 0.9 when the gate closes starts from zero.
#[test]
fn aggro_gate_close_and_reopen_needs_a_full_fill() {
    let fill = AwarenessDescriptor::DEFAULT_FILL_MS;
    let mid = 7.0 * CHECK_MS / fill;
    // 60 s closed.
    assert_eq!(
        checks_to_acquire_after_the_gate(fill, mid, 3_600),
        FILL_CHECKS + 1,
        "60 s closed"
    );
    // Closed and reopened between two ticks.
    assert_eq!(
        checks_to_acquire_after_the_gate(fill, mid, 0),
        FILL_CHECKS + 1,
        "within one tick"
    );
    // 0.9 at the close: thirteen checks of a fill that reads 0.9 there.
    let fill = 13.0 * CHECK_MS / 0.9;
    assert_eq!(
        checks_to_acquire_after_the_gate(fill, 0.9, 0),
        (fill / CHECK_MS).ceil() as usize + 1,
        "0.9 at the close starts from zero"
    );
}

// The NPC had a target when the gate closed: after it reopens, a full fill.
#[test]
fn aggro_gate_closed_on_a_targeted_npc_needs_a_full_fill() {
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(FILL_CHECKS + 1);
    assert_eq!(watch.target(npc), Some(player));
    watch.aggro(npc, false);
    watch.checks(3);
    assert_eq!(watch.target(npc), None, "a closed gate drops the target");
    watch.aggro(npc, true);
    // The closed gate reseated its path, so it decides on its first tick
    // after the reopen: the first check, a first sighting.
    watch.tick();
    let first_check = watch.ticks;
    assert_eq!(watch.target(npc), None, "no instant re-acquire");
    assert_eq!(watch.value(npc, player), 0.0);
    // Acquired on the first check a full fill (60 ticks) after it.
    loop {
        watch.check();
        if watch.ticks >= first_check + 60 {
            break;
        }
        assert_eq!(watch.target(npc), None, "tick {}", watch.ticks);
    }
    assert_eq!(watch.target(npc), Some(player));
}

// A hit between the close and the NPC's next run finds it still holding its
// target and changes no meter; after that run clears it, a hit stands at 1
// until the first check after the gate reopens; a close after a hit empties
// its meter.
#[test]
fn hit_against_closed_gate_is_judged_against_the_target_held() {
    let mut watch = Watch::new();
    let target = watch.player(in_view(10.0));
    let attacker = watch.player(hidden(40.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(FILL_CHECKS + 1);
    assert_eq!(watch.target(npc), Some(target));

    watch.aggro(npc, false);
    watch.hit(npc, Some(attacker));
    assert!(
        watch.brain(npc).awareness.is_empty(),
        "still holding its target: no meter"
    );

    watch.tick();
    assert_eq!(watch.target(npc), None, "the run cleared it");
    watch.hit(npc, Some(attacker));
    assert_eq!(watch.value(npc, attacker), 1.0);
    for _ in 0..600 {
        watch.tick();
    }
    assert_eq!(watch.value(npc, attacker), 1.0, "it stands while closed");
    // Nearer than the old target, so the ranking an engaged NPC runs keeps it.
    watch.place(attacker, in_view(5.0));
    watch.aggro(npc, true);
    watch.check();
    assert_eq!(
        watch.target(npc),
        Some(attacker),
        "the first check after the reopen acquires it"
    );

    // A close after a hit empties it.
    let mut watch = Watch::new();
    let attacker = watch.player(hidden(10.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(2);
    watch.hit(npc, Some(attacker));
    assert_eq!(watch.value(npc, attacker), 1.0);
    watch.aggro(npc, false);
    assert!(watch.brain(npc).awareness.is_empty());
}

// ---------------------------------------------------------------------------
// The bound (AC 8, P43; AC 24, P32, P33; AC 25, P31)
// ---------------------------------------------------------------------------

// Eight nearer players in view fill the meters; a ninth, farther, hits the NPC
// before any of the eight reaches 1: it takes a slot at 1 and is acquired on
// the next decide.
#[test]
fn a_hit_from_an_unmetered_ninth_takes_a_slot_at_one() {
    let mut watch = Watch::new();
    let eight: Vec<EntityId> = (0..8)
        .map(|index| watch.player(in_view(5.0 + index as f32)))
        .collect();
    let ninth = watch.player(in_view(30.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(5);
    assert!(eight.iter().all(|&player| watch.metered(npc, player)));
    assert!(!watch.metered(npc, ninth), "the farthest is left unmetered");
    watch.tick();
    watch.hit(npc, Some(ninth));
    assert_eq!(watch.value(npc, ninth), 1.0);
    assert!(
        !watch.metered(npc, eight[7]),
        "the lowest, farthest member made room"
    );
    watch.check();
    assert_eq!(watch.target(npc), Some(ninth));
}

/// Checks from a player stepping into view until the NPC acquires them, or
/// a resting one selects them.
fn checks_to_notice(watch: &mut Watch, npc: EntityId, player: EntityId) -> usize {
    watch.place(player, in_view(20.0));
    (1..=40)
        .find(|_| {
            watch.check();
            watch.target(npc) == Some(player) || watch.selected(npc) == Some(player)
        })
        .expect("noticed")
}

// Eight meters held by hostile NPCs that die and despawn free their slots:
// a player stepping into view is noticed as fast as with empty meters.
#[test]
fn departed_members_free_their_slots() {
    let mut control = Watch::new();
    let player = control.player(hidden(40.0));
    let npc = control.npc(&resting_graph());
    control.checks(3);
    let empty = checks_to_notice(&mut control, npc, player);
    assert_eq!(empty, FILL_CHECKS + 1);

    let mut watch = Watch::new();
    let player = watch.player(hidden(40.0));
    let peers: Vec<EntityId> = (0..8)
        .map(|index| watch.hostile_peer(Vec3::new(-3.0 - index as f32, 0.0, 0.0)))
        .collect();
    let npc = watch.npc(&resting_graph());
    watch.checks(3);
    assert_eq!(watch.brain(npc).awareness.len(), AWARENESS_METER_CAPACITY);
    for &peer in &peers {
        watch.kill(peer);
        watch.reg.despawn(peer).unwrap();
    }
    watch.place(player, in_view(20.0));
    watch.check();
    assert_eq!(
        watch.brain(npc).awareness.len(),
        1,
        "the first check after they depart frees their slots"
    );
    let mut checks = 1;
    while watch.selected(npc) != Some(player) {
        watch.check();
        checks += 1;
        assert!(checks <= 40, "noticed");
    }
    assert_eq!(checks, empty, "as fast as with empty meters");
}

// Nine nearer hostile NPCs behind a wall never crowd out a farther player in
// view; with nine passing candidates, the farthest is the one left unmetered.
#[test]
fn failing_nearer_candidates_never_crowd_out_a_passing_one() {
    let mut control = Watch::new();
    let player = control.player(hidden(40.0));
    let npc = control.npc(&default_noticing());
    control.checks(3);
    let empty = checks_to_notice(&mut control, npc, player);

    let mut watch = Watch::new();
    let player = watch.player(hidden(40.0));
    for index in 0..9 {
        watch.hostile_peer(hidden(6.0 + index as f32));
    }
    let npc = watch.npc(&default_noticing());
    watch.checks(3);
    assert!(watch.brain(npc).awareness.is_empty(), "walled peers fail");
    assert_eq!(checks_to_notice(&mut watch, npc, player), empty);

    let mut watch = Watch::new();
    let peers: Vec<EntityId> = (0..9)
        .map(|index| watch.hostile_peer(in_view(5.0 + index as f32)))
        .collect();
    let npc = watch.npc(&resting_graph());
    watch.checks(2);
    assert!(peers[..8].iter().all(|&peer| watch.metered(npc, peer)));
    assert!(!watch.metered(npc, peers[8]), "the farthest is left out");
}

// A candidate evicted with awareness 0.8 and re-admitted later starts from
// zero.
#[test]
fn eviction_restarts_at_zero() {
    let mut watch = Watch::new();
    let player = watch.player(in_view(30.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(13);
    assert_near(watch.value(npc, player), 0.8, "0.8");
    let peers: Vec<EntityId> = (0..8)
        .map(|index| watch.hostile_peer(in_view(5.0 + index as f32)))
        .collect();
    watch.check();
    assert!(
        !watch.metered(npc, player),
        "evicted by eight nearer passers"
    );
    for peer in peers {
        watch.reg.despawn(peer).unwrap();
    }
    watch.check();
    assert_eq!(watch.value(npc, player), 0.0, "re-admitted at zero");
    for check in 1..FILL_CHECKS {
        watch.check();
        assert_eq!(watch.target(npc), None, "check {check}");
    }
    watch.check();
    assert_eq!(watch.target(npc), Some(player));
}

// ---------------------------------------------------------------------------
// Resting NPCs (AC 13, P53; AC 14, P58)
// ---------------------------------------------------------------------------

// A resting NPC that selected a player at 20 m and misses them for one check
// re-selects them on the second check in a row that sees them; one that
// misses them for longer than `drainMs` notices them only after a full fill.
#[test]
fn resting_npc_reselects_on_second_sighting_in_a_row() {
    let mut watch = Watch::new();
    let player = watch.player(hidden(20.0));
    let npc = watch.npc(&resting_graph());
    for _ in 0..=FILL_CHECKS {
        watch.check_with(false);
    }
    assert_eq!(watch.selected(npc), Some(player));
    assert_eq!(watch.target(npc), None, "a resting NPC keeps no target");
    watch.check_with(false);
    assert_eq!(
        watch.selected(npc),
        Some(player),
        "re-selected while in view"
    );

    watch.check();
    assert_eq!(watch.selected(npc), None, "the missing check loses them");
    watch.check_with(false);
    assert_eq!(
        watch.selected(npc),
        None,
        "a sighting after a miss gains nothing"
    );
    watch.check_with(false);
    assert_eq!(watch.selected(npc), Some(player), "the second in a row");

    // Missed past `drainMs`: a full fill.
    let missed = (2_000.0 / CHECK_MS).ceil() as usize + 1;
    watch.checks(missed);
    assert!(!watch.metered(npc, player), "drained out");
    watch.check_with(false);
    for check in 1..FILL_CHECKS {
        watch.check_with(false);
        assert_eq!(watch.selected(npc), None, "check {check}");
    }
    watch.check_with(false);
    assert_eq!(watch.selected(npc), Some(player));
}

// A resting NPC targeting A, with A seen on alternate checks and B in plain
// view, never changes target on consecutive checks, A nearer or farther:
// each missed check loses A; it re-takes A only after two sightings in a row,
// or takes B once B's meter fills.
#[test]
fn resting_npc_never_changes_target_on_consecutive_checks() {
    for a_distance in [8.0, 40.0] {
        let mut watch = Watch::new();
        let a = watch.player(hidden(a_distance));
        let npc = watch.npc(&resting_graph());
        for _ in 0..=FILL_CHECKS {
            watch.check_with(false);
        }
        assert_eq!(watch.selected(npc), Some(a));
        // B steps into plain view; A is seen on alternate checks.
        let b = watch.player(in_view(22.0));
        let mut selections = vec![watch.selected(npc)];
        let mut b_first_seen = None;
        for check in 0..40 {
            let a_seen = check % 2 == 1;
            watch.check_with(!a_seen);
            b_first_seen.get_or_insert(check);
            let selected = watch.selected(npc);
            if !a_seen {
                assert_ne!(selected, Some(a), "{a_distance} m: check {check} missed A");
            }
            selections.push(selected);
        }
        let changes: Vec<bool> = selections
            .windows(2)
            .map(|pair| pair[0] != pair[1])
            .collect();
        assert!(
            !changes.windows(2).any(|pair| pair[0] && pair[1]),
            "{a_distance} m: changed target on consecutive checks: {selections:?}"
        );
        let b_taken = selections
            .iter()
            .position(|selected| *selected == Some(b))
            .expect("B is taken once its meter fills");
        assert_eq!(
            b_taken - 1,
            b_first_seen.unwrap() + FILL_CHECKS,
            "{a_distance} m: B is taken on the check its meter fills"
        );
        assert!(
            selections[1..].iter().all(|selected| *selected != Some(a)),
            "A is never seen twice in a row, so never re-taken"
        );
    }
}

// ---------------------------------------------------------------------------
// Losing and switching targets (AC 17, P34; AC 18, P55; AC 20, P65)
// ---------------------------------------------------------------------------

// The NPC's target dies and its death guard drops it: a player in view is
// acquired once their meter, cleared when the NPC acquired the first, refills,
// never on the decide after the drop. One at 0.8 before the fight, stepping
// into view later, needs a full fill.
#[test]
fn death_drop_needs_a_refill() {
    for in_view_at_drop in [true, false] {
        let mut watch = Watch::new();
        let first = watch.player(in_view(10.0));
        let other = watch.player(hidden(12.0));
        let npc = watch.npc(&default_noticing());
        watch.checks(3);
        watch.place(other, Vec3::new(12.0, 0.0, -20.0));
        watch.checks(FILL_CHECKS - 2);
        assert_eq!(watch.target(npc), Some(first));
        assert!(
            !watch.metered(npc, other),
            "its 0.8 cleared at the acquisition"
        );
        if !in_view_at_drop {
            watch.place(other, hidden(12.0));
        }
        watch.checks(10);
        assert!(!watch.metered(npc, other), "frozen through the fight");
        watch.kill(first);
        watch.check();
        assert_eq!(watch.target(npc), None, "the death guard dropped it");
        watch.check();
        assert_eq!(
            watch.target(npc),
            None,
            "never on the decide after the drop"
        );
        if !in_view_at_drop {
            watch.checks(5);
            watch.place(other, Vec3::new(12.0, 0.0, -20.0));
            watch.check();
        }
        assert_eq!(watch.value(npc, other), 0.0, "a first sighting");
        for check in 1..FILL_CHECKS {
            watch.check();
            assert_eq!(watch.target(npc), None, "check {check}");
        }
        watch.check();
        assert_eq!(watch.target(npc), Some(other));
    }
}

// After a drop with B (20 m) and C (10 m) in view, meters cleared at the first
// acquisition: both in view since the drop, it acquires C, the nearer on the
// tie; B in view 0.3 s before C, it acquires B. Neither before a meter fills.
#[test]
fn after_a_drop_the_first_full_meter_wins_nearest_on_tie() {
    for c_late in [false, true] {
        let mut watch = Watch::new();
        let first = watch.player(in_view(5.0));
        let b = watch.player(Vec3::new(16.0, 0.0, 12.0));
        let c = watch.player(hidden(8.0));
        let npc = watch.npc(&default_noticing());
        watch.checks(FILL_CHECKS + 1);
        assert_eq!(watch.target(npc), Some(first));
        if !c_late {
            watch.place(c, Vec3::new(6.0, 0.0, 8.0));
        }
        watch.checks(3);
        watch.kill(first);
        watch.check();
        assert_eq!(watch.target(npc), None, "dropped");
        // B in view since the drop; C, when late, from 0.3 s or more after.
        let late = 5;
        assert!(late as f32 * CHECK_MS >= 300.0);
        for check in 0..FILL_CHECKS {
            if c_late && check == late {
                watch.place(c, Vec3::new(6.0, 0.0, 8.0));
            }
            watch.check();
            assert_eq!(watch.target(npc), None, "c late {c_late}: check {check}");
        }
        watch.check();
        let expected = if c_late { b } else { c };
        assert_eq!(watch.target(npc), Some(expected), "c late {c_late}");
        let other = if c_late { c } else { b };
        assert!(
            !watch.metered(npc, other),
            "acquiring clears the other's meter"
        );
    }
}

// An engaged NPC switching from teammate A to a nearer P by today's ranking
// clears A's meter: once P is lost, A needs a full fill.
#[test]
fn a_switch_is_an_acquisition() {
    let mut watch = Watch::new();
    let a = watch.player(in_view(20.0));
    let p = watch.player(hidden(30.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(FILL_CHECKS + 1);
    assert_eq!(watch.target(npc), Some(a));
    watch.place(p, Vec3::new(4.0, 0.0, -4.0));
    watch.check();
    assert_eq!(watch.target(npc), Some(p), "switched by ranking");
    assert!(!watch.metered(npc, a), "the switch cleared A's meter");
    assert_eq!(watch.value(npc, p), 1.0);
    watch.kill(p);
    watch.check();
    assert_eq!(watch.target(npc), None);
    watch.check();
    assert_eq!(watch.value(npc, a), 0.0, "A from zero");
    for check in 1..FILL_CHECKS {
        watch.check();
        assert_eq!(watch.target(npc), None, "check {check}");
    }
    watch.check();
    assert_eq!(watch.target(npc), Some(a));
}

// ---------------------------------------------------------------------------
// `brain.awareness` (AC 23, P35)
// ---------------------------------------------------------------------------

// A guard on `brain.awareness.ge(0.5)` fires on the decide whose check first
// raises the meter to 0.5, not the decide after; on the decide a meter
// reaches 1, `hasTarget` and `brain.awareness` 1 read true together.
#[test]
fn awareness_fact_guard_fires_on_the_decide_its_check_raises_the_meter() {
    let graph = |threshold: IrNode| {
        test_behavior_graph!({
            initial: "rest".to_string(),
            activities: BTreeMap::from([
                ("rest".to_string(), authored_state("idle", MotionVerb::Hold, None)),
                ("look".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            ]),
            transitions: BTreeMap::from([("rest".to_string(), vec![edge("look", threshold)])]),
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: BTreeMap::new(),
            engagement_radius: None,
            move_speed: TEST_MOVE_SPEED,
        })
    };
    // 0.5 = 7.5 checks: the eighth after the first sighting.
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&graph(awareness_at_least(0.5)));
    for check in 0..8 {
        watch.check();
        assert_eq!(watch.state(npc), "rest", "check {check}");
    }
    watch.check();
    assert_eq!(watch.state(npc), "look", "the check that raised it to 0.5");
    assert!(watch.value(npc, player) >= 0.5);

    let together = IrNode::And {
        a: Box::new(awareness_at_least(1.0)),
        b: Box::new(brain_input(BRAIN_HAS_TARGET_INPUT)),
    };
    let mut watch = Watch::new();
    watch.player(in_view(10.0));
    let npc = watch.npc(&graph(together));
    for check in 0..FILL_CHECKS {
        watch.check();
        assert_eq!(watch.state(npc), "rest", "check {check}");
    }
    watch.check();
    assert_eq!(
        watch.state(npc),
        "look",
        "hasTarget and awareness 1 together"
    );
}

// `brain.awareness` reads the highest meter, and 1 while the NPC has a target.
#[test]
fn awareness_fact_reads_the_highest_meter_and_one_with_a_target() {
    let at_least = |value: f32| {
        test_behavior_graph!({
            initial: "rest".to_string(),
            activities: BTreeMap::from([
                ("rest".to_string(), authored_state("idle", MotionVerb::Hold, None)),
                ("over".to_string(), authored_state("idle", MotionVerb::Hold, None)),
            ]),
            transitions: BTreeMap::from([(
                "rest".to_string(),
                vec![edge("over", awareness_at_least(value))],
            )]),
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: BTreeMap::new(),
            engagement_radius: None,
            move_speed: TEST_MOVE_SPEED,
        })
    };
    // Two players, seen from checks 0 and 3: after check 6 the meters read
    // 0.4 and 0.2; the fact reads the higher.
    let mut watch = Watch::new();
    watch.player(in_view(10.0));
    let late = watch.player(hidden(10.0));
    let below = watch.npc(&at_least(0.41));
    let above = watch.npc(&at_least(0.39));
    watch.checks(3);
    watch.place(late, Vec3::new(10.0, 0.0, -20.0));
    watch.checks(4);
    assert_eq!(watch.state(below), "rest");
    assert_eq!(watch.state(above), "over");

    // With a target the fact reads 1, whatever its meters.
    let engaged = test_behavior_graph!({
        initial: "chase".to_string(),
        activities: BTreeMap::from([
            (
                "chase".to_string(),
                authored_state("walk", MotionVerb::ChaseTarget, None),
            ),
            (
                "sure".to_string(),
                authored_state("walk", MotionVerb::ChaseTarget, None),
            ),
        ]),
        transitions: BTreeMap::from([(
            "chase".to_string(),
            vec![edge("sure", awareness_at_least(1.0))],
        )]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    });
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&engaged);
    watch.brain_mut(npc, |brain| brain.acquired_target = Some(player));
    watch.check();
    assert_eq!(watch.state(npc), "sure");
}

// ---------------------------------------------------------------------------
// Ties and zero durations (AC 26, P36, P37)
// ---------------------------------------------------------------------------

// Two players whose meters reach 1 on one check: the NPC acquires the nearer,
// and acquiring clears the farther one's meter.
#[test]
fn tie_takes_nearest_and_clears_the_other() {
    let mut watch = Watch::new();
    let far = watch.player(Vec3::new(16.0, 0.0, 12.0));
    let near = watch.player(Vec3::new(6.0, 0.0, 8.0));
    let npc = watch.npc(&default_noticing());
    watch.checks(FILL_CHECKS);
    assert_eq!(watch.value(npc, far), watch.value(npc, near));
    watch.check();
    assert_eq!(watch.target(npc), Some(near));
    assert!(!watch.metered(npc, far));
}

// `fillMs` 0 loads and notices on the same check as `fillMs` 30: the second
// sighting; `drainMs` 0 empties a meter on the first check that misses.
#[test]
fn zero_durations_act_as_one_interval() {
    for fill_ms in [0.0, 30.0] {
        let mut watch = Watch::new();
        let player = watch.player(in_view(10.0));
        let npc = watch.npc(&noticing_graph(fill_ms, 2_000.0));
        watch.check();
        assert_eq!(watch.target(npc), None, "fillMs {fill_ms}: first sighting");
        watch.check();
        assert_eq!(watch.target(npc), Some(player), "fillMs {fill_ms}");
    }

    let mut watch = Watch::new();
    let player = watch.player(hidden(10.0));
    let npc = watch.npc(&noticing_graph(1_000.0, 0.0));
    watch.checks_with_sight(6);
    assert!(watch.value(npc, player) > 0.0);
    watch.check();
    assert!(!watch.metered(npc, player), "emptied on the first miss");
}

impl Watch {
    fn checks_with_sight(&mut self, count: usize) {
        for _ in 0..count {
            self.check_with(false);
        }
    }
}

// ---------------------------------------------------------------------------
// Live reload (AC 25, P31)
// ---------------------------------------------------------------------------

// A hot reload changing `fillMs` from 1000 to 500 with a meter at 0.4 keeps
// 0.4, then rises at the new rate from the next check.
#[test]
fn fill_ms_reload_keeps_value_and_takes_new_rate() {
    use postretro_sim::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;

    let mut descriptor = behavior_enemy_descriptor("sentry");
    let mut graph = default_noticing();
    graph.ai.awareness = Some(AwarenessDescriptor {
        fill_ms: 1_000.0,
        drain_ms: 2_000.0,
    });
    descriptor.behavior = Some(graph.clone());
    let mut watch = Watch::new();
    let player = watch.player(in_view(10.0));
    let npc = watch.npc(&graph);
    watch
        .reg
        .set_component(
            npc,
            postretro_entities::provenance::DescriptorProvenance {
                canonical_name: "sentry".to_string(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: postretro_entities::provenance::DescriptorSpawnPath::MapPlacement,
            },
        )
        .unwrap();
    let factions = FactionRegistry::default();
    let sentiment = RefCell::new(FactionSentimentState::default());
    let tick = |watch: &mut Watch, descriptors: &[EntityTypeDescriptor], generation: u64| {
        run_ai_tick_with_navigation_and_impact(
            &mut watch.reg,
            &mut watch.runtime,
            DT,
            AiTickInputs {
                nav_graph: None,
                mover_nav: crate::nav::MoverNav::none(),
                collision_world: None,
                descriptors,
                descriptor_generation: generation,
                factions: &factions,
                faction_sentiment: &sentiment,
                clips: None,
                schedule: Default::default(),
                cpu: None,
            },
            |_| {},
        );
        watch.ticks += 1;
    };
    tick(&mut watch, std::slice::from_ref(&descriptor), 1);
    for _ in 0..6 * DECIDE_INTERVAL_TICKS {
        tick(&mut watch, std::slice::from_ref(&descriptor), 1);
    }
    assert_near(watch.value(npc, player), 0.4, "0.4 at 1000 ms");
    descriptor.behavior.as_mut().unwrap().ai.awareness = Some(AwarenessDescriptor {
        fill_ms: 500.0,
        drain_ms: 2_000.0,
    });
    for _ in 0..DECIDE_INTERVAL_TICKS - 1 {
        tick(&mut watch, std::slice::from_ref(&descriptor), 2);
        assert_near(watch.value(npc, player), 0.4, "the reload keeps 0.4");
    }
    tick(&mut watch, std::slice::from_ref(&descriptor), 2);
    assert_near(
        watch.value(npc, player),
        0.4 + CHECK_MS / 500.0,
        "the next check rises at the new rate",
    );
}
