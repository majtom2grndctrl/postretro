// Engaged motion through the AI tick: `strafe` and `reposition` on the rotated
// combat ring, activity facing, and directional travel clips.
// See: context/lib/entity_model.md §7c (Engaged motion and facing)

use super::*;
use crate::combat_positioning::slot_spacing;
use postretro_entities::components::brain::{EngagedMotionMemory, TravelVariant};
use postretro_foundation::{FacingPolicy, RepositionParams};

/// Standoff for every graph here: the graph-level engagement radius, which an
/// actionless activity and an attack without its own standoff both use.
const R: f32 = 4.0;
const TOLERANCE: f32 = 1.0e-3;
/// One tick long enough for facing to reach any heading (`FACING_TURN_RATE`
/// is 2τ rad/s), so a facing check reads the resolved direction exactly.
const SNAP_DT: f32 = 0.5;

fn target_pos() -> Vec3 {
    Vec3::new(20.0, chaser_rest_y(), 20.0)
}

/// The point at `yaw` (radians, +yaw counter-clockwise seen from above)
/// around `center` at the standoff.
fn ring_point(center: Vec3, yaw: f32) -> Vec3 {
    center + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * R
}

fn bearing_yaw(from: Vec3, to: Vec3) -> f32 {
    (to.x - from.x).atan2(to.z - from.z)
}

const DEG: f32 = std::f32::consts::PI / 180.0;
/// Bearing yaw of world +X from a target.
const EAST: f32 = std::f32::consts::FRAC_PI_2;

fn activity(
    animation: &str,
    motion: MotionVerb,
    action: Option<ActionVerb>,
) -> BehaviorActivityDescriptor {
    authored_state(animation, motion, action)
}

fn reposition_activity(params: Option<RepositionParams>) -> BehaviorActivityDescriptor {
    let mut move_activity = activity("walk", MotionVerb::Reposition, None);
    move_activity.reposition = params;
    move_activity
}

fn arc(min: f32, max: f32) -> Option<RepositionParams> {
    Some(RepositionParams {
        arc_deg: [min, max],
        min_travel_m: None,
    })
}

fn min_travel(metres: f32) -> Option<RepositionParams> {
    Some(RepositionParams {
        arc_deg: postretro_foundation::DEFAULT_REPOSITION_ARC_DEG,
        min_travel_m: Some(metres),
    })
}

/// A contact attack in reach of the standoff, slow to cool down so the fire
/// latch shows the one shot an activity entry allows.
fn shoot_attack() -> BTreeMap<String, AttackParams> {
    BTreeMap::from([(
        "shoot".to_string(),
        AttackParams {
            sound: None,
            weapon: None,
            damage: Some(5.0),
            max_range: Some(R + 1.0),
            cooldown_ms: Some(100_000.0),
            engagement_radius: None,
            standoff_distance: None,
            damage_type: None,
            draws: None,
        },
    )])
}

/// A graph of root activities with no transitions: tests move between them
/// through the brain's own entry API.
fn engaged_graph(
    initial: &str,
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
) -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: initial.to_string(),
        activities: activities
            .into_iter()
            .map(|(name, activity)| (name.to_string(), activity))
            .collect(),
        transitions: BTreeMap::new(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: shoot_attack(),
        engagement_radius: Some(R),
        move_speed: TEST_MOVE_SPEED,
    })
}

/// Spawn an engaged enemy in `state`, facing its target.
fn spawn_engaged(
    reg: &mut EntityRegistry,
    position: Vec3,
    graph: &BehaviorGraphDescriptor,
    state: &str,
) -> EntityId {
    let enemy = spawn_enemy(reg, position, authored_brain(graph, state), 50.0);
    set_enemy_yaw(reg, enemy, bearing_yaw(position, target_pos()));
    enemy
}

/// Enter `state` as a transition would: a fresh entry on the next tick.
fn enter(reg: &mut EntityRegistry, enemy: EntityId, state: &str) {
    let mut brain = reg.get_component::<BrainComponent>(enemy).unwrap().clone();
    let index = graph_activity_index(&brain.graph, state).expect("the graph declares it");
    assert!(brain.enter_activity_at(0, index));
    reg.set_component(enemy, brain).unwrap();
}

fn memory(reg: &EntityRegistry, enemy: EntityId) -> EngagedMotionMemory {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .engaged_motion
}

fn strafe_direction(reg: &EntityRegistry, enemy: EntityId) -> i8 {
    match memory(reg, enemy) {
        EngagedMotionMemory::Strafe(strafe) => strafe.direction,
        other => panic!("expected strafe memory, got {other:?}"),
    }
}

fn set_strafe_direction(reg: &mut EntityRegistry, enemy: EntityId, direction: i8) {
    let mut brain = reg.get_component::<BrainComponent>(enemy).unwrap().clone();
    let EngagedMotionMemory::Strafe(strafe) = &mut brain.engaged_motion else {
        panic!("expected strafe memory");
    };
    strafe.direction = direction;
    reg.set_component(enemy, brain).unwrap();
}

fn destination(reg: &EntityRegistry, enemy: EntityId) -> Option<Vec3> {
    enemy_agent(reg, enemy).destination
}

fn position(reg: &EntityRegistry, enemy: EntityId) -> Vec3 {
    reg.get_component::<Transform>(enemy).unwrap().position
}

fn assert_xz(actual: Option<Vec3>, expected: Vec3, message: &str) {
    let actual = actual.unwrap_or_else(|| panic!("{message}: no destination"));
    assert!(
        distance_xz(actual, expected) <= TOLERANCE,
        "{message}: expected {expected:?}, got {actual:?}"
    );
}

fn region_nav(x0: u32, z0: u32, x1: u32, z1: u32) -> NavGraph {
    let mut section = OpenFloor::new().navmesh();
    section.regions = vec![NavRegion {
        x0,
        z0,
        x1,
        z1,
        floor_y_min: 0.0,
        floor_y_max: 0.25,
    }];
    NavGraph::from_section(&section)
}

/// The open floor plus one vertical wall quad at `x`, spanning `z0..z1`.
fn floor_with_wall(x: f32, z0: f32, z1: f32) -> CollisionWorld {
    let extent = 40.0;
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(extent, 0.0, 0.0),
            Vec3::new(extent, 0.0, extent),
            Vec3::new(0.0, 0.0, extent),
            Vec3::new(x, 0.0, z0),
            Vec3::new(x, 3.0, z0),
            Vec3::new(x, 3.0, z1),
            Vec3::new(x, 0.0, z1),
        ],
        vec![[0u32, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
    )
}

fn ai_tick(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    world: &CollisionWorld,
    nav: &NavGraph,
) {
    run_ai_tick_with_navigation(reg, runtime, STEER_DT, Some(nav), Some(world));
}

// ---------------------------------------------------------------------------
// Strafe
// ---------------------------------------------------------------------------

// Row M1.
#[test]
fn strafe_orbits_at_standoff_facing_target_and_fires_when_gates_pass() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let graph = engaged_graph(
        "wait",
        vec![(
            "wait",
            activity(
                "walk",
                MotionVerb::Strafe,
                Some(ActionVerb::Attack("shoot".to_string())),
            ),
        )],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let player = spawn_player(&mut reg, target_pos());
    // Out of the attack's reach, so the shot can only land once the strafe
    // has carried it in.
    let enemy = spawn_engaged(
        &mut reg,
        target_pos() + Vec3::new(2.0 * R, 0.0, 0.0),
        &graph,
        "wait",
    );

    let mut fired_moving = None;
    let mut swept = 0.0_f32;
    let mut last_bearing = bearing_yaw(target_pos(), position(&reg, enemy));
    for tick in 0..300 {
        let moving_before = LocomotionIntent::from_velocity(
            agent_steering::path_state(&reg, enemy)
                .unwrap()
                .steering_velocity(),
        )
        .moving;
        let events =
            run_ai_tick_with_navigation(&mut reg, &mut runtime, STEER_DT, Some(&nav), Some(&world));
        if events.contains(&Cow::Borrowed(ENEMY_ATTACK_EVENT)) {
            assert!(fired_moving.is_none(), "one shot per activity entry");
            fired_moving = Some(moving_before);
        }
        agent_steering::tick(
            &mut reg,
            &world,
            Some(&nav),
            postretro_collision::EnvironmentView::level_only(STEER_GRAVITY),
            STEER_DT,
        );
        let bearing = bearing_yaw(target_pos(), position(&reg, enemy));
        let step = (bearing - last_bearing + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        swept += step;
        last_bearing = bearing;
        if tick >= 120 {
            let distance = distance_xz(position(&reg, enemy), target_pos());
            assert!(
                (0.8 * R..=1.2 * R).contains(&distance),
                "tick {tick}: the strafe orbits at standoff, got {distance}"
            );
            assert!(
                yaw_distance(
                    enemy_yaw(&reg, enemy),
                    bearing_yaw(position(&reg, enemy), target_pos())
                ) <= super::facing::ATTACK_FACING_TOLERANCE_RADIANS,
                "tick {tick}: the strafer faces its target while it moves"
            );
            assert!(
                enemy_agent(&reg, enemy).steer_velocity.length() > 1.0,
                "tick {tick}: the strafe keeps moving"
            );
        }
    }
    assert!(
        swept.abs() > 90.0 * DEG,
        "the strafe circles its target in one direction, swept {} degrees",
        swept / DEG
    );
    assert_eq!(
        fired_moving,
        Some(true),
        "the action fires mid-move once facing, range and LOS pass"
    );
    assert_eq!(player_hp(&reg, player), 95.0);
}

/// One strafer east of the target, its entry consumed and its direction set
/// counter-clockwise, so its next slot lies at 45° (south-east, -Z) and its
/// reverse at -45° (north-east).
fn strafer_heading_ccw(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    world: &CollisionWorld,
    nav: &NavGraph,
) -> EntityId {
    let graph = engaged_graph(
        "wait",
        vec![("wait", activity("walk", MotionVerb::Strafe, None))],
    );
    spawn_player(reg, target_pos());
    let enemy = spawn_engaged(reg, ring_point(target_pos(), EAST), &graph, "wait");
    ai_tick(reg, runtime, world, nav);
    set_strafe_direction(reg, enemy, 1);
    enemy
}

// Row M2: an unreachable, an occupied and a blind next slot each reverse.
#[test]
fn strafe_reverses_at_blocked_occupied_or_blind_slot() {
    let ahead = ring_point(target_pos(), EAST + 45.0 * DEG);
    let reverse = ring_point(target_pos(), EAST - 45.0 * DEG);
    assert!(ahead.z < 18.0 && reverse.z > 22.0, "fixture geometry");

    // Blocked: the next slot is off the navmesh.
    {
        let world = OpenFloor::new().collision_world();
        let nav = region_nav(0, 18, 40, 40);
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        let enemy = strafer_heading_ccw(&mut reg, &mut runtime, &world, &nav);
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        assert_xz(
            destination(&reg, enemy),
            reverse,
            "blocked: walks the other way",
        );
        assert_eq!(strafe_direction(&reg, enemy), -1, "blocked: reversed");
    }

    // Occupied: another enemy stands on the next slot.
    {
        let floor = OpenFloor::new();
        let world = floor.collision_world();
        let nav = floor.nav_graph();
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        let enemy = strafer_heading_ccw(&mut reg, &mut runtime, &world, &nav);
        let mut bystander = brain_with(tuning(), TEST_IDLE_STATE);
        bystander.aggro_armed = false;
        spawn_enemy(&mut reg, ahead, bystander, 50.0);
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        assert_xz(
            destination(&reg, enemy),
            reverse,
            "occupied: walks the other way",
        );
        assert_eq!(strafe_direction(&reg, enemy), -1, "occupied: reversed");
    }

    // Blind: a wall hides the target from the next slot, not from the strafer.
    {
        let world = floor_with_wall(21.5, 17.8, 19.2);
        let nav = OpenFloor::new().nav_graph();
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        let enemy = strafer_heading_ccw(&mut reg, &mut runtime, &world, &nav);
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        assert_xz(
            destination(&reg, enemy),
            reverse,
            "blind: walks the other way",
        );
        assert_eq!(strafe_direction(&reg, enemy), -1, "blind: reversed");
    }
}

// Row M2 / pin P9: with neither neighbor usable it holds facing the target and
// never flips, until a neighbor frees.
#[test]
fn strafe_holds_without_flipping_when_both_neighbors_unusable() {
    let world = OpenFloor::new().collision_world();
    let corridor = region_nav(0, 19, 40, 21);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy = strafer_heading_ccw(&mut reg, &mut runtime, &world, &corridor);
    set_enemy_yaw(&mut reg, enemy, 0.0);

    for tick in 0..4 {
        run_ai_tick_with_navigation(
            &mut reg,
            &mut runtime,
            SNAP_DT,
            Some(&corridor),
            Some(&world),
        );
        assert_eq!(
            destination(&reg, enemy),
            None,
            "tick {tick}: holds its slot"
        );
        assert_eq!(strafe_direction(&reg, enemy), 1, "tick {tick}: never flips");
        assert!(
            yaw_distance(enemy_yaw(&reg, enemy), -EAST) <= TOLERANCE,
            "tick {tick}: holding, it still faces the target"
        );
    }

    let open = OpenFloor::new().nav_graph();
    ai_tick(&mut reg, &mut runtime, &world, &open);
    assert_xz(
        destination(&reg, enemy),
        ring_point(target_pos(), EAST + 45.0 * DEG),
        "a freed neighbor resumes the strafe in its direction",
    );
}

// Row M2 / pin P9: two strafers converging on one slot; the batch loser
// reverses once and does not flip back.
#[test]
fn strafe_batch_loser_reverses_once() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let graph = engaged_graph(
        "wait",
        vec![("wait", activity("walk", MotionVerb::Strafe, None))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    // East strafing counter-clockwise and south (-Z) strafing clockwise share
    // the south-east slot.
    let south = std::f32::consts::PI;
    let east = spawn_engaged(&mut reg, ring_point(target_pos(), EAST), &graph, "wait");
    let southern = spawn_engaged(&mut reg, ring_point(target_pos(), south), &graph, "wait");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    set_strafe_direction(&mut reg, east, 1);
    set_strafe_direction(&mut reg, southern, -1);
    let contested = ring_point(target_pos(), EAST + 45.0 * DEG);

    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let directions = [
        strafe_direction(&reg, east),
        strafe_direction(&reg, southern),
    ];
    let reversed = usize::from(directions[0] == -1) + usize::from(directions[1] == 1);
    assert_eq!(reversed, 1, "exactly the loser reverses: {directions:?}");
    let (winner, loser) = if directions[0] == 1 {
        (east, southern)
    } else {
        (southern, east)
    };
    assert_xz(
        destination(&reg, winner),
        contested,
        "the winner takes the slot",
    );
    let loser_reverse = if loser == east {
        ring_point(target_pos(), EAST - 45.0 * DEG)
    } else {
        ring_point(target_pos(), south + 45.0 * DEG)
    };
    assert_xz(
        destination(&reg, loser),
        loser_reverse,
        "the loser walks away",
    );

    let loser_direction = strafe_direction(&reg, loser);
    for tick in 0..3 {
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        assert_eq!(
            strafe_direction(&reg, loser),
            loser_direction,
            "tick {tick}: the loser reverses once, not back"
        );
    }
}

// Row M12: a strafe entered off the shared ring steps 45° from wherever the
// enemy stands, never snapping to a world-axis slot.
#[test]
fn strafe_after_reposition_steps_from_current_bearing() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let graph = engaged_graph(
        "move",
        vec![
            ("move", reposition_activity(None)),
            ("wait", activity("walk", MotionVerb::Strafe, None)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    // A 30° entry bearing rotates the reposition ring off the world axes.
    let enemy = spawn_engaged(
        &mut reg,
        ring_point(target_pos(), 30.0 * DEG),
        &graph,
        "move",
    );
    for _ in 0..120 {
        ai_then_steering_tick(&mut reg, &mut runtime, &world, &nav);
    }
    let arrived = position(&reg, enemy);
    let slot = destination(&reg, enemy).expect("the reposition claimed a slot");
    assert!(
        distance_xz(arrived, slot) < 1.0,
        "fixture: the reposition arrives"
    );

    enter(&mut reg, enemy, "wait");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let bearing = bearing_yaw(target_pos(), arrived);
    let goal = destination(&reg, enemy).expect("the strafe steps");
    let goal_bearing = bearing_yaw(target_pos(), goal);
    let step = yaw_distance(bearing, goal_bearing);
    assert!(
        (step - 45.0 * DEG).abs() <= TOLERANCE,
        "the strafe steps 45° from its current bearing, stepped {} degrees",
        step / DEG
    );
    let off_axis = (goal_bearing / DEG).rem_euclid(45.0);
    assert!(
        off_axis.min(45.0 - off_axis) > 1.0,
        "the step is not snapped to the shared world-axis ring: {} degrees",
        goal_bearing / DEG
    );
}

// ---------------------------------------------------------------------------
// Reposition
// ---------------------------------------------------------------------------

/// A graph whose `aim` holds with an action, so it holds a combat slot, and
/// whose `rest` holds without one, so it holds none; either precedes `move`.
fn reposition_graph(params: Option<RepositionParams>) -> BehaviorGraphDescriptor {
    engaged_graph(
        "aim",
        vec![
            (
                "aim",
                activity(
                    "attack",
                    MotionVerb::Hold,
                    Some(ActionVerb::Attack("shoot".to_string())),
                ),
            ),
            ("rest", activity("idle", MotionVerb::Hold, None)),
            ("move", reposition_activity(params)),
        ],
    )
}

fn reposition_entry(reg: &EntityRegistry, enemy: EntityId) -> (Vec3, Option<Vec3>) {
    match memory(reg, enemy) {
        EngagedMotionMemory::Reposition(entry) => (entry.entry_position, entry.entry_slot),
        other => panic!("expected reposition memory, got {other:?}"),
    }
}

/// Run AI and steering until the agent arrives, returning its claimed slot.
fn walk_reposition(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    world: &CollisionWorld,
    nav: &NavGraph,
    enemy: EntityId,
) -> Vec3 {
    for _ in 0..240 {
        ai_then_steering_tick(reg, runtime, world, nav);
    }
    let slot = destination(reg, enemy).expect("the reposition claims a slot");
    assert!(
        distance_xz(position(reg, enemy), slot) <= 0.6,
        "the reposition arrives and holds at its slot"
    );
    slot
}

// Row M3: from a held slot, from a `hold` with no slot, and on re-entry.
#[test]
fn reposition_arrives_one_spacing_from_entry_position_and_slot() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let spacing = slot_spacing(R);

    // After an engaged hold that claimed a slot.
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    let enemy = spawn_engaged(
        &mut reg,
        ring_point(target_pos(), EAST),
        &reposition_graph(None),
        "aim",
    );
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let held = enemy_combat_slot(&reg, enemy).expect("fixture: aim holds a slot");
    enter(&mut reg, enemy, "move");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let (entry_position, entry_slot) = reposition_entry(&reg, enemy);
    assert_eq!(entry_slot, Some(held), "the held slot is recorded at entry");
    let first = walk_reposition(&mut reg, &mut runtime, &world, &nav, enemy);
    assert!(distance_xz(first, entry_position) + TOLERANCE >= spacing);
    assert!(distance_xz(first, held) + TOLERANCE >= spacing);

    // Re-entering picks again, a spacing from where the last pick arrived.
    enter(&mut reg, enemy, "aim");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    enter(&mut reg, enemy, "move");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let (second_entry, _) = reposition_entry(&reg, enemy);
    assert!(
        distance_xz(second_entry, first) <= 0.6,
        "re-entry records a fresh entry position"
    );
    let second = walk_reposition(&mut reg, &mut runtime, &world, &nav, enemy);
    assert!(distance_xz(second, second_entry) + TOLERANCE >= spacing);
    assert!(
        distance_xz(second, first) + TOLERANCE >= spacing,
        "it moves on"
    );

    // After an actionless `hold`, which holds no slot.
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    let enemy = spawn_engaged(
        &mut reg,
        ring_point(target_pos(), EAST),
        &reposition_graph(None),
        "rest",
    );
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    assert_eq!(
        enemy_combat_slot(&reg, enemy),
        None,
        "fixture: rest holds no slot"
    );
    enter(&mut reg, enemy, "move");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let (entry_position, entry_slot) = reposition_entry(&reg, enemy);
    assert_eq!(entry_slot, None);
    let slot = walk_reposition(&mut reg, &mut runtime, &world, &nav, enemy);
    assert!(distance_xz(slot, entry_position) + TOLERANCE >= spacing);
}

/// Spawn a repositioner east of the target and run `ticks` AI ticks, returning
/// the destination after each.
fn reposition_destinations(
    params: Option<RepositionParams>,
    nav: &NavGraph,
    ticks: usize,
) -> Vec<Option<Vec3>> {
    reposition_destinations_beside(params, nav, ticks, &[])
}

/// As [`reposition_destinations`], with idle enemies standing at `bystanders`.
fn reposition_destinations_beside(
    params: Option<RepositionParams>,
    nav: &NavGraph,
    ticks: usize,
    bystanders: &[Vec3],
) -> Vec<Option<Vec3>> {
    let world = OpenFloor::new().collision_world();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, target_pos());
    for &at in bystanders {
        let mut bystander = brain_with(tuning(), TEST_IDLE_STATE);
        bystander.aggro_armed = false;
        spawn_enemy(&mut reg, at, bystander, 50.0);
    }
    let enemy = spawn_engaged(
        &mut reg,
        ring_point(target_pos(), EAST),
        &reposition_graph(params),
        "move",
    );
    (0..ticks)
        .map(|_| {
            ai_tick(&mut reg, &mut runtime, &world, nav);
            destination(&reg, enemy)
        })
        .collect()
}

// Row M3: with no valid slot it holds rather than oscillating.
#[test]
fn reposition_holds_without_oscillating_when_no_slot_qualifies() {
    // A short corridor around the enemy: every ring slot but its own lies off
    // the navmesh, and its own is the radial slot.
    let corridor = region_nav(19, 19, 26, 21);
    let destinations = reposition_destinations(None, &corridor, 6);
    assert!(
        destinations.iter().all(Option::is_none),
        "no qualifying slot holds every tick: {destinations:?}"
    );
}

// Row M4: arc bounds select the generated ring slots on both sides; an arc
// with no qualifying slot holds.
#[test]
fn reposition_arc_bounds_select_ring_slots_on_both_sides() {
    let nav = OpenFloor::new().nav_graph();
    let lateral = [
        ring_point(target_pos(), EAST + 90.0 * DEG),
        ring_point(target_pos(), EAST - 90.0 * DEG),
    ];
    let picked = reposition_destinations(arc(90.0, 90.0), &nav, 1)[0].expect("a lateral slot");
    assert!(
        lateral
            .iter()
            .any(|slot| distance_xz(*slot, picked) <= TOLERANCE),
        "[90, 90] picks a lateral slot, got {picked:?}"
    );

    let far = reposition_destinations(arc(180.0, 180.0), &nav, 1)[0];
    assert_xz(
        far,
        ring_point(target_pos(), EAST + 180.0 * DEG),
        "[180, 180] picks the far slot",
    );

    // [45, 135] reaches both 45° slots and both 135° slots; occupy the 45°
    // pair and the 90° pair and it must reach a 135° slot on either side.
    let occupied: Vec<Vec3> = [45.0, -45.0, 90.0, -90.0]
        .iter()
        .map(|degrees| ring_point(target_pos(), EAST + degrees * DEG))
        .collect();
    let wide = reposition_destinations_beside(arc(45.0, 135.0), &nav, 1, &occupied)[0]
        .expect("a 135° slot remains");
    let rear = [
        ring_point(target_pos(), EAST + 135.0 * DEG),
        ring_point(target_pos(), EAST - 135.0 * DEG),
    ];
    assert!(
        rear.iter()
            .any(|slot| distance_xz(*slot, wide) <= TOLERANCE),
        "[45, 135] includes the 135° slots, got {wide:?}"
    );

    let none = reposition_destinations(arc(0.0, 0.0), &nav, 4);
    assert!(
        none.iter().all(Option::is_none),
        "an arc holding only the radial slot holds: {none:?}"
    );
}

// Row M5 / pin P16: the arc and entry position stay fixed while the target
// moves; re-entering measures them again.
#[test]
fn reposition_entry_bearing_and_position_stay_fixed_while_target_moves() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let player = spawn_player(&mut reg, target_pos());
    let start = ring_point(target_pos(), EAST);
    let enemy = spawn_engaged(&mut reg, start, &reposition_graph(arc(90.0, 90.0)), "move");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let (entry_position, _) = reposition_entry(&reg, enemy);

    for step in 1..=5 {
        let moved = target_pos() + Vec3::new(0.0, 0.0, 0.6 * step as f32);
        set_enemy_position(&mut reg, player, moved);
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        let goal = destination(&reg, enemy).expect("a lateral slot");
        // Lateral to the entry bearing (east), around where the target is now.
        let lateral = [
            ring_point(moved, EAST + 90.0 * DEG),
            ring_point(moved, EAST - 90.0 * DEG),
        ];
        assert!(
            lateral
                .iter()
                .any(|slot| distance_xz(*slot, goal) <= TOLERANCE),
            "step {step}: the slot re-scores around the moved target on the entry bearing, \
             got {goal:?}"
        );
        assert_eq!(
            reposition_entry(&reg, enemy).0,
            entry_position,
            "step {step}"
        );
    }

    // Re-entry measures the bearing from where the target stands now: north
    // of the enemy, so the lateral slots lie east and west of the target.
    let moved = Vec3::new(start.x, target_pos().y, start.z + 2.0 * R);
    set_enemy_position(&mut reg, player, moved);
    enter(&mut reg, enemy, "move");
    ai_tick(&mut reg, &mut runtime, &world, &nav);
    let (reentry_position, _) = reposition_entry(&reg, enemy);
    assert_eq!(reentry_position, position(&reg, enemy));
    let goal = destination(&reg, enemy).expect("a lateral slot");
    let entry_bearing = bearing_yaw(moved, position(&reg, enemy));
    let lateral = [
        ring_point(moved, entry_bearing + 90.0 * DEG),
        ring_point(moved, entry_bearing - 90.0 * DEG),
    ];
    assert!(
        lateral
            .iter()
            .any(|slot| distance_xz(*slot, goal) <= TOLERANCE),
        "re-entry re-measures the arc, got {goal:?}"
    );
}

// Row M6: a minimum travel beyond every candidate holds without error or a
// flipping goal. (The 7 m / 9 m bound is pinned in the ring's own tests.)
#[test]
fn reposition_min_travel_beyond_every_candidate_holds() {
    let nav = OpenFloor::new().nav_graph();
    let destinations = reposition_destinations(min_travel(3.0 * R), &nav, 6);
    assert!(
        destinations.iter().all(Option::is_none),
        "nothing lies that far: {destinations:?}"
    );

    // Within reach, the same bound picks only slots at least that far.
    let destinations = reposition_destinations(min_travel(1.9 * R), &nav, 1);
    let slot = destinations[0].expect("the far side lies 2R away");
    assert!(distance_xz(slot, ring_point(target_pos(), EAST)) + TOLERANCE >= 1.9 * R);
}

// Row M7: twins entering on one tick claim distinct slots, and a replay
// assigns the same ones.
#[test]
fn reposition_twins_claim_distinct_slots_deterministically() {
    let run = || {
        let floor = OpenFloor::new();
        let world = floor.collision_world();
        let nav = floor.nav_graph();
        let graph = reposition_graph(arc(45.0, 135.0));
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        spawn_player(&mut reg, target_pos());
        let near = spawn_engaged(&mut reg, ring_point(target_pos(), EAST), &graph, "move");
        let far = spawn_engaged(
            &mut reg,
            target_pos() + Vec3::new(1.5 * R, 0.0, 0.0),
            &graph,
            "move",
        );
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        [destination(&reg, near), destination(&reg, far)]
    };
    let first = run();
    let [Some(a), Some(b)] = first else {
        panic!("both claim a slot: {first:?}");
    };
    assert!(distance_xz(a, b) > 0.5, "distinct slots: {a:?} {b:?}");
    let replay = run();
    assert_eq!(replay, first, "a replay assigns the same slots");
}

// Row M13: from directly above its target, the arc is measured from the held
// slot, or ignored with none.
#[test]
fn reposition_above_target_measures_from_held_slot_or_ignores_arc() {
    let world = OpenFloor::new().collision_world();
    let run = |held: Option<Vec3>, nav: &NavGraph, params| {
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        // The target stands on a ledge directly above the enemy.
        spawn_player(&mut reg, target_pos() + Vec3::new(0.0, 1.5, 0.0));
        let mut brain = authored_brain(&reposition_graph(params), "move");
        brain.combat_slot = held;
        let enemy = spawn_enemy(&mut reg, target_pos(), brain, 50.0);
        ai_tick(&mut reg, &mut runtime, &world, nav);
        destination(&reg, enemy)
    };
    let open = OpenFloor::new().nav_graph();

    let held = ring_point(target_pos(), EAST);
    let goal = run(Some(held), &open, arc(90.0, 90.0)).expect("a slot lateral to the held one");
    let lateral = [
        ring_point(target_pos(), EAST + 90.0 * DEG),
        ring_point(target_pos(), EAST - 90.0 * DEG),
    ];
    assert!(
        lateral
            .iter()
            .any(|slot| distance_xz(*slot, goal) <= TOLERANCE),
        "the arc is measured from the held slot, got {goal:?}"
    );

    // Honored from any bearing, [180, 180] would leave one slot; with the
    // only slot west of the target off the navmesh, a claim proves the arc
    // was ignored.
    let no_west = region_nav(17, 0, 40, 40);
    assert!(
        run(None, &no_west, arc(180.0, 180.0)).is_some(),
        "with no held slot the arc is ignored"
    );
}

// Row M13: straight inward or outward on the entry bearing never qualifies.
#[test]
fn reposition_never_picks_radial_slots() {
    let world = OpenFloor::new().collision_world();
    let nav = OpenFloor::new().nav_graph();
    let radial = ring_point(target_pos(), EAST);
    for (case, offset) in [("outward", 2.0 * R), ("inward", 0.6 * R)] {
        let mut reg = EntityRegistry::new();
        let mut runtime = AiRuntime::new();
        spawn_player(&mut reg, target_pos());
        // No minimum travel, so only the radial rule can refuse the nearest
        // slot.
        let enemy = spawn_engaged(
            &mut reg,
            target_pos() + Vec3::new(offset, 0.0, 0.0),
            &reposition_graph(min_travel(0.0)),
            "move",
        );
        ai_tick(&mut reg, &mut runtime, &world, &nav);
        let goal = destination(&reg, enemy).expect("another slot qualifies");
        assert!(
            distance_xz(goal, radial) > 1.0,
            "{case}: the radial slot never qualifies, got {goal:?}"
        );
    }
}

// Row M8 / pin P17: a restore mid-reposition and mid-strafe enters fresh.
#[test]
fn restore_mid_reposition_and_strafe_records_fresh_entry() {
    let floor = OpenFloor::new();
    let world = floor.collision_world();
    let nav = floor.nav_graph();
    let graph = engaged_graph(
        "move",
        vec![
            ("move", reposition_activity(None)),
            ("wait", activity("walk", MotionVerb::Strafe, None)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let player = spawn_player(&mut reg, target_pos());
    let repositioner = spawn_engaged(&mut reg, ring_point(target_pos(), EAST), &graph, "move");
    let strafer = spawn_engaged(
        &mut reg,
        ring_point(target_pos(), EAST + 180.0 * DEG),
        &graph,
        "wait",
    );
    for _ in 0..20 {
        ai_then_steering_tick(&mut reg, &mut runtime, &world, &nav);
    }
    let (entry_position, _) = reposition_entry(&reg, repositioner);
    assert!(
        distance_xz(entry_position, position(&reg, repositioner)) > 0.3,
        "fixture: the repositioner is mid-move"
    );
    // The direction a fresh entry here picks, from a control that never ran.
    let fresh_direction = {
        let mut control = EntityRegistry::new();
        let mut control_runtime = AiRuntime::new();
        spawn_player(&mut control, target_pos());
        let control_strafer = spawn_engaged(&mut control, position(&reg, strafer), &graph, "wait");
        ai_tick(&mut control, &mut control_runtime, &world, &nav);
        strafe_direction(&control, control_strafer)
    };
    set_strafe_direction(&mut reg, strafer, -fresh_direction);

    // Serialize every component and restore into a fresh registry and a
    // fresh AI runtime.
    let kinds = [
        ComponentKind::Transform,
        ComponentKind::PlayerMovement,
        ComponentKind::Mesh,
        ComponentKind::Health,
        ComponentKind::Agent,
        ComponentKind::Brain,
        ComponentKind::EntityState,
    ];
    let mut restored = EntityRegistry::new();
    for id in [player, repositioner, strafer] {
        let saved: Vec<serde_json::Value> = kinds
            .iter()
            .filter_map(|&kind| {
                reg.iter_with_kind(kind)
                    .find(|(entity, _)| *entity == id)
                    .map(|(_, value)| serde_json::to_value(value).expect("component serializes"))
            })
            .collect();
        let restored_id = restored.spawn(Transform::default());
        assert_eq!(restored_id, id, "fixture: restore keeps entity ids");
        for value in saved {
            let value: ComponentValue =
                serde_json::from_value(value).expect("component deserializes");
            restored.set_component_value(restored_id, value).unwrap();
        }
    }
    assert_eq!(memory(&restored, repositioner), EngagedMotionMemory::None);
    assert_eq!(memory(&restored, strafer), EngagedMotionMemory::None);

    let mut restored_runtime = AiRuntime::new();
    let restored_at = position(&restored, repositioner);
    ai_tick(&mut restored, &mut restored_runtime, &world, &nav);
    let (restored_entry, _) = reposition_entry(&restored, repositioner);
    assert_eq!(
        restored_entry, restored_at,
        "the first restored tick records a fresh entry position"
    );
    let goal = destination(&restored, repositioner).expect("a fresh pick, no hold");
    assert!(distance_xz(goal, restored_at) + TOLERANCE >= slot_spacing(R));
    assert_eq!(
        strafe_direction(&restored, strafer),
        fresh_direction,
        "the strafer re-picks its direction by the first-entry rule"
    );
}

// ---------------------------------------------------------------------------
// Facing
// ---------------------------------------------------------------------------

/// The yaw one long tick leaves an enemy at: it starts facing south-west
/// (yaw -135°), travelling +Z (yaw 0) with its target east (yaw 90°), so
/// travel facing, target facing and no facing each land somewhere distinct.
fn yaw_after_one_tick(graph: &BehaviorGraphDescriptor, state: &str) -> f32 {
    yaw_after_one_tick_with(graph, state, |_| {})
}

fn yaw_after_one_tick_with(
    graph: &BehaviorGraphDescriptor,
    state: &str,
    stage: impl FnOnce(&mut BrainComponent),
) -> f32 {
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy_at = Vec3::new(20.0, 0.0, 20.0);
    spawn_player(&mut reg, enemy_at + Vec3::new(3.0, 0.0, 0.0));
    let mut brain = authored_brain(graph, state);
    stage(&mut brain);
    let enemy = spawn_enemy(&mut reg, enemy_at, brain, 50.0);
    // spawn_enemy anchors home here; a position goal needs it elsewhere.
    let mut brain = reg.get_component::<BrainComponent>(enemy).unwrap().clone();
    brain.home_anchor = enemy_at + Vec3::new(0.0, 0.0, 10.0);
    reg.set_component(enemy, brain).unwrap();
    set_enemy_yaw(&mut reg, enemy, -135.0 * DEG);
    set_agent_velocity(&mut reg, enemy, Vec3::new(0.0, 0.0, 3.0));
    run_ai_tick(&mut reg, &mut runtime, SNAP_DT);
    enemy_yaw(&reg, enemy)
}

const FACES_TRAVEL: f32 = 0.0;
const FACES_TARGET: f32 = EAST;
const KEEPS_HEADING: f32 = -135.0 * DEG;

fn single_activity_graph(activity: BehaviorActivityDescriptor) -> BehaviorGraphDescriptor {
    let mut graph = engaged_graph("only", vec![("only", activity)]);
    graph.patrol = Some(PatrolDescriptor {
        points: vec![[0.0, 10.0], [0.0, 20.0]],
        mode: PatrolMode::Loop,
    });
    graph
}

// Row M10: authored facing overrides the verb's default, including over the
// motion an activity's own selector supplies.
#[test]
fn activity_facing_policy_overrides_verb_default() {
    let mut travel_reposition = reposition_activity(None);
    travel_reposition.facing = Some(FacingPolicy::Travel);
    assert!(
        yaw_distance(
            yaw_after_one_tick(&single_activity_graph(travel_reposition), "only"),
            FACES_TRAVEL
        ) <= TOLERANCE,
        "`facing: \"travel\"` on reposition faces travel"
    );

    let mut target_chase = activity("walk", MotionVerb::ChaseTarget, None);
    target_chase.facing = Some(FacingPolicy::Target);
    assert!(
        yaw_distance(
            yaw_after_one_tick(&single_activity_graph(target_chase), "only"),
            FACES_TARGET
        ) <= TOLERANCE,
        "`facing: \"target\"` on chaseTarget faces the target while moving"
    );

    let mut composite = activity("walk", MotionVerb::ChaseTarget, None);
    composite.motion = None;
    composite.facing = Some(FacingPolicy::Target);
    composite.layers = BTreeMap::from([(
        "move".to_string(),
        BehaviorLayerDescriptor::Selector(vec![BehaviorSelectorEntry::Motion(
            MotionVerb::ChaseTarget,
        )]),
    )]);
    assert!(
        yaw_distance(
            yaw_after_one_tick(&single_activity_graph(composite), "only"),
            FACES_TARGET
        ) <= TOLERANCE,
        "an activity's facing covers the motion its own selector supplies"
    );
}

// Row M10: with no `facing`, every existing verb faces exactly as before, and
// the two engaged slot verbs face their target.
#[test]
fn existing_verbs_face_as_before_without_facing() {
    for verb in MotionVerb::ALL {
        let expected = match verb {
            // Engaged pursuit faces its travel while it moves.
            MotionVerb::ChaseTarget => FACES_TRAVEL,
            // Position goals face travel.
            MotionVerb::MoveToAnchor | MotionVerb::MoveToLastKnown | MotionVerb::Patrol => {
                FACES_TRAVEL
            }
            // A bare hold engages nothing, and freeze writes no heading.
            MotionVerb::Hold | MotionVerb::Freeze => KEEPS_HEADING,
            MotionVerb::Strafe | MotionVerb::Reposition => FACES_TARGET,
        };
        let graph = single_activity_graph(activity("walk", verb, None));
        let yaw = yaw_after_one_tick_with(&graph, "only", |brain| {
            brain.last_known_target_pos = Some(Vec3::new(20.0, 0.0, 40.0));
        });
        assert!(
            yaw_distance(yaw, expected) <= TOLERANCE,
            "{verb:?}: expected yaw {} degrees, got {}",
            expected / DEG,
            yaw / DEG
        );
    }
}

// ---------------------------------------------------------------------------
// Directional travel clips
// ---------------------------------------------------------------------------

fn mesh_with_walk_variants() -> MeshComponent {
    let mut mesh = enemy_mesh();
    let states = &mut mesh.animation.as_mut().unwrap().states;
    states.insert("walk_left".to_string(), usable_state("walk_left_clip", 5));
    states.insert("walk_right".to_string(), usable_state("walk_right_clip", 6));
    states.insert(
        "walk_backward".to_string(),
        usable_state("walk_backward_clip", 7),
    );
    mesh
}

/// A strafer facing its target east of it, then travelling along each of
/// `velocities` for one long tick; returns the clip after each tick.
fn clips_for_travel(
    facing: Option<FacingPolicy>,
    mesh: MeshComponent,
    velocities: &[Vec3],
) -> (Vec<String>, AiRuntime) {
    let mut strafe = activity("walk", MotionVerb::Strafe, None);
    strafe.facing = facing;
    let graph = single_activity_graph(strafe);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let enemy_at = Vec3::new(20.0, 0.0, 20.0);
    spawn_player(&mut reg, enemy_at + Vec3::new(3.0, 0.0, 0.0));
    let enemy = spawn_enemy(&mut reg, enemy_at, authored_brain(&graph, "only"), 50.0);
    reg.set_component(enemy, mesh).unwrap();
    let clips = velocities
        .iter()
        .map(|velocity| {
            set_agent_velocity(&mut reg, enemy, *velocity);
            run_ai_tick(&mut reg, &mut runtime, SNAP_DT);
            enemy_animation(&reg, enemy)
        })
        .collect();
    (clips, runtime)
}

// Row M11: under target facing, sideways travel plays the side clip and
// backward travel the backward clip; a left-to-right change switches with no
// activity or stop/resume edge.
#[test]
fn target_facing_travel_selects_directional_variant() {
    // Facing east (+X): +Z is its right, -Z its left, -X behind it.
    let (clips, _) = clips_for_travel(
        None,
        mesh_with_walk_variants(),
        &[
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.5),
        ],
    );
    assert_eq!(clips, ["walk_right", "walk_left", "walk_backward", "walk"]);
}

// A strafe running near the 45° edge keeps its side clip: once on screen it
// holds until travel swings past the hold band, so the clip never restarts
// tick to tick.
#[test]
fn travel_near_the_sector_edge_keeps_the_side_clip() {
    // Facing east (yaw π/2); `degrees` counter-clockwise from it is its left.
    let left_of_facing = |degrees: f32| {
        let yaw = std::f32::consts::FRAC_PI_2 + degrees.to_radians();
        Vec3::new(yaw.sin(), 0.0, yaw.cos()) * 3.0
    };
    let (clips, _) = clips_for_travel(
        None,
        mesh_with_walk_variants(),
        &[
            left_of_facing(48.5),
            left_of_facing(41.0),
            left_of_facing(48.5),
            left_of_facing(38.0),
            left_of_facing(30.0),
        ],
    );
    assert_eq!(
        clips,
        ["walk_left", "walk_left", "walk_left", "walk_left", "walk"]
    );
}

#[test]
fn travel_facing_plays_base_clip() {
    let (clips, _) = clips_for_travel(
        Some(FacingPolicy::Travel),
        mesh_with_walk_variants(),
        &[Vec3::new(0.0, 0.0, 3.0), Vec3::new(-3.0, 0.0, 0.0)],
    );
    assert_eq!(clips, ["walk", "walk"]);
}

#[test]
fn missing_variant_falls_back_to_base_clip() {
    let (clips, runtime) = clips_for_travel(
        None,
        enemy_mesh(),
        &[Vec3::new(0.0, 0.0, 3.0), Vec3::new(-3.0, 0.0, 0.0)],
    );
    assert_eq!(clips, ["walk", "walk"]);
    assert!(
        runtime
            .warned
            .iter()
            .all(|key| !key.starts_with("anim:walk_")),
        "a missing variant never warns: {:?}",
        runtime.warned
    );
}

#[test]
fn travel_variant_latch_persists_with_the_brain() {
    let mut brain = BrainComponent::from_graph(&tuning());
    brain.presented_travel_variant = TravelVariant::Left;
    let value = serde_json::to_value(ComponentValue::Brain(brain)).unwrap();
    let ComponentValue::Brain(restored) = serde_json::from_value(value).unwrap() else {
        panic!("a brain round-trips as a brain");
    };
    assert_eq!(
        restored.presented_travel_variant,
        TravelVariant::Left,
        "a restored brain keeps the variant it presents, so it never strands one"
    );
}

// An actionless strafe or reposition is locomotion: at a standstill it yields
// to the graph's rest animation; one that carries an action keeps its own.
#[test]
fn actionless_engaged_slot_verbs_yield_to_rest_animation_at_standstill() {
    for verb in [MotionVerb::Strafe, MotionVerb::Reposition] {
        let graph = engaged_graph(
            "rest",
            vec![
                ("rest", activity("idle", MotionVerb::Hold, None)),
                ("plain", activity("walk", verb, None)),
                (
                    "acting",
                    activity(
                        "attack",
                        verb,
                        Some(ActionVerb::Attack("shoot".to_string())),
                    ),
                ),
            ],
        );
        let plain = authored_brain(&graph, "plain");
        assert_eq!(animation_for_path(&plain, false), Some("idle"), "{verb:?}");
        assert_eq!(animation_for_path(&plain, true), Some("walk"), "{verb:?}");
        let acting = authored_brain(&graph, "acting");
        assert_eq!(
            animation_for_path(&acting, false),
            Some("attack"),
            "{verb:?}"
        );
    }
}

#[path = "engaged_motion_alloc_tests.rs"]
mod alloc;

#[path = "replan_stress_tests.rs"]
mod replan_stress;
