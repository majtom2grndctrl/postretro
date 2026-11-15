// Clip-timed attacks through the AI tick: `action.at` firing on its clip's
// moment, its cancellations, `brain.animationDone`, and the bind-time warning,
// on clip rigs at 60 Hz.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

#[path = "clip_timed_test_fixtures.rs"]
mod fixtures;

#[path = "clip_timed_shipped_tests.rs"]
mod shipped;

use super::*;
use crate::alloc_probe::AllocSnapshot;
use crate::attack_turns::{AttackBudgets, BudgetTable, TokenBudget, attack_index};
use crate::scripting_systems::hit_zones::instance_phase;
use fixtures::*;
use postretro_entities::components::brain::{
    AttackTokenId, ResolvedDraw, ResolvedDraws, TravelVariant,
};
use postretro_entities::components::mesh::switch_animation_state;
use postretro_foundation::FacingPolicy;

/// One standing enemy seated in `jab` firing `action` on `attack_jab`, whose
/// one-second clip carries `hit` at `event`.
fn jab_scene(event: f32, action: ActionVerb, rows: Vec<GuardedRow>) -> (Scene, EntityId) {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", event)])]);
    let enemy = scene.spawn(0, &jab_graph(action, rows), "jab");
    (scene, enemy)
}

fn hit_jab() -> ActionVerb {
    timed(JAB, "hit")
}

fn mesh_state(scene: &Scene, enemy: EntityId) -> String {
    enemy_animation(&scene.reg, enemy)
}

/// Every token draws from one budget of `limit`, none refreshing.
fn one_token(scene: &mut Scene, limit: u32) {
    scene.runtime.turns.budgets = AttackBudgets::new(BudgetTable::new(vec![TokenBudget {
        limit,
        refresh_ms: 0.0,
    }]));
}

/// Make `attack` on `enemy` draw one of token 0, binding brains as the next
/// tick would.
fn draw_token_zero(scene: &mut Scene, enemy: EntityId, attack: &str) {
    scene
        .runtime
        .programs
        .sync(&scene.reg, &[], 0, &mut HashSet::new());
    let index = attack_index(&scene.brain(enemy).graph, attack).expect("declared attack");
    let draws = ResolvedDraws::from_slice(&[ResolvedDraw {
        token: AttackTokenId(0),
        cost: 1,
    }])
    .unwrap();
    scene
        .runtime
        .programs
        .set_attack_draws_for_test(enemy, index, draws);
}

fn holds_turn(scene: &Scene, enemy: EntityId) -> bool {
    scene.brain(enemy).attack_turns.holds_any()
}

/// An enemy holding `slam`'s turn while seated in `guard`, whose slam never
/// reaches; `rest` holds nothing.
fn blocker_graph() -> BehaviorGraphDescriptor {
    let mut guard = act("idle", Some(untimed("slam")));
    guard.attack_turn = Some("slam".to_string());
    let mut slam = contact_attack(10.0);
    slam.max_range = Some(0.1);
    clip_graph(
        "guard",
        vec![("guard", guard), ("rest", act("idle", None))],
        vec![],
        vec![("slam", slam)],
    )
}

// Fire 1.
#[test]
fn fire1_at_attack_fires_on_the_tick_its_clip_reaches_the_event() {
    let hits = |event, action| jab_scene(event, action, vec![]).0.hits(45);
    assert_eq!(
        hits(0.5, hit_jab()),
        vec![30],
        "`hit` at 0.5 s: not tick 29 after entry, tick 30, a moment on a tick boundary firing on it"
    );
    assert_eq!(hits(0.51, hit_jab()), vec![31], "`hit` at 0.51 s");
    assert_eq!(
        hits(0.5, untimed(JAB)),
        vec![0],
        "without `at` the same graph fires on its entry tick"
    );
}

// Fire 2.
#[test]
fn fire2_event_at_zero_fires_on_the_entry_tick() {
    assert_eq!(jab_scene(0.0, hit_jab(), vec![]).0.hits(45), vec![0]);
}

// Fire 3.
#[test]
fn fire3_out_of_range_at_the_moment_whiffs_until_a_fresh_entry() {
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    let pawn = scene.pawn;
    let hits = scene.hits_with(60, |scene, tick| match tick {
        25 => set_enemy_position(&mut scene.reg, pawn, Vec3::new(5.0, 0.0, 0.0)),
        35 => set_enemy_position(&mut scene.reg, pawn, player_spot()),
        _ => {}
    });
    assert!(
        hits.is_empty(),
        "out of reach at the moment, nothing fires that entry, even back in reach: {hits:?}"
    );
    assert_eq!(scene.state(enemy), "jab", "fixture: the entry lasts");

    // A fresh entry on the clip, through another state, fires at its moment.
    scene.enter(enemy, "stalk");
    scene.tick();
    scene.enter(enemy, "jab");
    assert_eq!(scene.hits(45), vec![30]);
}

// Fire 4.
#[test]
fn fire4_no_turn_at_the_moment_whiffs_and_a_turn_granted_that_tick_fires() {
    for release_on in [40, 30] {
        let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
        one_token(&mut scene, 1);
        let blocker = scene.spawn(0, &blocker_graph(), "guard");
        let jabber = scene.spawn(1, &jab_graph(hit_jab(), vec![]), "jab");
        draw_token_zero(&mut scene, blocker, "slam");
        draw_token_zero(&mut scene, jabber, JAB);
        let hits = scene.hits_with(60, |scene, tick| {
            if tick == 1 {
                assert!(holds_turn(scene, blocker), "fixture: the blocker holds");
                assert!(!holds_turn(scene, jabber), "fixture: the jabber waits");
            }
            if tick == release_on {
                scene.enter(blocker, "rest");
            }
        });
        assert!(
            holds_turn(&scene, jabber),
            "the jabber is granted in the end"
        );
        if release_on == 30 {
            assert_eq!(
                hits,
                vec![30],
                "a turn granted in the moment tick's own arbitration fires on that tick"
            );
        } else {
            assert!(
                hits.is_empty(),
                "no turn at the moment: a whiff, and nothing once granted: {hits:?}"
            );
        }
    }
}

// Fire 5.
#[test]
fn fire5_leaving_before_the_moment_fires_nothing() {
    // Leaving 200 ms in, long before the moment.
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![edge("stalk", after_ms(199.0))]);
    assert!(scene.hits(45).is_empty());
    assert_eq!(scene.state(enemy), "stalk");

    // An exit row passing on the moment's own tick: transitions run first.
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![edge("stalk", after_ms(499.0))]);
    let mut states = Vec::new();
    let hits = scene.hits_with(45, |scene, _| states.push(scene.state(enemy)));
    states.push(scene.state(enemy));
    assert_eq!(
        states[30], "jab",
        "fixture: still in the jab before tick 30"
    );
    assert_eq!(states[31], "stalk", "fixture: the exit passes on tick 30");
    assert!(
        hits.is_empty(),
        "the path left on the moment tick: {hits:?}"
    );
}

// Fire 6.
#[test]
fn fire6_a_last_tick_moment_fires_before_an_animation_done_exit() {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 0.5, &[("hit", 0.5)])]);
    let enemy = scene.spawn(
        0,
        &jab_graph(hit_jab(), vec![edge("stalk", guard_done())]),
        "jab",
    );
    let mut after = Vec::new();
    let hits = scene.hits_with(40, |scene, _| after.push(scene.state(enemy)));
    after.push(scene.state(enemy));
    assert_eq!(hits, vec![30], "the moment on the clip's last tick fires");
    assert_eq!(
        after[31], "jab",
        "`animationDone` is false on the moment tick"
    );
    assert_eq!(
        after[32], "stalk",
        "`animationDone` turns true on the next tick, and the exit leaves after the fire"
    );
}

/// `windup` and `jab` share the one-shot `attack_jab`; `windup` enters `jab`
/// after `ms`.
fn windup_scene(ms: f32) -> (Scene, EntityId) {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
    let graph = clip_graph(
        "windup",
        vec![
            ("windup", act("attack_jab", None)),
            ("jab", act("attack_jab", Some(hit_jab()))),
        ],
        vec![("windup", vec![edge("jab", after_ms(ms))])],
        vec![(JAB, contact_attack(10.0))],
    );
    let enemy = scene.spawn(0, &graph, "windup");
    (scene, enemy)
}

// Fire 7.
#[test]
fn fire7_entry_on_a_shared_one_shot_past_the_moment_fires_nothing_and_on_it_fires() {
    // Entered on tick 36, 0.6 s into the clip: past the moment.
    let (mut scene, enemy) = windup_scene(599.0);
    assert!(scene.hits(60).is_empty());
    assert_eq!(scene.state(enemy), "jab", "fixture: the jab was entered");

    // Entered on tick 30, the tick the clip reaches the moment.
    let (mut scene, _) = windup_scene(499.0);
    assert_eq!(scene.hits(60), vec![30], "fires on that entry tick");
}

// Fire 8.
#[test]
fn fire8_a_second_activity_on_one_state_measures_from_the_first_entry() {
    // `jab` is entered on tick 12; its moment is 0.5 s from `windup`'s entry.
    let (mut scene, _) = windup_scene(199.0);
    assert_eq!(
        scene.hits(60),
        vec![30],
        "not tick 42, 30 ticks after its own entry"
    );
}

/// `aim` and `shoot` share the one-second `aim_loop` carrying `kick` at
/// 0.25 s; `aim` enters `shoot` on tick 72. Returns the scene, the enemy, and
/// the first tick after the entry on which the loop passes the moment.
fn aim_scene() -> (Scene, EntityId, usize) {
    let mut scene = Scene::new(vec![looping("aim_loop", 1.0, &[("kick", 0.25)])]);
    let graph = clip_graph(
        "aim",
        vec![
            ("aim", act("aim_loop", None)),
            ("shoot", act("aim_loop", Some(timed(JAB, "kick")))),
        ],
        vec![("aim", vec![edge("shoot", after_ms(1199.0))])],
        vec![(JAB, contact_attack(10.0))],
    );
    let enemy = scene.spawn(0, &graph, "aim");
    let passes = loop_pass_ticks(instance_phase(enemy.to_raw(), 1.0), 0.25, 1.0, 200);
    assert!(passes[0] < 72, "fixture: `aim` plays past the moment");
    let next = *passes.iter().find(|&&tick| tick >= 72).unwrap();
    assert_ne!(next, 72, "fixture: no pass on the entry tick");
    (scene, enemy, next)
}

// Fire 9: a loop entered past the moment fires at its next pass.
#[test]
fn fire9_a_loop_entered_past_the_moment_fires_at_the_next_pass() {
    let (mut scene, enemy, next) = aim_scene();
    let entry = 72;
    let mut states = Vec::new();
    let hits = scene.hits_with(200, |scene, _| states.push(scene.state(enemy)));
    assert_eq!(
        states[entry], "aim",
        "fixture: `shoot` is entered on tick {entry}"
    );
    assert_eq!(states[entry + 1], "shoot");
    assert_eq!(
        hits,
        vec![next],
        "the next pass from the drawn phase, not entry"
    );
}

/// A target-facing `shoot` on `base` firing `hit_jab()`, whose travel variants
/// play their own clips.
fn strafing_graph(base: &str) -> BehaviorGraphDescriptor {
    let mut shoot = act(base, Some(hit_jab()));
    shoot.facing = Some(FacingPolicy::Target);
    clip_graph(
        "shoot",
        vec![("shoot", shoot)],
        vec![],
        vec![(JAB, contact_attack(10.0))],
    )
}

/// One enemy seated in [`strafing_graph`]'s `shoot`.
fn strafing_scene(specs: Vec<ClipSpec>, base: &'static str) -> (Scene, EntityId) {
    let mut scene = Scene::new(specs);
    let enemy = scene.spawn(0, &strafing_graph(base), "shoot");
    (scene, enemy)
}

/// Travel along +Z while facing the player along +X: the right-hand variant.
fn travel_right(scene: &mut Scene, enemy: EntityId) {
    scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(0.0, 0.0, 2.0));
}

// Fire 9: a looping travel variant restarts clip time at its drawn phase.
#[test]
fn fire9_a_looping_travel_variant_restarts_at_its_drawn_phase() {
    let (mut scene, enemy) = strafing_scene(
        vec![
            looping("strike_loop", 1.0, &[("hit", 0.25)]),
            looping("strike_loop_right", 0.8, &[("hit", 0.6)]),
        ],
        "strike_loop",
    );
    let phase = instance_phase(enemy.to_raw(), 0.8);
    let expected = 1 + loop_pass_ticks(phase, 0.6, 0.8, 120)[0];
    let hits = scene.hits_with(120, |scene, tick| {
        if tick == 1 {
            travel_right(scene, enemy);
        }
    });
    assert_eq!(mesh_state(&scene, enemy), "strike_loop_right");
    assert_eq!(
        hits,
        vec![expected],
        "the variant's moment, from its drawn phase at the switch"
    );
}

// Fire 10.
#[test]
fn fire10_an_impact_switch_cancels_the_moment_even_once_restored() {
    // Switched off and back within one gap between AI ticks.
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    let hits = scene.hits_with(45, |scene, tick| {
        if tick == 10 {
            scene.switch(enemy, "hit_reaction");
            scene.switch(enemy, "attack_jab");
        }
    });
    assert!(hits.is_empty(), "off and back in one gap: {hits:?}");
    assert_eq!(scene.state(enemy), "jab", "the activity is unchanged");

    // Restored later.
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    let hits = scene.hits_with(45, |scene, tick| match tick {
        10 => scene.switch(enemy, "hit_reaction"),
        20 => scene.switch(enemy, "attack_jab"),
        _ => {}
    });
    assert!(hits.is_empty(), "restored before the moment: {hits:?}");

    // No switch: the same swing fires.
    let (mut scene, _) = jab_scene(0.5, hit_jab(), vec![]);
    assert_eq!(scene.hits(45), vec![30]);
}

// Fire 11.
#[test]
fn fire11_switches_before_the_entry_or_after_the_moment_compute_keep_the_swing() {
    // An impact switch before AI on the entry tick (6) cancels only the
    // entry being left: the entry's own switch draws over it.
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
    let graph = jab_graph(hit_jab(), vec![]);
    let mut graph = graph;
    graph
        .envelope
        .transitions
        .insert("stalk".to_string(), vec![edge("jab", after_ms(99.0))]);
    let enemy = scene.spawn(0, &graph, "stalk");
    let mut states = Vec::new();
    let hits = scene.hits_with(45, |scene, tick| {
        if tick == 6 {
            scene.switch(enemy, "hit_reaction");
        }
        states.push(scene.state(enemy));
    });
    assert_eq!(states[6], "stalk", "fixture: the jab is entered on tick 6");
    assert_eq!(hits, vec![36], "the entry's moment still fires");

    // An earlier slot's contact hit switches the enemy after its compute on
    // the moment tick.
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
    let striker = clip_graph(
        "wait",
        vec![
            ("wait", act("idle", None)),
            ("strike", act("idle", Some(untimed("slam")))),
        ],
        vec![("wait", vec![edge("strike", after_ms(499.0))])],
        vec![("slam", contact_attack(10.0))],
    );
    scene.spawn(0, &striker, "wait");
    let jabber = scene.spawn(1, &jab_graph(hit_jab(), vec![]), "jab");
    let mut lost = Vec::new();
    for tick in 0..45 {
        let hp = player_hp(&scene.reg, scene.pawn);
        scene.tick_with_impact(|reg| {
            let _ = switch_animation_state(reg, jabber, "hit_reaction");
        });
        lost.push((tick, hp - player_hp(&scene.reg, scene.pawn)));
        if tick == 30 {
            assert_eq!(
                mesh_state(&scene, jabber),
                "hit_reaction",
                "fixture: the earlier slot's hit switched the jabber this tick"
            );
        }
    }
    let hit_ticks: Vec<_> = lost.iter().filter(|(_, lost)| *lost > 0.0).collect();
    assert_eq!(
        hit_ticks,
        vec![&(30, 2.0 * JAB_DAMAGE)],
        "both swings land on tick 30: the jabber's fire stands"
    );

    // A hitscan in the weapon stage, after the AI stage of the moment tick.
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    let hits = scene.hits_with(45, |scene, tick| {
        if tick == 31 {
            scene.switch(enemy, "hit_reaction");
        }
    });
    assert_eq!(hits, vec![30], "the moment tick's fire stands, and only it");
}

/// The jab scene launched on tick 5 for `launched` ticks, airborne while
/// launched when `airborne`, on a mesh with or without `jump_loop`.
fn launched_hits(jump_loop: bool, launched: usize, airborne: bool) -> (Vec<usize>, Vec<String>) {
    let mut specs = vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])];
    if jump_loop {
        specs.push(looping("jump_loop", 1.0, &[]));
    }
    let mut scene = Scene::new(specs);
    let enemy = scene.spawn(0, &jab_graph(hit_jab(), vec![]), "jab");
    let mut clips = Vec::new();
    let hits = scene.hits_with(60, |scene, tick| {
        if tick == 5 {
            scene.set_agent(enemy, |agent| {
                agent.knockback_velocity = Vec3::new(2.0, 0.0, 0.0);
                if airborne {
                    agent.ground = postretro_foundation::GroundRef::Airborne;
                }
            });
        }
        if tick == 5 + launched {
            scene.set_agent(enemy, |agent| {
                agent.knockback_velocity = Vec3::ZERO;
                agent.ground = postretro_foundation::GroundRef::World;
            });
        }
        clips.push(mesh_state(scene, enemy));
    });
    (hits, clips)
}

// Fire 12.
#[test]
fn fire12_a_knockback_launch_cancels_the_moment() {
    let (hits, clips) = launched_hits(true, 20, true);
    assert!(
        clips.iter().any(|clip| clip == "jump_loop"),
        "fixture: the launch shows `jump_loop`"
    );
    assert!(hits.is_empty(), "landed before the moment: {hits:?}");

    let (hits, _) = launched_hits(true, 40, true);
    assert!(hits.is_empty(), "airborne at the moment: {hits:?}");

    let (hits, clips) = launched_hits(false, 20, true);
    assert!(clips.iter().all(|clip| clip != "jump_loop"));
    assert!(hits.is_empty(), "a mesh without `jump_loop`: {hits:?}");

    let (hits, clips) = launched_hits(true, 3, true);
    assert!(
        clips.iter().all(|clip| clip != "jump_loop"),
        "fixture: shorter than the fall delay"
    );
    assert!(
        hits.is_empty(),
        "a launch shorter than the fall delay: {hits:?}"
    );

    let (hits, _) = launched_hits(true, 3, false);
    assert!(hits.is_empty(), "a grounded shove: {hits:?}");

    let (hits, _) = launched_hits(true, 0, false);
    assert_eq!(hits, vec![30], "fixture: unlaunched, the swing fires");
}

// Fire 13.
#[test]
fn fire13_a_travel_variant_switch_restarts_at_the_variant_clips_event() {
    let specs = || {
        vec![
            one_shot("strike", 1.0, &[("hit", 0.5)]),
            one_shot("strike_right", 1.0, &[("hit", 0.3)]),
        ]
    };
    let (mut scene, enemy) = strafing_scene(specs(), "strike");
    let hits = scene.hits_with(60, |scene, tick| {
        if tick == 10 {
            travel_right(scene, enemy);
        }
    });
    assert_eq!(mesh_state(&scene, enemy), "strike_right");
    assert_eq!(hits, vec![28], "0.3 s after the switch on tick 10");

    let (mut scene, enemy) = strafing_scene(specs(), "strike");
    assert_eq!(
        scene.hits(60),
        vec![30],
        "fixture: unswitched, the base clip"
    );
    assert_eq!(mesh_state(&scene, enemy), "strike");
}

// Fire 14.
#[test]
fn fire14_clip_time_runs_at_the_rate_the_previous_pose_stage_set() {
    let rate_hits = |rate: fn(usize) -> f32| {
        let mut scene = Scene::new(vec![one_shot("walk", 2.0, &[("hit", 0.5)])]);
        let graph = clip_graph(
            "advance",
            vec![("advance", act("walk", Some(hit_jab())))],
            vec![],
            vec![(JAB, contact_attack(10.0))],
        );
        let enemy = scene.spawn(0, &graph, "advance");
        scene.hits_with(45, |scene, tick| {
            // The pose stage after tick `tick - 1` sets this tick's rate.
            if tick > 0 {
                scene.set_rate(enemy, rate(tick));
            }
        })
    };
    assert_eq!(
        rate_hits(|_| 1.5),
        vec![20],
        "at 1.5× the moment comes at two-thirds of its time"
    );
    assert_eq!(
        rate_hits(|tick| if tick <= 10 { 1.0 } else { 2.0 }),
        vec![20],
        "each tick advances by the rate the previous tick's pose stage set"
    );
}

// Fire 15.
#[test]
fn fire15_a_loop_fires_once_per_entry_at_the_next_pass_from_its_drawn_phase() {
    let shoot = || {
        clip_graph(
            "shoot",
            vec![("shoot", act("fire_loop", Some(timed(JAB, "kick"))))],
            vec![],
            vec![(JAB, contact_attack(10.0))],
        )
    };
    let run = |fillers: usize| {
        let mut scene = Scene::new(vec![looping("fire_loop", 1.0, &[("kick", 0.25)])]);
        for _ in 0..fillers {
            scene.reg.spawn(Transform::default());
        }
        let enemy = scene.spawn(0, &shoot(), "shoot");
        let passes = loop_pass_ticks(instance_phase(enemy.to_raw(), 1.0), 0.25, 1.0, 240);
        assert!(passes.len() >= 3, "fixture: the loop passes several times");
        (passes[0], scene.hits(240))
    };
    let (first, hits) = run(0);
    assert_eq!(
        hits,
        vec![first],
        "once, at the first pass from the drawn phase"
    );
    let (other, other_hits) = (1..16)
        .map(run)
        .find(|(other, _)| *other != first)
        .expect("fixture: another enemy draws another phase");
    assert_eq!(
        other_hits,
        vec![other],
        "a different phase fires on its own tick"
    );
}

/// A projectile weapon whose action fires `shots` shots `wait_ms` apart.
fn burst_weapon(name: &str, shots: usize, wait_ms: f32) -> EntityTypeDescriptor {
    let mut descriptor = projectile_weapon_descriptor(name, 4.0, 1.0, 300.0);
    let mut steps = Vec::new();
    for shot in 0..shots {
        if shot > 0 {
            steps.push(serde_json::json!({ "kind": "wait", "durationMs": wait_ms }));
        }
        steps.push(serde_json::json!({ "kind": "shot" }));
    }
    descriptor.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": 300, "steps": steps,
    }))
    .expect("a valid burst action");
    descriptor
}

// Fire 16.
#[test]
fn fire16_a_weapon_attack_starts_its_activation_at_the_moment() {
    let shots = |action: ActionVerb| {
        let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
        scene.descriptors = vec![burst_weapon("enemy.rifle", 3, 100.0)];
        let mut volley = contact_attack(0.0);
        volley.weapon = Some("enemy.rifle".to_string());
        (volley.damage, volley.max_range, volley.cooldown_ms) = (None, None, None);
        let graph = clip_graph(
            "shoot",
            vec![("shoot", act("attack_jab", Some(action)))],
            vec![],
            vec![("volley", volley)],
        );
        scene.spawn(0, &graph, "shoot");
        (0..80)
            .filter(|_| !scene.tick().projectile_spawns.is_empty())
            .collect::<Vec<usize>>()
    };
    let on_entry = shots(untimed("volley"));
    let at_moment = shots(timed("volley", "hit"));
    assert_eq!(
        on_entry.len(),
        3,
        "fixture: a three-shot burst: {on_entry:?}"
    );
    assert_eq!(
        on_entry[0], 0,
        "fixture: untimed, the burst starts on entry"
    );
    assert_eq!(at_moment[0], 30, "the activation starts at the moment");
    assert_eq!(
        at_moment,
        on_entry.iter().map(|tick| tick + 30).collect::<Vec<_>>(),
        "later shots keep the weapon's steps"
    );
}

// Fire 17.
#[test]
fn fire17_a_missing_event_warns_once_per_class_and_state_and_fires_on_entry() {
    let capture = LogCapture::start();
    let lacks = "`brute` animation state `attack_jab`: clip `attack_jab_clip` has no such event";

    // A resolved clip lacking the name: two enemies of one class, one warning.
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("swing", 0.2)])]);
    let graph = jab_graph(hit_jab(), vec![]);
    let first = scene.spawn(0, &graph, "jab");
    scene.spawn(1, &graph, "jab");
    assert_eq!(scene.hits(20), vec![0], "both fire on entry");
    assert_eq!(scene.brain(first).activity_attack_count(0), Some(1));
    capture.assert_logged_once(Level::Warn, lacks);
    capture.clear();

    // A travel variant's clip lacking it, drawn on entry.
    let (mut scene, enemy) = strafing_scene(
        vec![
            one_shot("attack_jab", 1.0, &[("hit", 0.5)]),
            one_shot("attack_jab_right", 1.0, &[]),
        ],
        "attack_jab",
    );
    travel_right(&mut scene, enemy);
    assert_eq!(
        scene.hits(20),
        vec![0],
        "the variant drawn on entry fires on it"
    );
    assert_eq!(mesh_state(&scene, enemy), "attack_jab_right");
    capture.assert_logged_once(
        Level::Warn,
        "`brute` animation state `attack_jab_right`: clip `attack_jab_right_clip` has no such event",
    );
    capture.assert_not_logged(Level::Warn, lacks);
    capture.clear();

    // A state whose clip did not resolve.
    let mut scene = Scene::new(vec![unresolved("attack_jab")]);
    scene.spawn(0, &jab_graph(hit_jab(), vec![]), "jab");
    assert_eq!(scene.hits(20), vec![0]);
    capture.assert_logged_once(
        Level::Warn,
        "`brute` animation state `attack_jab`: clip `attack_jab_clip` did not resolve",
    );
}

// The bind-time check reads a variant's clip by name as well as by index, so a
// runtime spawn bound before its indices fill is checked as it will draw.
// Regression: every variant was skipped while its index was unfilled.
#[test]
fn a_missing_event_warns_for_a_variant_bound_before_its_clip_index_fills() {
    let capture = LogCapture::start();
    let (mut scene, enemy) = strafing_scene(
        vec![
            one_shot("attack_jab", 1.0, &[("hit", 0.5)]),
            one_shot("attack_jab_right", 1.0, &[]),
        ],
        "attack_jab",
    );
    let mut mesh = scene
        .reg
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .clone();
    for state in mesh.animation.as_mut().unwrap().states.values_mut() {
        state.clip_index = None;
    }
    scene.reg.set_component(enemy, mesh).unwrap();
    scene.tick();
    capture.assert_logged_once(
        Level::Warn,
        "`brute` animation state `attack_jab_right`: clip `attack_jab_right_clip` has no such event",
    );
}

// Fire 18.
#[test]
fn fire18_the_implied_mark_holds_the_turn_from_entry() {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
    one_token(&mut scene, 1);
    let graph = jab_graph(hit_jab(), vec![]);
    assert_eq!(
        graph.envelope.activities["jab"].attack_turn.as_deref(),
        Some(JAB),
        "fixture: `at` implies the mark descriptor load writes"
    );
    let jabber = scene.spawn(0, &graph, "jab");
    let waiter = scene.spawn(1, &blocker_graph(), "guard");
    draw_token_zero(&mut scene, jabber, JAB);
    draw_token_zero(&mut scene, waiter, "slam");
    let hits = scene.hits_with(31, |scene, tick| {
        if tick > 0 {
            assert!(holds_turn(scene, jabber), "tick {tick}: held from entry");
            assert!(
                !holds_turn(scene, waiter),
                "tick {tick}: the waiter is not granted before the moment"
            );
            assert!(scene.brain(waiter).attack_turns.request.is_some());
        }
    });
    assert_eq!(hits, vec![30]);
}

// End 1.
#[test]
fn end1_animation_done_turns_true_the_tick_after_the_clip_reaches_its_duration() {
    let leave_tick = |spec: ClipSpec| {
        let mut scene = Scene::new(vec![spec]);
        let graph = clip_graph(
            "play",
            vec![
                ("play", act("attack_jab", None)),
                ("done", act("idle", None)),
            ],
            vec![("play", vec![edge("done", guard_done())])],
            vec![],
        );
        let enemy = scene.spawn(0, &graph, "play");
        (0..200).find(|_| {
            scene.tick();
            scene.state(enemy) == "done"
        })
    };
    assert_eq!(
        leave_tick(one_shot("attack_jab", 0.5, &[])),
        Some(31),
        "false through tick 30, which reaches the duration; true on 31"
    );
    assert_eq!(
        leave_tick(looping("attack_jab", 0.5, &[])),
        None,
        "false throughout a loop"
    );
    assert_eq!(
        leave_tick(unresolved("attack_jab")),
        Some(0),
        "true for a state whose clip did not resolve"
    );
}

// End 2.
#[test]
fn end2_an_entered_one_shot_reads_not_done_and_fires_its_own_moment() {
    let mut scene = Scene::new(vec![
        one_shot("flinch", 0.2, &[]),
        one_shot("attack_jab", 1.0, &[("hit", 0.5)]),
    ]);
    let graph = clip_graph(
        "flinch",
        vec![
            ("flinch", act("flinch", None)),
            ("jab", act("attack_jab", Some(hit_jab()))),
            ("stalk", act("walk", None)),
        ],
        vec![
            ("flinch", vec![edge("jab", guard_done())]),
            ("jab", vec![edge("stalk", guard_done())]),
        ],
        vec![(JAB, contact_attack(10.0))],
    );
    let enemy = scene.spawn(0, &graph, "flinch");
    let mut after = Vec::new();
    let hits = scene.hits_with(90, |scene, _| after.push(scene.state(enemy)));
    after.push(scene.state(enemy));
    // `flinch` reaches 0.2 s on tick 12, so `jab` is entered on tick 13.
    assert_eq!(after[13], "flinch");
    assert_eq!(after[14], "jab", "fixture: entered on tick 13");
    assert_eq!(after[15], "jab", "its first evaluated tick reads not done");
    assert_eq!(hits, vec![43], "its own clip's moment, 30 ticks in");
    assert_eq!(after[74], "jab");
    assert_eq!(
        after[75], "stalk",
        "leaves the tick after its own clip ends"
    );
}

// End 3.
#[test]
fn end3_a_cancelled_moment_still_reaches_clip_end() {
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![edge("stalk", guard_done())]);
    let mut after = Vec::new();
    let hits = scene.hits_with(80, |scene, tick| {
        if tick == 10 {
            scene.switch(enemy, "hit_reaction");
        }
        after.push(scene.state(enemy));
    });
    after.push(scene.state(enemy));
    assert!(hits.is_empty(), "the impact cancelled the moment");
    assert_eq!(after[61], "jab");
    assert_eq!(
        after[62], "stalk",
        "clip time ran on for the path's state: done the tick after its 1 s"
    );
}

/// `engage` (on `walk`) nests `windup` (on looping `idle`) entering `jab`
/// (on the 0.5 s one-shot) on tick 10. `parent` puts the `animationDone` row
/// on `engage`; otherwise on `jab`.
fn nested_done_tick(parent: bool) -> Option<usize> {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 0.5, &[])]);
    let done = vec![edge("out", guard_done())];
    let mut inner = BTreeMap::from([("windup".to_string(), vec![edge("jab", after_ms(166.0))])]);
    let mut outer = BTreeMap::new();
    if parent {
        outer.insert("engage".to_string(), done);
    } else {
        inner.insert("jab".to_string(), done);
    }
    let offense = BehaviorGraphEnvelope {
        initial: "windup".to_string(),
        activities: BTreeMap::from([
            ("windup".to_string(), act("idle", None)),
            ("jab".to_string(), act("attack_jab", None)),
            ("out".to_string(), act("idle", None)),
        ]),
        transitions: inner,
    };
    let mut engage = act("walk", None);
    engage.layers = BTreeMap::from([(
        "offense".to_string(),
        BehaviorLayerDescriptor::Graph(offense),
    )]);
    let mut graph = clip_graph(
        "engage",
        vec![("engage", engage), ("out", act("idle", None))],
        vec![],
        vec![],
    );
    graph.envelope.transitions = outer;
    let enemy = scene.spawn(0, &graph, "engage");
    (0..120).find(|_| {
        scene.tick();
        scene.state(enemy).ends_with("out")
    })
}

// End 4.
#[test]
fn end4_a_parent_and_its_leaf_read_one_animation_done_on_a_tick() {
    // The leaf entered its clip 10 ticks after the parent's entry, so a
    // scope-relative fact would disagree.
    let parent = nested_done_tick(true);
    assert_eq!(
        parent,
        Some(41),
        "the parent leaves the tick after the leaf's clip"
    );
    assert_eq!(
        nested_done_tick(false),
        parent,
        "the leaf reads the same value"
    );
}

// The brief's Scripting surface example as a fixture: its descriptor parses
// to the graph the runtime fires and leaves on.
#[test]
fn surf1_the_surface_example_jab_fires_at_hit_and_leaves_on_animation_done() {
    const SOURCE: &str = r#"({ components: { behavior: {
        initial: "stalk",
        moveSpeed: 3,
        attacks: { jab: { damage: 8, maxRange: 2, cooldownMs: 1200 } },
        activities: {
            stalk: { animation: "walk" },
            jab: { animation: "attack_jab", action: { attack: "jab", at: "hit" } }
        },
        transitions: {
            stalk: [{ to: "jab", when: { op: "and",
                a: { op: "le", a: { op: "input", name: "@brain.targetDistance" }, b: { op: "const", value: 2 } },
                b: { op: "input", name: "@attackTurnFree.jab" } } }],
            jab: [{ to: "stalk", when: { op: "input", name: "@brain.animationDone" } }]
        }
    } } })"#;
    let runtime = rquickjs::Runtime::new().expect("JS runtime");
    let context = rquickjs::Context::full(&runtime).expect("JS context");
    let graph = context.with(|ctx| {
        let value = ctx.eval(SOURCE).expect("the example evaluates");
        postretro_scripting_core::data_descriptors::entity_descriptor_from_js(&ctx, value)
            .expect("the example parses")
            .behavior
            .expect("the example carries a behavior")
    });
    assert_eq!(
        graph.envelope.activities["jab"].attack_turn.as_deref(),
        Some(JAB),
        "`at` implies the jab's mark"
    );
    // `attack_jab` plays `punch`, whose `hit` the strike cut inherits.
    let mut scene = Scene::new(vec![playing(
        "punch",
        one_shot("attack_jab", 0.4, &[("hit", 0.153)]),
    )]);
    let enemy = scene.spawn(0, &graph, "stalk");
    let mut after = Vec::new();
    let hits = scene.hits_with(26, |scene, _| after.push(scene.state(enemy)));
    after.push(scene.state(enemy));
    assert_eq!(after[1], "jab", "fixture: the jab is entered on tick 0");
    assert_eq!(hits, vec![10], "fires as `punch` reaches `hit` (0.153 s)");
    assert_eq!(after[25], "jab");
    assert_eq!(
        after[26], "stalk",
        "leaves once the punch has played through"
    );
}

/// Pick `enemy` up into a fresh holder's inventory: the pickup takes its mesh
/// out of the world. Returns the holder.
fn pick_up(scene: &mut Scene, enemy: EntityId) -> EntityId {
    let holder = scene.reg.spawn(Transform::default());
    let mut inventory = postretro_entities::components::inventory::Inventory::default();
    inventory.wieldables[9] = Some(enemy);
    scene.reg.set_component(holder, inventory).unwrap();
    scene.reg.remove_component::<MeshComponent>(enemy).unwrap();
    holder
}

/// Drop `enemy` from `holder` as the sim's drop does: a fresh mesh drawing its
/// default state, its switch count back at zero and no clip index filled yet,
/// handed to the brain. The frame loop fills the indices only after the drop's
/// tick: [`resolve_dropped`].
fn drop_from(scene: &mut Scene, enemy: EntityId, holder: EntityId) {
    scene
        .reg
        .remove_component::<postretro_entities::components::inventory::Inventory>(holder)
        .unwrap();
    let mut mesh = scene.mesh.clone();
    for state in mesh.animation.as_mut().unwrap().states.values_mut() {
        state.clip_index = None;
    }
    scene.reg.set_component(enemy, mesh).unwrap();
    postretro_entities::components::brain::note_replaced_mesh(&mut scene.reg, enemy);
}

/// Fill a dropped mesh's clip indices with the resolve the frame loop runs
/// after the drop's tick.
fn resolve_dropped(scene: &mut Scene, enemy: EntityId) {
    crate::sim::resolve_mesh_entity_bindings_for_entities(
        &mut scene.reg,
        &scene.clip_tables,
        &scene.clips,
        [enemy],
    );
}

/// The tick the held jab is dropped on.
const DROP_TICK: usize = 30;

/// The jab scene held from tick 10 and dropped on [`DROP_TICK`], its clip
/// indices filling after that tick, calling `extra` ahead of each tick after
/// the hold, drop and fill.
fn held_jab_hits(mut extra: impl FnMut(&mut Scene, EntityId, usize)) -> Vec<usize> {
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    let mut holder = None;
    scene.hits_with(70, |scene, tick| {
        match tick {
            10 => holder = Some(pick_up(scene, enemy)),
            DROP_TICK => drop_from(scene, enemy, holder.take().expect("held")),
            _ if tick == DROP_TICK + 1 => resolve_dropped(scene, enemy),
            _ => {}
        }
        extra(scene, enemy, tick);
    })
}

// A held enemy leaves the AI set, so its clip time freezes with its activity
// clocks. Its drop's fresh mesh is no foreign switch: the enemy draws its
// swing again on the first tick after the mesh's clip indices fill (the tick
// after the drop), and the moment is timed from that redraw. Regression: that
// mesh's switch count, back at zero, read as a foreign switch and cancelled
// the swing.
#[test]
fn held_enemy_freezes_clip_time_and_fires_on_release() {
    let mut drawn = Vec::new();
    let hits = held_jab_hits(|scene, enemy, tick| {
        if tick == DROP_TICK + 1 || tick == DROP_TICK + 2 {
            drawn.push(mesh_state(scene, enemy));
        }
    });
    assert_eq!(
        drawn,
        ["idle", "attack_jab"],
        "fixture: the drop tick leaves the fresh mesh's default state, and the next tick \
         redraws the swing"
    );
    // The redraw on the tick after the drop restarts clip time at 0; the
    // moment at 0.5 s is 30 ticks later.
    let redraw_tick = DROP_TICK + 1;
    assert_eq!(hits, vec![redraw_tick + 30], "0.5 s after the redraw");
}

// A switch on the dropped enemy's fresh mesh before its redraw tick's AI (a
// projectile flinch) is foreign: the swing is cancelled and the flinch stays
// on screen. A flinch cannot land on the drop tick itself, whose fresh mesh
// has no clip index filled. Regressions: with the brain's last own switch its
// first, the fresh mesh's first switch matched the count it had seen; and the
// pending redraw switched the mesh back onto the swing and restarted its
// clock, so the flinch never showed.
#[test]
fn a_flinch_on_a_dropped_enemys_fresh_mesh_before_the_ai_cancels_the_swing() {
    let mut drawn = Vec::new();
    let hits = held_jab_hits(|scene, enemy, tick| match tick {
        9 => assert_eq!(
            scene.brain(enemy).clip_clock.mesh_switches,
            Some(1),
            "fixture: the brain's own switch so far is the mesh's first"
        ),
        _ if tick == DROP_TICK + 1 => scene.switch(enemy, "hit_reaction"),
        _ if tick == DROP_TICK + 2 => drawn.push(mesh_state(scene, enemy)),
        _ => {}
    });
    assert_eq!(drawn, ["hit_reaction"], "the flinch shows after apply");
    assert!(
        hits.is_empty(),
        "the flinched swing never connects: {hits:?}"
    );
}

// An earlier slot's contact hit flinches the dropped enemy inside the apply
// batch on its redraw tick, after its compute: the flinch stays on screen, and
// the swing never connects.
#[test]
fn a_contact_flinch_on_the_redraw_tick_wins_over_the_redraw() {
    let run = |flinch: bool| {
        let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
        // Enters `strike` and swings on the redraw tick (31): 31 ticks of
        // 16.67 ms is 516.7 ms.
        let striker = clip_graph(
            "wait",
            vec![
                ("wait", act("idle", None)),
                ("strike", act("idle", Some(untimed("slam")))),
            ],
            vec![("wait", vec![edge("strike", after_ms(510.0))])],
            vec![("slam", contact_attack(10.0))],
        );
        scene.spawn(0, &striker, "wait");
        let jabber = scene.spawn(1, &jab_graph(hit_jab(), vec![]), "jab");
        let mut holder = None;
        let mut hits = Vec::new();
        let mut drawn = None;
        for tick in 0..80 {
            match tick {
                10 => holder = Some(pick_up(&mut scene, jabber)),
                DROP_TICK => drop_from(&mut scene, jabber, holder.take().expect("held")),
                _ if tick == DROP_TICK + 1 => resolve_dropped(&mut scene, jabber),
                _ => {}
            }
            let hp = player_hp(&scene.reg, scene.pawn);
            scene.tick_with_impact(|reg| {
                if flinch {
                    let _ = switch_animation_state(reg, jabber, "hit_reaction");
                }
            });
            if player_hp(&scene.reg, scene.pawn) < hp {
                hits.push(tick);
            }
            if tick == DROP_TICK + 1 {
                drawn = Some(mesh_state(&scene, jabber));
            }
        }
        (hits, drawn)
    };
    let redraw_tick = DROP_TICK + 1;
    assert_eq!(
        run(false),
        (
            vec![redraw_tick, redraw_tick + 30],
            Some("attack_jab".to_string())
        ),
        "fixture: the striker hits on the redraw tick, and the unflinched swing fires 0.5 s \
         after its redraw"
    );
    assert_eq!(
        run(true),
        (vec![redraw_tick], Some("hit_reaction".to_string())),
        "the flinch stays on screen and the swing never connects"
    );
}

// Picked up a tick or two short of its moment, or of its clip's end, and
// dropped: the frozen pre-pickup clock reaches nothing before the redraw. No
// moment fires on it and no whiff spends the latch, so the moment fires 0.5 s
// after the redraw; `animationDone` stays false until the redrawn clip plays
// through. Regression: the clock advanced on the drop tick and the redraw
// tick, reaching the moment, or the clip's end, on the clip the fresh mesh
// did not draw.
#[test]
fn a_dropped_enemy_reaches_no_moment_before_its_redraw() {
    // Unheld, the moment fires on tick 30 and `animationDone` turns true on
    // tick 61. Picked up before tick `pickup`'s AI, the clock last advanced
    // on tick `pickup - 1`: one tick short at 30 or 61, two at 29 or 60.
    for pickup in [29, 30, 60, 61] {
        let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
        let enemy = scene.spawn(
            0,
            &jab_graph(hit_jab(), vec![edge("stalk", guard_done())]),
            "jab",
        );
        let drop = pickup + 10;
        let redraw = drop + 1;
        let mut holder = None;
        let mut left = None;
        let hits = scene.hits_with(redraw + 70, |scene, tick| {
            match tick {
                _ if tick == pickup => holder = Some(pick_up(scene, enemy)),
                _ if tick == drop => drop_from(scene, enemy, holder.take().expect("held")),
                _ if tick == drop + 1 => resolve_dropped(scene, enemy),
                _ => {}
            }
            // The state after the previous tick.
            if left.is_none() && scene.state(enemy) == "stalk" {
                left = Some(tick - 1);
            }
        });
        let expected_hit = if pickup <= 30 { redraw + 30 } else { 30 };
        assert_eq!(
            hits,
            vec![expected_hit],
            "picked up on tick {pickup}: the moment fires once, its latch unspent before the \
             redraw"
        );
        // The redrawn one-second clip reaches its duration 60 ticks after the
        // redraw; `animationDone` reads true, and the activity leaves, on the
        // tick after.
        assert_eq!(
            left,
            Some(redraw + 61),
            "picked up on tick {pickup}: `animationDone` holds false until the redrawn clip \
             plays through"
        );
    }
}

// A save between the drop and the redraw, or while held, restores the switch
// count and the redraw request, so it fires on the tick an unsaved run does.
// A save lands between frames, after the drop tick's clip indices fill.
// Regression: a restore read the fresh mesh's default state as a pre-save
// flinch and cancelled the swing.
#[test]
fn a_restore_after_a_drop_fires_on_the_same_tick() {
    let unsaved = held_jab_hits(|_, _, _| {});
    assert_eq!(unsaved, vec![DROP_TICK + 1 + 30], "fixture");
    for save_tick in [20, DROP_TICK + 1] {
        let hits = held_jab_hits(|scene, enemy, tick| {
            if tick == save_tick {
                save_and_restore(scene, enemy);
            }
        });
        assert_eq!(hits, unsaved, "saved on tick {save_tick}");
    }
}

// The redraw restarts clip time after the moment check, so the next tick's
// window opens just below the start. Regression: the next tick's window opened
// at the start and a moment there was never reached.
#[test]
fn a_moment_at_the_start_of_a_redrawn_clip_fires_on_the_next_tick() {
    let run = |hold: bool| {
        let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.0)])]);
        let graph = clip_graph(
            "windup",
            vec![
                ("windup", act("attack_jab", None)),
                ("jab", act("attack_jab", Some(hit_jab()))),
            ],
            vec![("windup", vec![edge("jab", after_ms(99.0))])],
            vec![(JAB, contact_attack(10.0))],
        );
        let enemy = scene.spawn(0, &graph, "windup");
        let mut holder = None;
        scene.hits_with(45, |scene, tick| match tick {
            10 if hold => holder = Some(pick_up(scene, enemy)),
            20 if hold => drop_from(scene, enemy, holder.take().expect("held")),
            21 if hold => resolve_dropped(scene, enemy),
            _ => {}
        })
    };
    assert!(
        run(false).is_empty(),
        "fixture: the jab is entered past its moment at 0"
    );
    assert_eq!(
        run(true),
        vec![22],
        "the tick after the redraw on tick 21, the tick after the drop"
    );
}

// Clip time serializes with the activity clocks, so a restore mid-entry fires
// on the same tick.
#[test]
fn restored_brain_fires_on_the_same_tick() {
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    assert!(scene.hits(15).is_empty());
    save_and_restore(&mut scene, enemy);
    assert_eq!(scene.hits(30), vec![15], "tick 30 after entry, as unsaved");
}

/// Round-trip `enemy`'s brain and mesh, when it has one, through serde into a
/// fresh runtime, as a save and load do. The load resolves the mesh's clip
/// indices again, which serde leaves unfilled.
fn save_and_restore(scene: &mut Scene, enemy: EntityId) {
    let brain: BrainComponent =
        serde_json::from_value(serde_json::to_value(scene.brain(enemy)).unwrap()).unwrap();
    scene.reg.set_component(enemy, brain).unwrap();
    if let Ok(mesh) = scene.reg.get_component::<MeshComponent>(enemy) {
        let mut mesh: MeshComponent =
            serde_json::from_value(serde_json::to_value(mesh).unwrap()).unwrap();
        let resolved = &scene.mesh.animation.as_ref().unwrap().states;
        for (name, state) in &mut mesh.animation.as_mut().unwrap().states {
            state.clip_index = resolved[name].clip_index;
        }
        scene.reg.set_component(enemy, mesh).unwrap();
    }
    scene.runtime = AiRuntime::new();
}

// A flinch after the last compute, then a save: the restored switch count
// still reads the flinch as foreign. Regression: the restore rebound without a
// cancel and the saved open latch fired.
#[test]
fn a_restore_after_a_flinch_keeps_the_moment_cancelled() {
    let (mut scene, enemy) = jab_scene(0.5, hit_jab(), vec![]);
    assert!(scene.hits(15).is_empty());
    // A hitscan in the weapon stage, after this tick's AI stage.
    scene.switch(enemy, "hit_reaction");
    save_and_restore(&mut scene, enemy);
    assert!(
        scene.brain(enemy).clip_clock.moment_open,
        "fixture: the save carries the open latch"
    );
    let hits = scene.hits(30);
    assert!(
        hits.is_empty(),
        "the flinched swing never connects: {hits:?}"
    );
}

// An earlier slot's contact hit plays an impact clip, and the enemy's own
// travel-variant switch draws over it in the same batch: the swing under way
// was still flinched. Regression: the own switch's count masked the foreign one.
#[test]
fn a_foreign_switch_under_the_enemys_own_switch_cancels_the_moment() {
    let run = |flinch: bool| {
        let mut scene = Scene::new(vec![
            one_shot("strike", 1.0, &[("hit", 0.5)]),
            one_shot("strike_right", 1.0, &[("hit", 0.3)]),
        ]);
        let striker = clip_graph(
            "wait",
            vec![
                ("wait", act("idle", None)),
                ("strike", act("idle", Some(untimed("slam")))),
            ],
            vec![("wait", vec![edge("strike", after_ms(165.0))])],
            vec![("slam", contact_attack(10.0))],
        );
        scene.spawn(0, &striker, "wait");
        let jabber = scene.spawn(1, &strafing_graph("strike"), "shoot");
        let mut hits = Vec::new();
        for tick in 0..60 {
            if tick == 10 {
                travel_right(&mut scene, jabber);
            }
            let hp = player_hp(&scene.reg, scene.pawn);
            scene.tick_with_impact(|reg| {
                if flinch && tick == 10 {
                    let _ = switch_animation_state(reg, jabber, "hit_reaction");
                }
            });
            if player_hp(&scene.reg, scene.pawn) < hp {
                hits.push(tick);
            }
            if tick == 10 {
                assert_eq!(
                    mesh_state(&scene, jabber),
                    "strike_right",
                    "fixture: the jabber's own variant switch drew last"
                );
            }
        }
        hits
    };
    assert_eq!(
        run(false),
        vec![10, 28],
        "fixture: the striker hits on tick 10 and the variant's moment fires"
    );
    assert_eq!(run(true), vec![10], "the flinched swing never connects");
}

/// The tick `play`, on the one-second one-shot `attack_jab`, leaves on
/// `animationDone`. An impact clip takes the mesh off it on tick 10; with
/// `redraw`, stepping off on tick 20 makes the enemy's own switch draw it
/// again.
fn redrawn_leave_tick(redraw: bool) -> Option<usize> {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[])]);
    let graph = clip_graph(
        "play",
        vec![
            ("play", act("attack_jab", None)),
            ("done", act("idle", None)),
        ],
        vec![("play", vec![edge("done", guard_done())])],
        vec![],
    );
    let enemy = scene.spawn(0, &graph, "play");
    (0..200).find(|&tick| {
        if tick == 10 {
            scene.switch(enemy, "hit_reaction");
        }
        if redraw && tick == 20 {
            // The locomotion edge re-requests the activity's clip.
            scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(0.0, 0.0, 2.0));
        }
        scene.tick();
        if tick == 20 {
            let drawn = if redraw { "attack_jab" } else { "hit_reaction" };
            assert_eq!(mesh_state(&scene, enemy), drawn, "fixture: the drawn clip");
        }
        scene.state(enemy) == "done"
    })
}

// Clip time is the time since the drawn clip began playing, and the enemy's
// own switch back onto it starts it over. Regression: clip time ran on from the
// state's first start while the drawn clip restarted at 0.
#[test]
fn the_enemys_own_switch_back_onto_its_clip_restarts_clip_time() {
    assert_eq!(
        redrawn_leave_tick(false),
        Some(61),
        "off its clip, clip time runs on from the state's start"
    );
    assert_eq!(
        redrawn_leave_tick(true),
        Some(81),
        "done one second after the redraw on tick 20"
    );
}

/// `strike` on `state` firing `hit_jab()` with no motion of its own: its
/// `move` selector resolves a position goal while `@state.retreat` is set,
/// which suppresses the action for that tick, and holds otherwise.
fn retreating_graph(state: &str) -> BehaviorGraphDescriptor {
    let mut strike = act(state, Some(hit_jab()));
    strike.motion = None;
    strike.layers = BTreeMap::from([(
        "move".to_string(),
        BehaviorLayerDescriptor::Selector(vec![
            BehaviorSelectorEntry::Row(BehaviorSelectorRow {
                when: Some(IrNode::Ge {
                    a: Box::new(brain_input("@state.retreat")),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(1.0),
                    }),
                }),
                motion: Some(MotionVerb::MoveToAnchor),
                action: None,
            }),
            BehaviorSelectorEntry::Motion(MotionVerb::Hold),
        ]),
    )]);
    clip_graph(
        "strike",
        vec![("strike", strike)],
        vec![],
        vec![(JAB, contact_attack(10.0))],
    )
}

fn set_retreat(scene: &mut Scene, enemy: EntityId, retreat: bool) {
    scene
        .reg
        .entity_state_mut(enemy)
        .expect("spawned enemy carries entity state")
        .set("retreat", if retreat { 1.0 } else { 0.0 });
}

// The latch belongs to the activity whose action carries `at`, not to the
// action a selector resolves on one tick. Regression: a position goal on the
// entry tick closed the latch for the entry, and one on the moment's tick
// deferred the moment to the loop's next pass.
#[test]
fn a_position_goal_suppressing_the_action_neither_closes_nor_defers_the_moment() {
    // A position goal on the entry tick.
    let mut scene = Scene::new(vec![one_shot("attack_jab", 1.0, &[("hit", 0.5)])]);
    let enemy = scene.spawn(0, &retreating_graph("attack_jab"), "strike");
    let hits = scene.hits_with(45, |scene, tick| set_retreat(scene, enemy, tick == 0));
    assert_eq!(hits, vec![30], "the entry's moment still fires");

    // A position goal on the moment's tick: reached and spent, a whiff.
    let run = |retreat_at_moment: bool| {
        let mut scene = Scene::new(vec![looping("fire_loop", 1.0, &[("hit", 0.25)])]);
        let enemy = scene.spawn(0, &retreating_graph("fire_loop"), "strike");
        let first = loop_pass_ticks(instance_phase(enemy.to_raw(), 1.0), 0.25, 1.0, 200)[0];
        let hits = scene.hits_with(200, |scene, tick| {
            set_retreat(scene, enemy, retreat_at_moment && tick == first)
        });
        (first, hits)
    };
    let (first, hits) = run(false);
    assert_eq!(
        hits,
        vec![first],
        "fixture: unsuppressed, the first pass fires"
    );
    let (_, hits) = run(true);
    assert!(
        hits.is_empty(),
        "not deferred to the loop's next pass: {hits:?}"
    );
}

// Once an entry's timed moment is spent, a travel variant whose clip lacks the
// event no longer fires the attack as without `at`. Regression: a whiffed or
// cancelled swing fired later in the entry through the untimed variant.
#[test]
fn a_spent_moment_does_not_fire_through_an_untimed_travel_variant() {
    let specs = || {
        vec![
            one_shot("strike", 1.0, &[("hit", 0.5)]),
            one_shot("strike_right", 1.0, &[]),
        ]
    };
    // Out of reach at the moment, then back in reach on the variant.
    let (mut scene, enemy) = strafing_scene(specs(), "strike");
    let pawn = scene.pawn;
    let hits = scene.hits_with(60, |scene, tick| match tick {
        25 => set_enemy_position(&mut scene.reg, pawn, Vec3::new(5.0, 0.0, 0.0)),
        35 => {
            set_enemy_position(&mut scene.reg, pawn, player_spot());
            travel_right(scene, enemy);
        }
        _ => {}
    });
    assert_eq!(mesh_state(&scene, enemy), "strike_right", "fixture");
    assert!(hits.is_empty(), "whiffed: {hits:?}");

    // Cancelled by an impact clip, then drawn on the variant.
    let (mut scene, enemy) = strafing_scene(specs(), "strike");
    let hits = scene.hits_with(60, |scene, tick| match tick {
        10 => scene.switch(enemy, "hit_reaction"),
        20 => travel_right(scene, enemy),
        _ => {}
    });
    assert_eq!(mesh_state(&scene, enemy), "strike_right", "fixture");
    assert!(hits.is_empty(), "cancelled: {hits:?}");
}

// The entered activity's selector reads its own clip's `animationDone` on its
// entry tick, not the predecessor's that has played through. Regression: the
// fact lagged a tick, and the selector fired on entry.
#[test]
fn an_entered_one_shots_selector_reads_its_own_clip_on_entry() {
    let mut scene = Scene::new(vec![
        one_shot("flinch", 0.2, &[]),
        one_shot("attack_jab", 0.5, &[]),
    ]);
    let mut jab = act("attack_jab", None);
    jab.layers = BTreeMap::from([(
        "offense".to_string(),
        BehaviorLayerDescriptor::Selector(vec![BehaviorSelectorEntry::Row(BehaviorSelectorRow {
            when: Some(guard_done()),
            motion: None,
            action: Some(untimed(JAB)),
        })]),
    )]);
    let graph = clip_graph(
        "flinch",
        vec![("flinch", act("flinch", None)), ("jab", jab)],
        vec![("flinch", vec![edge("jab", guard_done())])],
        vec![(JAB, contact_attack(10.0))],
    );
    let enemy = scene.spawn(0, &graph, "flinch");
    let mut after = Vec::new();
    let hits = scene.hits_with(60, |scene, _| after.push(scene.state(enemy)));
    after.push(scene.state(enemy));
    assert_eq!(after[13], "flinch");
    assert_eq!(after[14], "jab", "fixture: entered on tick 13");
    assert_eq!(
        hits,
        vec![44],
        "not on the entry tick: once the jab's own clip has played through"
    );
}

// A seated path's rows read the clip it is about to draw; a transition on that
// tick back onto the clip the clock tracks gives the entered path's selectors
// the clock's own value. Regression: they kept the seated path's value, and
// read a played-through clip as not done for that tick.
#[test]
fn a_transition_back_onto_the_tracked_clip_reads_its_own_animation_done() {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 0.5, &[])]);
    let mut jab = act("attack_jab", None);
    jab.layers = BTreeMap::from([(
        "offense".to_string(),
        BehaviorLayerDescriptor::Selector(vec![BehaviorSelectorEntry::Row(BehaviorSelectorRow {
            when: Some(guard_done()),
            motion: None,
            action: Some(untimed(JAB)),
        })]),
    )]);
    let not_done = IrNode::Not {
        x: Box::new(guard_done()),
    };
    let graph = clip_graph(
        "warm",
        vec![
            ("warm", act("attack_jab", None)),
            ("pause", act("idle", None)),
            ("jab", jab),
        ],
        vec![("pause", vec![edge("jab", not_done)])],
        vec![(JAB, contact_attack(10.0))],
    );
    let enemy = scene.spawn(0, &graph, "warm");
    let mut states = Vec::new();
    let hits = scene.hits_with(60, |scene, tick| {
        if tick == 40 {
            // Seated as a reseat seats a path: its rows run this tick.
            scene.enter(enemy, "pause");
        }
        states.push(scene.state(enemy));
    });
    states.push(scene.state(enemy));
    assert_eq!(
        states[41], "jab",
        "fixture: `pause` reads its looping clip as not done and leaves on tick 40"
    );
    assert_eq!(
        hits,
        vec![40],
        "`attack_jab` played through on tick 30: the selector reads done on tick 40"
    );
}

/// The jab scene seated in `stalk` and entered into `jab` on tick 14. With
/// `launch`, a knockback on tick 2 is spent by tick 4 while the body stays
/// airborne until tick 50.
fn mid_air_entry_hits(launch: bool) -> (Vec<usize>, Vec<String>) {
    let mut scene = Scene::new(vec![
        one_shot("attack_jab", 1.0, &[("hit", 0.5)]),
        looping("jump_loop", 1.0, &[]),
    ]);
    let enemy = scene.spawn(0, &jab_graph(hit_jab(), vec![]), "stalk");
    let mut clips = Vec::new();
    let hits = scene.hits_with(60, |scene, tick| {
        match tick {
            2 if launch => scene.set_agent(enemy, |agent| {
                agent.knockback_velocity = Vec3::new(2.0, 0.0, 0.0);
                agent.ground = postretro_foundation::GroundRef::Airborne;
            }),
            4 if launch => scene.set_agent(enemy, |agent| agent.knockback_velocity = Vec3::ZERO),
            14 => scene.enter(enemy, "jab"),
            50 => scene.set_agent(enemy, |agent| {
                agent.ground = postretro_foundation::GroundRef::World;
            }),
            _ => {}
        }
        clips.push(mesh_state(scene, enemy));
    });
    (hits, clips)
}

// An entry mid-air, after drag spent the knockback but before the body lands,
// is a launched swing. Regression: it fired while `jump_loop` showed.
#[test]
fn an_entry_mid_air_after_a_spent_knockback_never_connects() {
    let (hits, _) = mid_air_entry_hits(false);
    assert_eq!(
        hits,
        vec![44],
        "fixture: grounded, the entry's moment fires"
    );
    let (hits, clips) = mid_air_entry_hits(true);
    assert!(
        clips[15..50].iter().any(|clip| clip == "jump_loop"),
        "fixture: `jump_loop` shows after the entry"
    );
    assert!(hits.is_empty(), "airborne or after landing: {hits:?}");
}

// A loop restarted while its model was still loading takes its drawn phase
// once the clip resolves, so its moments sit where the renderer draws them.
// Regression: it kept phase 0.
#[test]
fn a_loop_whose_clip_resolves_late_keeps_its_drawn_phase() {
    let (mut scene, _, next) = aim_scene();
    let mut loaded = Some(std::mem::replace(&mut scene.clips, HitZoneStore::new()));
    let hits = scene.hits_with(200, |scene, tick| {
        if tick == 5 {
            scene.clips = loaded.take().expect("loads once");
        }
    });
    assert_eq!(hits, vec![next], "the pass a phased start draws");
}

// No per-tick allocation: the clip clock's steady-state work allocates nothing.
#[test]
fn clip_clock_steady_state_allocates_nothing() {
    let (mut scene, enemy) = jab_scene(0.9, hit_jab(), vec![]);
    for _ in 0..5 {
        scene.tick();
    }
    let mut brain = scene.brain(enemy);
    assert!(
        brain.clip_clock.moment_open,
        "fixture: the moment is pending"
    );
    let source = crate::clip_clock::ClipSource {
        registry: &scene.reg,
        clips: Some(&scene.clips),
        id: enemy,
    };
    let mut warned = HashSet::new();
    let snapshot = AllocSnapshot::arm();
    let clock = brain.clip_clock.clone();
    crate::clip_clock::begin_tick(&mut brain, &source, DT);
    let done = crate::clip_clock::animation_done(&brain.clip_clock, &source);
    let moment = crate::clip_clock::settle(&mut brain, &source, None, TravelVariant::Base);
    scene
        .runtime
        .programs
        .warn_unclipped_moments(&scene.reg, &scene.clips, &mut warned);
    let allocs = snapshot.allocs_since();
    drop(clock);
    assert!(!done);
    assert_eq!(moment, crate::clip_clock::Moment::Pending);
    assert_eq!(allocs, 0, "clone, advance, done, settle and the bind check");
}

/// Mean heap allocations per AI tick for a wave standing out of reach in
/// `jab`, its moment pending when `action` fires at one.
fn wave_allocs_per_tick(action: ActionVerb) -> f64 {
    let mut scene = Scene::new(vec![one_shot("attack_jab", 2.0, &[("hit", 1.5)])]);
    let mut graph = jab_graph(action, vec![]);
    // An untimed jab carries the same mark, so turns work alike.
    graph
        .envelope
        .activities
        .get_mut("jab")
        .unwrap()
        .attack_turn = Some(JAB.to_string());
    set_enemy_position(&mut scene.reg, scene.pawn, Vec3::new(3.0, 0.0, 0.0));
    for index in 0..16 {
        scene.spawn(index, &graph, "jab");
    }
    for _ in 0..4 {
        scene.tick();
    }
    let snapshot = AllocSnapshot::arm();
    for _ in 0..16 {
        scene.tick();
    }
    snapshot.allocs_since() as f64 / 16.0
}

// The per-tick cost of a pending clip-timed attack adds no allocation to the
// AI tick.
#[test]
fn clip_timed_wave_allocates_no_more_per_tick_than_an_untimed_one() {
    let untimed = wave_allocs_per_tick(untimed(JAB));
    let timed = wave_allocs_per_tick(hit_jab());
    assert!(
        timed <= untimed,
        "clip-timed {timed:.1} vs untimed {untimed:.1} allocations per tick"
    );
}
