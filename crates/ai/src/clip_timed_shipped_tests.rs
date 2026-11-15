// Clip-timed attacks in the shipped dev brute and limitator, on their committed clip libraries.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

use postretro_model::skeleton::AnimationClip;
use postretro_scripting_core::runtime::ModManifestResult;

use super::*;
use crate::ai_tests::attack_turn_ticks::shipped_content::{
    descriptor, dev_manifest, dev_registry, durable_player, spawn_shipped,
};

/// The dev mod's enemies on one durable player, ticked at 60 Hz with every
/// spawned model's clips loaded as level load installs them.
struct Shipped {
    manifest: ModManifestResult,
    reg: EntityRegistry,
    runtime: AiRuntime,
    clips: HitZoneStore,
    player: EntityId,
}

impl Shipped {
    fn new(manifest: ModManifestResult, player_at: Vec3) -> Self {
        let mut reg = dev_registry(&manifest);
        let player = durable_player(&mut reg, player_at);
        Self {
            manifest,
            reg,
            runtime: AiRuntime::new(),
            clips: HitZoneStore::new(),
            player,
        }
    }

    /// Spawn `name` at the origin, facing the player along +X.
    fn spawn(&mut self, name: &str) -> EntityId {
        spawn_shipped(
            &mut self.reg,
            &self.manifest,
            &mut self.clips,
            name,
            Vec3::ZERO,
        )
    }

    /// The clip `enemy`'s mesh state `state` plays, from the loaded library.
    fn clip(&self, enemy: EntityId, state: &str) -> &AnimationClip {
        let mesh = self.reg.get_component::<MeshComponent>(enemy).unwrap();
        let declared = &mesh.animation.as_ref().unwrap().states[state];
        self.clips
            .get_by_name(&mesh.model)
            .and_then(|model| model.state_clip(declared.clip_index, &declared.clip))
            .expect("the state's clip resolves")
    }

    fn tick(&mut self) -> AiTickResult {
        run_ai_tick_with_navigation_and_impact(
            &mut self.reg,
            &mut self.runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav::none(),
                nav_graph: None,
                collision_world: Some(&CollisionWorld::new()),
                descriptors: &self.manifest.entities,
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: Some(&self.clips),
            },
            |_| {},
        )
    }

    fn path(&self, enemy: EntityId) -> String {
        enemy_state_path(&self.reg, enemy)
    }

    fn cooldown(&self, enemy: EntityId, attack: &str) -> f32 {
        self.reg
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .attack_cooldown_remaining_ms
            .get(attack)
            .copied()
            .unwrap_or(0.0)
    }

    fn holds_turn(&self, enemy: EntityId) -> bool {
        self.reg
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .attack_turns
            .holds_any()
    }

    fn place_player(&mut self, at: Vec3) {
        let mut transform = *self.reg.get_component::<Transform>(self.player).unwrap();
        transform.position = at;
        self.reg.set_component(self.player, transform).unwrap();
    }

    /// The spot `distance` from `enemy` on the ground, straight away from it
    /// through `from`.
    fn away_from(&self, enemy: EntityId, from: Vec3, distance: f32) -> Vec3 {
        let at = self.reg.get_component::<Transform>(enemy).unwrap().position;
        let away = Vec3::new(from.x - at.x, 0.0, from.z - at.z).normalize_or_zero();
        Vec3::new(at.x, from.y, at.z) + away * distance
    }
}

/// Ticks from a one-shot's restart until its clip time, a whole number of
/// ticks, reaches `event` seconds, the restart tick being 0: the tick a moment
/// there fires. The slack is the clip clock's, so a moment on a tick boundary
/// lands on that tick.
fn one_shot_moment_tick(event: f32) -> usize {
    const SLACK: f64 = 1.0e-4;
    (0..)
        .find(|&tick| tick as f64 * f64::from(DT) + SLACK >= f64::from(event))
        .expect("a finite moment")
}

/// Ticks from a loop's restart at `enemy`'s drawn phase until its first pass
/// of `event`, the restart tick being 0.
fn loop_moment_tick(enemy: EntityId, clip: &AnimationClip, event: &str) -> usize {
    let time = clip.event_time(event).expect("the clip carries the event");
    let phase = instance_phase(enemy.to_raw(), clip.duration);
    loop_pass_ticks(phase, time, clip.duration, 600)[0]
}

/// The first tick at or after `from` whose path ends in `/{leaf}`.
fn entry_tick(paths: &[String], leaf: &str, from: usize) -> Option<usize> {
    let suffix = format!("/{leaf}");
    (from..paths.len()).find(|&tick| {
        paths[tick].ends_with(&suffix) && (tick == 0 || !paths[tick - 1].ends_with(&suffix))
    })
}

// The reference brute's slam lands as `smash` reaches its `hit`, and its jabs
// as `punch` reaches its own, each timed from its entry onto the clip.
#[test]
fn shipped_brute_slam_fires_at_the_smash_hit_and_jabs_at_the_punch_hit() {
    let mut shipped = Shipped::new(dev_manifest(), Vec3::new(1.5, 0.0, 0.0));
    let brute = shipped.spawn("reference_enemy");
    // Its own armor broken sends it to retreat; the shipped archetype spawns
    // with 120.
    give_reference_armor(&mut shipped.reg, brute);
    assert_eq!(
        shipped.clip(brute, "attack_slam").name,
        "smash",
        "fixture: the slam plays the smash"
    );
    assert_eq!(
        shipped.clip(brute, "attack_jab").name,
        "punch",
        "fixture: the jab plays the punch"
    );
    let slam_hit = shipped
        .clip(brute, "attack_slam")
        .event_time("hit")
        .expect("`smash` carries its `hit`");
    let jab_hit = shipped
        .clip(brute, "attack_jab")
        .event_time("hit")
        .expect("`punch` carries its `hit`");
    let (slam_moment, jab_moment) = (
        one_shot_moment_tick(slam_hit),
        one_shot_moment_tick(jab_hit),
    );

    let (mut paths, mut slams, mut jabs, mut hurt) = (Vec::new(), Vec::new(), Vec::new(), 0);
    for tick in 0..(8.0 / DT) as usize {
        let (slam, jab) = (
            shipped.cooldown(brute, "slam"),
            shipped.cooldown(brute, "jab"),
        );
        let hp = player_hp(&shipped.reg, shipped.player);
        shipped.tick();
        if shipped.cooldown(brute, "slam") > slam {
            slams.push(tick);
        }
        if shipped.cooldown(brute, "jab") > jab {
            jabs.push(tick);
        }
        hurt += usize::from(player_hp(&shipped.reg, shipped.player) < hp);
        paths.push(shipped.path(brute));
    }

    let windup = entry_tick(&paths, "windup", 0).expect("the brute winds up a slam");
    assert!(!slams.is_empty(), "the brute slams: {paths:?}");
    assert_eq!(
        paths[windup + 1..=slams[0]]
            .iter()
            .filter(|path| !path.ends_with("/windup") && !path.ends_with("/commit"))
            .count(),
        0,
        "the slam's smash runs from the windup through the commit"
    );
    assert_eq!(
        slams.first().copied(),
        Some(windup + slam_moment),
        "the slam fires as the smash, started at the windup's entry, reaches `hit` ({slam_hit} s)"
    );
    assert!(!jabs.is_empty(), "the brute jabs after its slam: {paths:?}");
    let mut from = 0;
    for &fired in &jabs {
        let entered = entry_tick(&paths, "jab", from).expect("each jab enters `jab`");
        assert_eq!(
            fired,
            entered + jab_moment,
            "a jab fires as its punch reaches `hit` ({jab_hit} s)"
        );
        from = fired;
    }
    assert_eq!(
        hurt,
        slams.len() + jabs.len(),
        "each fire lands on the player"
    );
}

// A jab cut short the tick after it fires, by its target stepping out of
// reach, and re-entered as soon as `stalk`'s rest allows still fires at its
// punch's `hit`: the soonest re-entered moment falls after the jab's cooldown.
#[test]
fn shipped_brute_jab_re_entered_right_after_a_fire_fires_at_the_punch_hit() {
    let near = Vec3::new(1.5, 0.0, 0.0);
    let mut shipped = Shipped::new(dev_manifest(), near);
    let brute = shipped.spawn("reference_enemy");
    give_reference_armor(&mut shipped.reg, brute);
    let hit = shipped
        .clip(brute, "attack_jab")
        .event_time("hit")
        .expect("`punch` carries its `hit`");
    let moment = one_shot_moment_tick(hit);

    let (mut paths, mut jabs, mut hurt) = (Vec::new(), Vec::<usize>::new(), Vec::new());
    for tick in 0..(6.0 / DT) as usize {
        // Out of jab reach for the one tick after the first jab fires.
        let stepped_out = jabs.first().is_some_and(|&fired| fired + 1 == tick);
        let at = if stepped_out {
            shipped.away_from(brute, near, 3.0)
        } else {
            near
        };
        shipped.place_player(at);
        let (jab, hp) = (
            shipped.cooldown(brute, "jab"),
            player_hp(&shipped.reg, shipped.player),
        );
        shipped.tick();
        if shipped.cooldown(brute, "jab") > jab {
            jabs.push(tick);
        }
        if player_hp(&shipped.reg, shipped.player) < hp {
            hurt.push(tick);
        }
        paths.push(shipped.path(brute));
        if jabs.len() == 2 {
            break;
        }
    }

    let fired = *jabs.first().expect("the brute jabs");
    assert!(
        paths[fired + 1].ends_with("/stalk"),
        "stepping out of reach ends the jab the tick after it fires: {paths:?}"
    );
    let reentered = entry_tick(&paths, "jab", fired + 1).expect("the brute jabs again");
    // `stalk`'s 150 ms rest, nine or ten whole ticks.
    assert!(
        reentered <= fired + 1 + 10,
        "fixture: the jab re-enters as soon as the rest allows: {paths:?}"
    );
    assert_eq!(
        jabs.get(1).copied(),
        Some(reentered + moment),
        "the re-entered jab fires as its punch reaches `hit` ({hit} s), not a cooldown whiff"
    );
    assert!(
        hurt.contains(&(reentered + moment)),
        "the re-entered jab lands on the player"
    );
}

/// A shipped rifle enemy six meters from the player, with `fillers` entities
/// spawned first so its id, and so its drawn phase, differs.
fn rifle_scene(manifest: ModManifestResult, name: &str, fillers: usize) -> (Shipped, EntityId) {
    let mut shipped = Shipped::new(manifest, Vec3::new(6.0, 0.0, 0.0));
    for _ in 0..fillers {
        shipped.reg.spawn(Transform::default());
    }
    let enemy = shipped.spawn(name);
    (shipped, enemy)
}

/// One `burst` entry: its entry and exit ticks, the path it left to, its
/// moment tick, the ticks a shot left the rifle, and whether it held an
/// attack turn on entry and after leaving.
#[derive(Debug)]
struct Burst {
    entered: usize,
    left: usize,
    left_to: String,
    moment: usize,
    shots: Vec<usize>,
    turn_on_entry: bool,
    turn_after: bool,
}

/// Tick until `enemy`'s first `burst` entry has left, calling `before` ahead
/// of each tick.
fn first_burst(
    shipped: &mut Shipped,
    enemy: EntityId,
    mut before: impl FnMut(&mut Shipped, &str),
) -> Burst {
    let kick = loop_moment_tick(enemy, shipped.clip(enemy, "shoot"), "kick");
    let (mut entered, mut shots, mut turn_on_entry) = (None, Vec::new(), false);
    for tick in 0..(6.0 / DT) as usize {
        let path = shipped.path(enemy);
        before(shipped, &path);
        let result = shipped.tick();
        shots.extend((!result.projectile_spawns.is_empty()).then_some(tick));
        let path = shipped.path(enemy);
        match entered {
            None if path.ends_with("/burst") => {
                entered = Some(tick);
                turn_on_entry = shipped.holds_turn(enemy);
            }
            Some(entered) if !path.ends_with("/burst") => {
                return Burst {
                    entered,
                    left: tick,
                    left_to: path,
                    moment: entered + kick,
                    shots,
                    turn_on_entry,
                    turn_after: shipped.holds_turn(enemy),
                };
            }
            _ => {}
        }
    }
    panic!("the burst ends within 6 s; entered: {entered:?}, shots: {shots:?}");
}

// The limitator's first burst shot leaves on `firing_rifle`'s `kick`,
// the loop's first pass of it from the enemy's drawn phase.
#[test]
fn shipped_limitator_first_burst_shot_fires_at_the_rifle_kick() {
    let (mut shipped, limitator) = rifle_scene(dev_manifest(), "limitator", 0);
    let clip = shipped.clip(limitator, "shoot");
    assert_eq!(
        clip.name, "firing_rifle",
        "fixture: the burst plays the rifle loop"
    );
    assert!(
        clip.event_time("kick").is_some(),
        "`firing_rifle` carries its `kick`"
    );
    let burst = first_burst(&mut shipped, limitator, |_, _| {});
    assert_eq!(
        burst.shots.first().copied(),
        Some(burst.moment),
        "the first shot fires at the loop's next `kick` from the drawn phase: {burst:?}"
    );
}

// A limitator whose `shoot` misses its `kick` (its rifle still
// recovering at the moment) fires nothing that entry, even once the rifle is
// ready, then leaves `burst` on its timer and frees its `ranged` turn.
#[test]
fn shipped_limitator_whiffing_its_kick_leaves_burst_and_frees_its_turn() {
    let (mut shipped, limitator) = rifle_scene(dev_manifest(), "limitator", 0);
    let pass = shipped.clip(limitator, "shoot").duration;
    // Recovering through the first pass from any phase, and ready well before
    // the burst's timer ends.
    let recovering_ms = (pass + 2.0 * DT) * 1000.0;
    let burst = first_burst(&mut shipped, limitator, |shipped, path| {
        if path.ends_with("/wait") {
            let mut brain = shipped
                .reg
                .get_component::<BrainComponent>(limitator)
                .unwrap()
                .clone();
            brain
                .attack_cooldown_remaining_ms
                .insert("shoot".to_string(), recovering_ms);
            shipped.reg.set_component(limitator, brain).unwrap();
        }
    });
    assert!(
        recovering_ms < (burst.left - burst.entered - 1) as f32 * DT * 1000.0,
        "fixture: the rifle is ready for the rest of the entry: {burst:?}"
    );
    assert_eq!(
        burst.shots,
        Vec::<usize>::new(),
        "the whiffed burst fires nothing"
    );
    assert!(
        burst.left_to.ends_with("/move"),
        "it leaves `burst` for its reposition: {burst:?}"
    );
    assert!(
        burst.turn_on_entry && !burst.turn_after,
        "it holds its `ranged` turn from entry, and leaving frees it: {burst:?}"
    );
}

// At every drawn phase, a limitator that fires leaves `burst` after its
// third shot. `rifle_trooper` and `rifle_warrior` run the same graph.
#[test]
fn shipped_limitator_leaves_burst_after_its_third_shot_at_every_phase() {
    let manifest = dev_manifest();
    let behavior = |name: &str| descriptor(&manifest, name).behavior.clone();
    assert_eq!(behavior("rifle_trooper"), behavior("limitator"));
    assert_eq!(behavior("rifle_warrior"), behavior("limitator"));

    let mut waits = std::collections::BTreeSet::new();
    let mut pass_ticks = 0;
    for fillers in 0..24 {
        let (mut shipped, limitator) = rifle_scene(manifest.clone(), "limitator", fillers);
        pass_ticks = (shipped.clip(limitator, "shoot").duration / DT).floor() as usize;
        let burst = first_burst(&mut shipped, limitator, |_, _| {});
        assert_eq!(
            burst.shots.len(),
            3,
            "a three-shot burst inside the entry: {burst:?}"
        );
        assert_eq!(burst.shots[0], burst.moment, "{burst:?}");
        assert!(
            burst.left > burst.shots[2],
            "it leaves after its third shot: {burst:?}"
        );
        assert!(burst.left_to.ends_with("/move"), "{burst:?}");
        assert!(
            burst.turn_on_entry && !burst.turn_after,
            "it holds its turn through the burst and frees it: {burst:?}"
        );
        waits.insert(burst.moment - burst.entered);
    }
    assert!(
        waits.len() >= 6 && waits.iter().any(|&wait| wait + 2 >= pass_ticks),
        "fixture: the sweep draws phases across the {pass_ticks}-tick pass, \
         a near-full wait for the kick included: {waits:?}"
    );
}

// A limitator whose target slips past `BREAK_RANGE` the tick after its first
// shot leaves `burst` for `close`, and back in range re-enters `burst` the
// short way, through `wait` rather than its reposition. That burst still
// fires at its `kick`: `wait` spends the rifle's recovery.
#[test]
fn shipped_limitator_burst_re_entered_through_wait_after_a_fire_fires_at_the_rifle_kick() {
    let near = Vec3::new(6.0, 0.0, 0.0);
    let (mut shipped, limitator) = rifle_scene(dev_manifest(), "limitator", 0);
    let kick = loop_moment_tick(limitator, shipped.clip(limitator, "shoot"), "kick");

    let (mut paths, mut shots) = (Vec::new(), Vec::<usize>::new());
    for tick in 0..(6.0 / DT) as usize {
        // Past the break range, inside the rifle's, for the one tick after
        // the first shot. The strafe has moved the limitator off its spawn.
        let stepped_out = shots.first().is_some_and(|&fired| fired + 1 == tick);
        let at = if stepped_out {
            shipped.away_from(limitator, near, 10.0)
        } else {
            near
        };
        shipped.place_player(at);
        let result = shipped.tick();
        if !result.projectile_spawns.is_empty() {
            shots.push(tick);
        }
        paths.push(shipped.path(limitator));
    }

    let fired = *shots.first().expect("the first burst fires");
    assert!(
        paths[fired].ends_with("/burst") && paths[fired + 1].ends_with("/close"),
        "stepping past the break range ends the burst the tick after its first shot: {paths:?}"
    );
    let reentered = entry_tick(&paths, "burst", fired + 1).expect("the limitator bursts again");
    assert!(
        paths[fired + 1..reentered]
            .iter()
            .all(|path| !path.ends_with("/move")),
        "fixture: it comes back through `wait`, not its reposition: {paths:?}"
    );
    assert_eq!(
        shots.iter().copied().find(|&shot| shot >= reentered),
        Some(reentered + kick),
        "the re-entered burst fires at its `kick`, not a cooldown whiff: {shots:?}"
    );
}
