// Clip-timed attacks in the shipped dev brute and limitator, on their committed clip libraries.
// See: context/lib/entity_model.md §7c (Clip-timed attacks, Stances and attacks)

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

/// One brute strike's release: the attack, its release's entry tick and the
/// ticks its attack fired from that entry until the next.
#[derive(Debug)]
struct Release {
    attack: &'static str,
    entered: usize,
    fired: Vec<usize>,
}

/// Every release of `attacks` in `paths`, with the ticks each attack's
/// cooldown restarted (it fired) from that entry until the next.
fn releases(
    paths: &[String],
    fires: &[(&'static str, usize)],
    attacks: &[&'static str],
) -> Vec<Release> {
    let mut found = Vec::new();
    for &attack in attacks {
        let leaf = format!("{attack}_release");
        let mut from = 0;
        while let Some(entered) = entry_tick(paths, &leaf, from) {
            let next = entry_tick(paths, &leaf, entered + 1).unwrap_or(paths.len());
            let fired = fires
                .iter()
                .filter(|(name, tick)| *name == attack && (entered..next).contains(tick))
                .map(|&(_, tick)| tick)
                .collect();
            found.push(Release {
                attack,
                entered,
                fired,
            });
            from = entered + 1;
        }
    }
    found
}

// K1: the reference brute's slam, lunge and jab releases fire as their
// `*_strike` clip reaches its `hit`, timed from the release's entry onto the
// clip, and no missing-event warning is logged once its clips bind.
#[test]
fn shipped_brute_strikes_fire_at_their_strike_clips_hit() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut shipped = Shipped::new(dev_manifest(), Vec3::new(1.5, 0.0, 0.0));
    let brute = shipped.spawn("reference_enemy");
    // Its own armor broken sends it to retreat; the shipped archetype spawns
    // with 120.
    give_reference_armor(&mut shipped.reg, brute);
    let strikes = [
        ("slam", "smash_strike"),
        ("lunge", "swipe_strike"),
        ("jab", "punch_strike"),
    ];
    let mut moments = BTreeMap::new();
    for (attack, state) in strikes {
        let clip = shipped.clip(brute, state);
        assert_eq!(
            clip.name, state,
            "fixture: `{state}` plays its namesake clip"
        );
        let hit = clip
            .event_time("hit")
            .unwrap_or_else(|| panic!("`{state}` carries `hit`"));
        moments.insert(attack, one_shot_moment_tick(hit));
    }

    let attacks = strikes.map(|(attack, _)| attack);
    let (mut paths, mut fires, mut hurt) = (Vec::new(), Vec::new(), 0);
    for tick in 0..(30.0 / DT) as usize {
        let before = attacks.map(|attack| shipped.cooldown(brute, attack));
        let hp = player_hp(&shipped.reg, shipped.player);
        shipped.tick();
        for (index, attack) in attacks.into_iter().enumerate() {
            if shipped.cooldown(brute, attack) > before[index] {
                fires.push((attack, tick));
            }
        }
        hurt += usize::from(player_hp(&shipped.reg, shipped.player) < hp);
        paths.push(shipped.path(brute));
    }

    let releases = releases(&paths, &fires, &attacks);
    for attack in attacks {
        assert!(
            releases
                .iter()
                .any(|release| release.attack == attack && !release.fired.is_empty()),
            "the brute's `{attack}` releases and fires within 30 s: {releases:?}"
        );
    }
    for release in &releases {
        assert_eq!(
            release.fired,
            vec![release.entered + moments[release.attack]],
            "a release fires once, as its clip reaches `hit`: {release:?}"
        );
    }
    assert_eq!(hurt, fires.len(), "each fire lands on the player");
    let missing: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level <= log::Level::Warn && record.message.contains("cannot time it")
        })
        .collect();
    assert!(missing.is_empty(), "no missing-event warning: {missing:#?}");
}

/// What one brute run on the open floor left, tick by tick: the path, the
/// mesh state and the playback rate the pose stage set, the brute's ground
/// speed and steering multiplier, and its distance to the player; plus the
/// ticks the run's attack fired (its cooldown restarted) and the ticks the
/// player lost health, with the health lost.
#[derive(Debug, Default)]
struct BruteRun {
    paths: Vec<String>,
    states: Vec<String>,
    rates: Vec<f32>,
    speed: Vec<f32>,
    steering: Vec<f32>,
    distance: Vec<f32>,
    /// The brute's and the player's positions after each tick.
    positions: Vec<Vec3>,
    players: Vec<Vec3>,
    /// The brute's leap landing spot after each tick.
    spots: Vec<Option<Vec3>>,
    fired: Vec<usize>,
    /// The recorded attack's cooldown left after each tick, in milliseconds.
    cooldowns: Vec<f32>,
    hits: Vec<(usize, f32)>,
    /// Each mesh state whose clip carries `hit`: the hit's clip time and the
    /// clip's length, in seconds.
    moments: BTreeMap<String, (f32, f32)>,
}

impl BruteRun {
    fn entered(&self, leaf: &str) -> usize {
        entry_tick(&self.paths, leaf, 0)
            .unwrap_or_else(|| panic!("the brute enters `{leaf}`: {:?}", self.paths))
    }
}

/// Where the brute runs start, resting on an open floor.
fn brute_home() -> Vec3 {
    Vec3::new(10.0, chaser_rest_y(), 10.0)
}

/// Where the charge runs start: the brute, and the player 1.5 m off along +X.
fn charge_home() -> (Vec3, Vec3) {
    let brute = brute_home();
    (brute, brute + Vec3::new(1.5, 0.0, 0.0))
}

impl Shipped {
    /// Set the playback rate `enemy`'s current state plays at, as the pose
    /// stage does after steering: the stride rule's clamped ratio of its
    /// steered ground speed to the state's stride, for a state the graph
    /// rate-scales, else the state's fixed `playbackRate`. Both through the
    /// state's own policy (`MeshAnimation::stride_playback_target`). Returns
    /// the rate.
    fn pose_rate(&mut self, enemy: EntityId) -> f32 {
        use postretro_entities::components::brain::TravelVariant;
        use postretro_entities::components::mesh::PlaybackRateTarget;

        let travel = agent_steering::path_state(&self.reg, enemy)
            .expect("the brute steers")
            .steering_velocity();
        let speed = Vec3::new(travel.x, 0.0, travel.z).length();
        let move_speed = self
            .reg
            .get_component::<AgentComponent>(enemy)
            .unwrap()
            .move_speed;
        let brain = self.reg.get_component::<BrainComponent>(enemy).unwrap();
        let mut mesh = self
            .reg
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .clone();
        let model = mesh.model.clone();
        let animation = mesh.animation.as_mut().unwrap();
        let current = animation.current_state.clone();
        let scaled = animation.speed_scale
            && brain
                .rate_scaled_states()
                .iter()
                .any(|state| TravelVariant::of_state(state, &current).is_some());
        let stride = scaled.then(|| {
            let declared = &animation.states[&current];
            let derived = self
                .clips
                .get_by_name(&model)
                .and_then(|model| model.state_clip(declared.clip_index, &declared.clip))
                .and_then(|clip| clip.travel_speed);
            MeshAnimation::locomotion_rate_ratio(
                speed,
                declared.effective_travel_speed(derived),
                move_speed,
            )
        });
        let rate = match animation.stride_playback_target(stride) {
            PlaybackRateTarget::Stride {
                raw_ratio,
                max_rate,
            } => MeshAnimation::normalized_playback_rate_capped(raw_ratio, max_rate),
            PlaybackRateTarget::Fixed(rate) => rate,
        };
        animation.rate = rate;
        self.reg.set_component(enemy, mesh).unwrap();
        rate
    }
}

/// The shipped brute on an open floor with the player at `player_at` from it,
/// AI, steering and the pose stage's playback rate ticked at 60 Hz for
/// `seconds`, its offense seated in `tell` as soon as it engages. Before each
/// tick `place`, given the paths so far, may move the player. `attack` names
/// the attack whose fires are recorded.
fn brute_run(
    tell: &str,
    attack: &str,
    player_at: Vec3,
    seconds: f32,
    place: impl FnMut(&Shipped, EntityId, &[String]) -> Option<Vec3>,
) -> BruteRun {
    brute_run_in(dev_manifest(), tell, attack, player_at, seconds, place)
}

/// [`brute_run`] on `manifest`, the dev mod's or an edit of it. The floor is
/// 64 m a side, room for a 50 m leap.
fn brute_run_in(
    manifest: ModManifestResult,
    tell: &str,
    attack: &str,
    player_at: Vec3,
    seconds: f32,
    mut place: impl FnMut(&Shipped, EntityId, &[String]) -> Option<Vec3>,
) -> BruteRun {
    let floor = OpenFloor::wide();
    let (world, nav) = (floor.collision_world(), floor.nav_graph());
    let home = brute_home();
    let mut shipped = Shipped::new(manifest, home + player_at);
    let brute = spawn_shipped(
        &mut shipped.reg,
        &shipped.manifest,
        &mut shipped.clips,
        "reference_enemy",
        home,
    );
    give_reference_armor(&mut shipped.reg, brute);
    let mut run = BruteRun::default();
    let mut seated = false;
    for tick in 0..(seconds / DT) as usize {
        if !seated && shipped.path(brute).ends_with("/approach") {
            shipped.seat_offense(brute, tell);
            seated = true;
        }
        if let Some(at) = place(&shipped, brute, &run.paths) {
            shipped.place_player(at);
        }
        let at = shipped
            .reg
            .get_component::<Transform>(brute)
            .unwrap()
            .position;
        let cooldown = shipped.cooldown(brute, attack);
        let hp = player_hp(&shipped.reg, shipped.player);
        run_ai_tick_with_navigation_and_impact(
            &mut shipped.reg,
            &mut shipped.runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav::none(),
                nav_graph: Some(&nav),
                collision_world: Some(&world),
                descriptors: &shipped.manifest.entities,
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: Some(&shipped.clips),
            },
            |_| {},
        );
        agent_steering::tick(
            &mut shipped.reg,
            &world,
            Some(&nav),
            postretro_collision::EnvironmentView::level_only(STEER_GRAVITY),
            DT,
        );
        run.rates.push(shipped.pose_rate(brute));
        if shipped.cooldown(brute, attack) > cooldown {
            run.fired.push(tick);
        }
        run.cooldowns.push(shipped.cooldown(brute, attack));
        let lost = hp - player_hp(&shipped.reg, shipped.player);
        if lost > 0.0 {
            run.hits.push((tick, lost));
        }
        let now = shipped
            .reg
            .get_component::<Transform>(brute)
            .unwrap()
            .position;
        let player = shipped
            .reg
            .get_component::<Transform>(shipped.player)
            .unwrap()
            .position;
        run.speed.push(distance_xz(at, now) / DT);
        run.distance.push(distance_xz(now, player));
        run.positions.push(now);
        run.players.push(player);
        run.spots.push(
            shipped
                .reg
                .get_component::<BrainComponent>(brute)
                .unwrap()
                .leap_spot,
        );
        run.steering.push(
            shipped
                .reg
                .get_component::<AgentComponent>(brute)
                .unwrap()
                .activity_speed,
        );
        run.states.push(shipped.anim_state(brute));
        run.paths.push(shipped.path(brute));
    }
    let mesh = shipped.reg.get_component::<MeshComponent>(brute).unwrap();
    for state in mesh.animation.as_ref().unwrap().states.keys() {
        let clip = shipped.clip(brute, state);
        if let Some(hit) = clip.event_time("hit") {
            run.moments.insert(state.clone(), (hit, clip.duration));
        }
    }
    run
}

/// The shipped brute's charge from `bellow`. While it holds the tell, `place`
/// may move the player.
fn charge_run(place: impl Fn(&Shipped, EntityId) -> Option<Vec3>) -> BruteRun {
    let (home, player_at) = charge_home();
    brute_run(
        "bellow",
        "charge",
        player_at - home,
        8.0,
        |shipped, brute, _| {
            if shipped.path(brute).ends_with("/charge_bellow") {
                place(shipped, brute)
            } else {
                None
            }
        },
    )
}

// Sp6, shipped: the brute's charge under `bellow` lands its contact on reach,
// once per entry. A target in reach as the release begins is hit on its entry
// tick; one that backed off is chased down at twice the brute's pace and hit
// on reach; one too far to catch sees no hit, and the recover fires nothing.
#[test]
fn shipped_brute_charge_lands_on_reach_once_or_recovers_with_no_hit() {
    let (_, start) = charge_home();

    // Held ground: hit as the release begins, once.
    let held = charge_run(|_, _| None);
    let release = held.entered("charge_release");
    assert_eq!(
        held.fired,
        vec![release],
        "in reach on entry: {:?}",
        held.paths
    );
    assert!(
        held.paths[release + 1..]
            .iter()
            .any(|path| path.ends_with("/charge_recover")),
        "the charge recovers: {:?}",
        held.paths
    );

    // Backed off out of reach during the roar: chased at twice the pace.
    let backed_off = charge_run(|shipped, brute| Some(shipped.away_from(brute, start, 6.0)));
    let release = backed_off.entered("charge_release");
    let recover = backed_off.entered("charge_recover");
    assert_eq!(
        backed_off.fired.len(),
        1,
        "one contact per entry: {:?}",
        backed_off.paths
    );
    let fired = backed_off.fired[0];
    assert!(
        (release + 1..recover).contains(&fired),
        "the contact lands after the chase, before the recover: {fired} in {release}..{recover}"
    );
    assert!(
        backed_off.steering[release..recover]
            .iter()
            .all(|&speed| speed == 2.0),
        "the release steers at `speed: 2`: {:?}",
        &backed_off.steering[release..recover]
    );
    let top = backed_off.speed[release..=fired]
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        top > 3.0 * 1.5,
        "the charge outruns the brute's 3 m/s pace: {top} m/s"
    );

    // Out of reach for the whole charge, though on the floor's navmesh: no
    // hit, and the recover never fires. The whiffed charge still starts its
    // cooldown as it leaves the release, and only then.
    let fled = charge_run(|shipped, brute| Some(shipped.away_from(brute, start, 20.0)));
    let recover = fled.entered("charge_recover");
    assert!(
        fled.hits.is_empty(),
        "never reached, never hit: {:?}",
        fled.paths
    );
    assert_eq!(
        fled.fired,
        vec![recover],
        "the whiff starts the cooldown on leaving the release: {:?}",
        fled.paths
    );
    assert!(
        fled.paths[recover..]
            .iter()
            .any(|path| path.ends_with("/cock")),
        "it rests in its ending stance: {:?}",
        fled.paths
    );
}

/// How long a stance's pose held before its strike released, in
/// milliseconds: from the entry tick of its rest, where the pose is taken
/// and its time starts, to its release's. The hold carries that time.
fn held_ms(run: &BruteRun, rest: &str, release: &str) -> f32 {
    (run.entered(release) - run.entered(rest)) as f32 * DT * 1000.0
}

/// A tell holding exactly `ms` releases on the first evaluation its pose's
/// time reaches it: within two ticks of it, since f32 ticks may fall a hair
/// short.
fn assert_held_exactly(held: f32, ms: f32, run: &BruteRun) {
    assert!(
        (ms..ms + 2000.0 * DT).contains(&held),
        "the tell holds {ms} ms: {held} ms, {:?}",
        run.paths
    );
}

/// Ticks from a loop's restart at clip time 0, played at `rate`, until each of
/// its first `passes` passes of a moment at `event` on a clip of `duration`,
/// the restart tick being 0.
fn zero_phase_pass_ticks(event: f32, duration: f32, rate: f32, passes: usize) -> Vec<usize> {
    const SLACK: f64 = 1.0e-4;
    let step = f64::from(DT) * f64::from(rate);
    (0..passes)
        .map(|pass| {
            let at = f64::from(event) + pass as f64 * f64::from(duration);
            (0..)
                .find(|&tick| tick as f64 * step + SLACK >= at)
                .expect("a finite moment")
        })
        .collect()
}

// Sp3, shipped: the dev brute's charge strides its `run` past the engine's
// 1.5x default. The state's raised `maxPlaybackRate` (a dev-content
// exception) lets the stride rule play it at its steered speed, twice the
// brute's 3 m/s, over the state's 2.2 m/s stride: about 2.7x, under the 3x cap.
#[test]
fn shipped_brute_charge_strides_run_past_the_default_playback_cap() {
    use postretro_entities::components::mesh::RATE_MAX;

    let (_, start) = charge_home();
    let run = charge_run(|shipped, brute| Some(shipped.away_from(brute, start, 6.0)));
    let release = run.entered("charge_release");
    let recover = run.entered("charge_recover");
    assert!(
        run.states[release..recover]
            .iter()
            .all(|state| state == "run"),
        "the charge plays `run`: {:?}",
        &run.states[release..recover]
    );
    let top = run.rates[release..recover]
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    let stride = 2.0 * 3.0 / 2.2;
    assert!(
        top > RATE_MAX && (top - stride).abs() <= 0.05,
        "the charge strides at its steered ratio {stride}, past {RATE_MAX}: {top}"
    );
}

// The combo: `flurry` holds the drawn fist exactly 2,000 ms, then the release
// plays `jab_combo` (`jab_quick` at a fixed 2.5x) from clip time 0 and lands a
// 5-point jab at `hit` on each of its three passes, about 320 ms apart, under
// one turn. It recovers leaning back and rests coiled.
#[test]
fn shipped_brute_combo_lands_three_jabs_320_ms_apart_after_a_2000_ms_hold() {
    let run = brute_run(
        "flurry",
        "combo",
        Vec3::new(1.5, 0.0, 0.0),
        6.0,
        |_, _, _| None,
    );
    assert_held_exactly(held_ms(&run, "flurry", "combo_release"), 2000.0, &run);
    let release = run.entered("combo_release");
    let recover = run.entered("combo_recover");
    assert!(
        run.states[release..recover]
            .iter()
            .all(|state| state == "jab_combo")
            && run.rates[release..recover].iter().all(|&rate| rate == 2.5),
        "the release plays `jab_combo` at 2.5x: {:?} {:?}",
        &run.states[release..recover],
        &run.rates[release..recover]
    );

    let (hit, pass) = run.moments["jab_combo"];
    let expected: Vec<usize> = zero_phase_pass_ticks(hit, pass, 2.5, 3)
        .into_iter()
        .map(|tick| release + tick)
        .collect();
    assert_eq!(
        run.hits_in(release, recover),
        expected.iter().map(|&tick| (tick, 5.0)).collect::<Vec<_>>(),
        "three 5-point jabs, one per pass at `hit`: {:?}",
        &run.paths[release..recover]
    );
    assert_eq!(run.fired, expected, "each jab restarts the one cooldown");
    for pair in expected.windows(2) {
        let apart = (pair[1] - pair[0]) as f32 * DT * 1000.0;
        assert!(
            (apart - 320.0).abs() <= 1000.0 * DT,
            "the jabs land about 320 ms apart: {apart} ms"
        );
    }
    assert!(
        recover > expected[2],
        "the release lasts through its last pass"
    );
    assert_eq!(
        run.states[recover], "swipe_windup",
        "it recovers leaning back"
    );
    assert!(
        entry_tick(&run.paths, "coil", recover).is_some(),
        "it rests coiled: {:?}",
        &run.paths[recover..]
    );
}

/// The leap's cap: `speed: 14` times the brute's 3 m/s `moveSpeed`.
const LEAP_TOP_SPEED: f32 = 14.0 * 3.0;
/// The leap's `standoffDistance`: its spot lies this far short of the target.
const LEAP_STANDOFF: f32 = 1.5;
/// How near its spot an agent stops: 1.5 capsule radii (0.35 m).
const LEAP_ARRIVAL_BAND: f32 = 1.5 * 0.35;

/// The shipped brute's leap from `crouch` on `manifest`, run for `seconds`.
/// The player starts 10 m off along +X, inside the brute's 16 m detection,
/// and steps to `distance` m along +X as the brute takes the crouch, so the
/// tell queues from there. With `sidestep`, the player steps 8 m along +Z one
/// second into the release, 200 ms before the hit.
fn leap_run_in(
    manifest: ModManifestResult,
    distance: f32,
    sidestep: bool,
    seconds: f32,
) -> BruteRun {
    let mut stepped_out = false;
    brute_run_in(
        manifest,
        "crouch",
        "leap",
        Vec3::new(10.0, 0.0, 0.0),
        seconds,
        move |shipped, _, paths| {
            if !stepped_out && paths.last().is_some_and(|path| path.ends_with("/crouch")) {
                stepped_out = true;
                return Some(brute_home() + Vec3::new(distance, 0.0, 0.0));
            }
            let release = entry_tick(paths, "leap_release", 0)?;
            (sidestep && paths.len() == release + (1.0 / DT) as usize).then(|| {
                let at = shipped
                    .reg
                    .get_component::<Transform>(shipped.player)
                    .unwrap()
                    .position;
                at + Vec3::new(0.0, 0.0, 8.0)
            })
        },
    )
}

fn leap_run(distance: f32, sidestep: bool, seconds: f32) -> BruteRun {
    leap_run_in(dev_manifest(), distance, sidestep, seconds)
}

impl BruteRun {
    /// The hits that landed from `from` up to, not including, `to`.
    fn hits_in(&self, from: usize, to: usize) -> Vec<(usize, f32)> {
        self.hits
            .iter()
            .copied()
            .filter(|&(tick, _)| (from..to).contains(&tick))
            .collect()
    }
}

impl BruteRun {
    /// The landing spot a leap released on tick `release` should fix: the
    /// standoff short of where the player stood, toward where the brute
    /// stood, as the tick began.
    fn expected_spot(&self, release: usize) -> Vec3 {
        let (brute, player) = (self.positions[release - 1], self.players[release - 1]);
        let away = Vec3::new(brute.x - player.x, 0.0, brute.z - player.z).normalize();
        player + away * LEAP_STANDOFF
    }

    /// The spot the leap released on tick `release` fixed, asserting it held
    /// for every tick of the release.
    fn fixed_spot(&self, release: usize, recover: usize) -> Vec3 {
        let spot = self.spots[release].expect("the leap fixes a spot on takeoff");
        assert!(
            self.spots[release..recover]
                .iter()
                .all(|held| *held == Some(spot)),
            "the spot holds through the release, never re-aimed: {:?}",
            &self.spots[release..recover]
        );
        spot
    }
}

// The leap, near and far (owner addition, playtest round): from 5 m, and from
// the edge of its 50 m queue, the brute takes `crouch` and its tell waits in
// line there (`queueRange: 50`), holds exactly 666 ms, then leaps. On takeoff
// it fixes its landing spot 1.5 m short of the target, then flies there paced
// to land on the clip's `hit` while `leap_strike` plays at 1x, and slams
// once, for 18. Its pace follows the distance: a crawl for the short leap,
// its 42 m/s cap for the long one, never above it. Replaces the 12 m and 15 m
// cases of the fixed-speed `chaseTarget` leap, which steered at 3.5x and
// could not reach beyond about 12 m.
#[test]
fn shipped_brute_leap_lands_on_its_spot_at_the_clip_hit_from_near_and_far() {
    let mut tops = Vec::new();
    for distance in [5.0, 50.0] {
        let run = leap_run(distance, false, 6.0);
        let hold = run.entered("leap_crouch");
        assert!(
            (run.distance[hold] - distance).abs() <= 0.5,
            "the tell queues from about {distance} m: {}",
            run.distance[hold]
        );
        assert_held_exactly(held_ms(&run, "crouch", "leap_release"), 666.0, &run);
        let release = run.entered("leap_release");
        let recover = run.entered("leap_recover");
        let (hit, _) = run.moments["leap_strike"];
        let moment = release + one_shot_moment_tick(hit);
        assert_eq!(
            run.hits_in(release, recover),
            vec![(moment, 18.0)],
            "one slam as `leap_strike` reaches `hit` from {distance} m: {:?}",
            &run.paths[release..recover]
        );
        assert_eq!(run.fired, vec![moment]);

        let spot = run.fixed_spot(release, recover);
        let expected = run.expected_spot(release);
        assert!(
            distance_xz(spot, expected) <= 1.0e-3,
            "the spot is the standoff short of the target at takeoff: {spot} vs {expected}"
        );
        assert!(
            distance_xz(run.positions[moment - 1], spot) <= LEAP_ARRIVAL_BAND + 0.05,
            "from {distance} m it stands on its spot as the hit lands: {} m off",
            distance_xz(run.positions[moment - 1], spot)
        );
        let flight = distance_xz(run.positions[release - 1], spot);
        let halfway = distance_xz(run.positions[release + (moment - release) / 2], spot);
        assert!(
            (0.25 * flight..0.75 * flight).contains(&halfway),
            "from {distance} m it is still on its way halfway to the hit: {halfway} of {flight} m left"
        );
        assert!(
            run.distance[moment] <= 3.5,
            "the leap from {distance} m lands in slam range: {} m",
            run.distance[moment]
        );
        assert!(
            run.steering[release..recover]
                .iter()
                .all(|&speed| speed == 14.0),
            "the release's `speed: 14` caps its pace"
        );
        let top = run.speed[release..=moment]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        assert!(
            top <= LEAP_TOP_SPEED + 0.01,
            "the leap never passes its cap: {top} m/s"
        );
        tops.push(top);
        // `scaleWithSpeed: false` holds the strike at 1x however fast it
        // moves, and `run`'s raised cap never reaches it.
        assert!(
            run.states[release..recover]
                .iter()
                .all(|state| state == "leap_strike")
                && run.rates[release..recover].iter().all(|&rate| rate == 1.0),
            "the leap plays `leap_strike` at 1x: {:?}",
            &run.rates[release..recover]
        );
        assert_eq!(run.states[recover], "idle", "it stands up on `idle`");
        assert!(
            entry_tick(&run.paths, "coil", recover).is_some(),
            "it rests coiled: {:?}",
            &run.paths[recover..]
        );
    }
    assert!(
        tops[0] < 6.0 && tops[1] > 0.9 * LEAP_TOP_SPEED,
        "the pace follows the distance: {tops:?} m/s"
    );
}

// A target that sidesteps 8 m during the leap is out of slam range at the
// clip's `hit`. The leap does not re-aim: it still lands on the spot it fixed
// on takeoff, and whiffs, deals nothing, and still recovers and rests coiled.
// The enemy committed, so the whiff starts the leap's 6000 ms cooldown as it
// leaves the release, and it does not leap again until that has run out.
// Updated for the fixed spot: the `chaseTarget` leap steered after the
// target, and whiffed only because 10.5 m/s could not keep up.
#[test]
fn shipped_brute_leap_lands_on_its_spot_and_whiffs_when_the_target_sidesteps() {
    const LEAP_COOLDOWN_MS: f32 = 6000.0;
    let run = leap_run(12.0, true, 12.0);
    let release = run.entered("leap_release");
    let recover = run.entered("leap_recover");
    let (hit, _) = run.moments["leap_strike"];
    let moment = release + one_shot_moment_tick(hit);
    let spot = run.fixed_spot(release, recover);
    assert!(
        distance_xz(spot, run.expected_spot(release)) <= 1.0e-3,
        "the spot is fixed from the target's position at takeoff"
    );
    assert!(
        distance_xz(run.positions[moment - 1], spot) <= LEAP_ARRIVAL_BAND + 0.05,
        "it lands on the spot it fixed, not after the target: {} m off",
        distance_xz(run.positions[moment - 1], spot)
    );
    assert!(
        run.distance[moment] > 3.5,
        "fixture: the target is out of slam range at the hit: {} m",
        run.distance[moment]
    );
    assert!(
        run.hits_in(release, recover).is_empty(),
        "a whiff deals nothing: {:?}",
        run.hits
    );
    assert!(recover > moment, "the release still ends at its moment");
    assert!(
        entry_tick(&run.paths, "coil", recover).is_some(),
        "it rests coiled: {:?}",
        &run.paths[recover..]
    );
    assert_eq!(
        run.fired.first(),
        Some(&recover),
        "the whiff starts the cooldown on leaving the release, not before: {:?}",
        run.fired
    );
    assert_eq!(
        run.cooldowns[recover], LEAP_COOLDOWN_MS,
        "the leap's full cooldown"
    );
    let ready = recover + (LEAP_COOLDOWN_MS / 1000.0 / DT).floor() as usize;
    assert!(
        run.cooldowns.len() > ready + 60,
        "fixture: the run outlasts the cooldown"
    );
    if let Some(again) = entry_tick(&run.paths, "leap_release", recover) {
        assert!(
            again > ready,
            "no second leap until the cooldown has run out: tick {again}, ready at {ready}"
        );
    }
    assert!(
        run.paths[recover..ready]
            .iter()
            .all(|path| !path.ends_with("/leap_crouch") && !path.ends_with("/leap_release")),
        "the leap neither tells nor releases while it cools down: {:?}",
        &run.paths[recover..ready]
    );
}

// The cap holds: the dev brute with its leap's `speed` cut to 4 (12 m/s)
// leaps at a target 30 m off, beyond the reach of 12 m/s by the hit. It flies
// at no more than its cap, falls short, and whiffs.
#[test]
fn shipped_brute_leap_capped_below_its_distance_falls_short_and_whiffs() {
    let mut manifest = dev_manifest();
    let brute = manifest
        .entities
        .iter_mut()
        .find(|entity| entity.canonical_name.as_deref() == Some("reference_enemy"))
        .expect("the dev mod ships the brute");
    let behavior = brute.behavior.as_mut().expect("the brute behaves");
    let BehaviorLayerDescriptor::Graph(offense) = behavior
        .envelope
        .activities
        .get_mut("engage")
        .and_then(|engage| engage.layers.get_mut("offense"))
        .expect("the brute's offense")
    else {
        panic!("the offense is a graph");
    };
    let leap = offense
        .activities
        .get_mut("leap_release")
        .expect("the leap's release");
    assert_eq!(leap.motion, Some(postretro_foundation::MotionVerb::Leap));
    leap.speed = Some(4.0);

    let run = leap_run_in(manifest, 30.0, false, 6.0);
    let release = run.entered("leap_release");
    let recover = run.entered("leap_recover");
    let (hit, _) = run.moments["leap_strike"];
    let moment = release + one_shot_moment_tick(hit);
    let top = run.speed[release..=moment]
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        (11.5..=12.0 + 0.01).contains(&top),
        "it flies at its 12 m/s cap and no faster: {top} m/s"
    );
    let flown = distance_xz(run.positions[release - 1], run.positions[moment]);
    assert!(
        flown < 15.0,
        "12 m/s for 1.2 s falls short of the 28.5 m to its spot: {flown} m"
    );
    assert!(
        run.distance[moment] > 3.5 && run.hits_in(release, recover).is_empty(),
        "short of slam range at the hit, it whiffs: {} m, {:?}",
        run.distance[moment],
        run.hits
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

impl Shipped {
    /// Seat `enemy`'s offense level, at depth 1, in `activity`.
    fn seat_offense(&mut self, enemy: EntityId, activity: &str) {
        let mut brain = self
            .reg
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .clone();
        let BehaviorLayerDescriptor::Graph(offense) =
            &brain.graph.envelope.activities["engage"].layers["offense"]
        else {
            panic!("the offense is a graph");
        };
        let index = offense
            .activities
            .keys()
            .position(|name| name == activity)
            .unwrap_or_else(|| panic!("the offense declares `{activity}`"));
        assert!(brain.enter_activity_at(1, index));
        self.reg.set_component(enemy, brain).unwrap();
    }

    /// The animation state `enemy`'s mesh plays.
    fn anim_state(&self, enemy: EntityId) -> String {
        self.reg
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone()
    }
}

/// One aimed strike from the limitator's `aim`: every tick's path and turn,
/// the ticks a shot left the rifle, and the release's moment tick.
#[derive(Debug)]
struct AimedStrike {
    paths: Vec<String>,
    turns: Vec<bool>,
    shots: Vec<usize>,
    moment: usize,
}

impl AimedStrike {
    fn entered(&self, leaf: &str) -> usize {
        entry_tick(&self.paths, leaf, 0)
            .unwrap_or_else(|| panic!("the limitator enters `{leaf}`: {:?}", self.paths))
    }
}

/// Seat `enemy` in its `aim` rest as it starts strafing, and tick until its
/// strike settles.
fn aimed_strike(shipped: &mut Shipped, enemy: EntityId) -> AimedStrike {
    let kick = loop_moment_tick(enemy, shipped.clip(enemy, "shoot"), "kick");
    let (mut paths, mut turns, mut shots) = (Vec::new(), Vec::new(), Vec::new());
    let mut seated = false;
    for tick in 0..(6.0 / DT) as usize {
        if !seated && shipped.path(enemy).ends_with("/wait") {
            shipped.seat_offense(enemy, "aim");
            seated = true;
        }
        let result = shipped.tick();
        shots.extend((!result.projectile_spawns.is_empty()).then_some(tick));
        paths.push(shipped.path(enemy));
        turns.push(shipped.holds_turn(enemy));
        if paths[tick].ends_with("/settle") {
            let release = entry_tick(&paths, "shoot_release", 0).expect("the aim releases");
            return AimedStrike {
                paths,
                turns,
                shots,
                moment: release + kick,
            };
        }
    }
    panic!("the aimed strike settles within 6 s: {paths:?}, shots: {shots:?}");
}

// K2: the limitator's lowered `shoot` release fires its first shot as
// `firing_rifle` reaches `kick` from the enemy's drawn phase and leaves on the
// evaluation after it; its recover keeps the turn and the firing loop while
// the burst's other two shots land, then it settles. At every drawn phase,
// with no missing-event warning once its clips bind. `rifle_trooper` and
// `rifle_warrior` lower the same graph.
#[test]
fn shipped_limitator_aimed_shoot_fires_at_the_kick_and_lands_its_burst_at_every_phase() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let manifest = dev_manifest();
    let behavior = |name: &str| descriptor(&manifest, name).behavior.clone();
    assert_eq!(behavior("rifle_trooper"), behavior("limitator"));
    assert_eq!(behavior("rifle_warrior"), behavior("limitator"));

    let mut waits = std::collections::BTreeSet::new();
    let mut pass_ticks = 0;
    for fillers in 0..24 {
        let (mut shipped, limitator) = rifle_scene(manifest.clone(), "limitator", fillers);
        let clip = shipped.clip(limitator, "shoot");
        assert_eq!(
            clip.name, "firing_rifle",
            "fixture: the release plays the rifle loop"
        );
        pass_ticks = (clip.duration / DT).floor() as usize;
        let strike = aimed_strike(&mut shipped, limitator);
        let hold = strike.entered("shoot_aim");
        let release = strike.entered("shoot_release");
        let recover = strike.entered("shoot_recover");
        let settled = strike.entered("settle");
        assert_eq!(
            strike.shots.first().copied(),
            Some(strike.moment),
            "the first shot fires at the loop's next `kick` from the drawn phase: {strike:?}"
        );
        assert_eq!(
            recover,
            strike.moment + 1,
            "the release leaves on the evaluation after its moment: {strike:?}"
        );
        assert!(
            strike.paths[release..recover]
                .iter()
                .all(|path| path.ends_with("/shoot_release")),
            "{strike:?}"
        );
        assert_eq!(strike.shots.len(), 3, "the whole burst fires: {strike:?}");
        assert!(
            strike.shots[1..]
                .iter()
                .all(|&shot| strike.paths[shot].ends_with("/shoot_recover")),
            "the recover holds through the burst's other shots: {strike:?}"
        );
        assert!(
            settled > strike.shots[2],
            "the strike settles after its third shot: {strike:?}"
        );
        assert!(
            strike.turns[hold..settled].iter().all(|&held| held) && !strike.turns[settled],
            "the turn is held from the hold through the burst and ends in `settle`: {strike:?}"
        );
        waits.insert(strike.moment - release);
    }
    assert!(
        waits.len() >= 6 && waits.iter().any(|&wait| wait + 2 >= pass_ticks),
        "fixture: the sweep draws phases across the {pass_ticks}-tick pass, \
         a near-full wait for the kick included: {waits:?}"
    );
    let missing: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level <= log::Level::Warn && record.message.contains("cannot time it")
        })
        .collect();
    assert!(missing.is_empty(), "no missing-event warning: {missing:#?}");
}

// D5: 15 damage in the limitator's aim and 15 more in its hold break the
// pose's 25 into `flinch`, which plays `hit_reaction` and fires nothing. The
// flinch resumes on its timer into the reposition, whose activity sets the
// next state, so the one-shot reaction never freezes on its last frame. A
// flinch whose target is lost leaves the same way, for `idle`.
#[test]
fn shipped_limitator_heavy_fire_during_its_aim_flinches_it_and_it_resumes_on_its_timer() {
    for lose_target in [false, true] {
        let (mut shipped, limitator) = rifle_scene(dev_manifest(), "limitator", 0);
        let (mut seated, mut flinched, mut left) = (None, None, None);
        let (mut paths, mut shots) = (Vec::new(), Vec::new());
        for tick in 0..(6.0 / DT) as usize {
            if seated.is_none() && shipped.path(limitator).ends_with("/wait") {
                shipped.seat_offense(limitator, "aim");
                seated = Some(tick);
            }
            if seated.is_some_and(|seat| tick == seat + 1 || tick == seat + 10) {
                crate::ai_tests::stance_pose::hit(&mut shipped.reg, limitator, 15.0);
            }
            if lose_target && flinched.is_some_and(|at| tick == at + 5) {
                shipped.reg.despawn(shipped.player).unwrap();
            }
            let result = shipped.tick();
            shots.extend((!result.projectile_spawns.is_empty()).then_some(tick));
            let path = shipped.path(limitator);
            if flinched.is_none() && path.ends_with("/flinch") {
                flinched = Some(tick);
                assert_eq!(
                    shipped.anim_state(limitator),
                    "hit_reaction",
                    "the flinch plays the reaction"
                );
                assert!(!shipped.holds_turn(limitator), "the break ends the turn");
            }
            let in_flinch = path.ends_with("/flinch");
            paths.push(path);
            if flinched.is_some() && !in_flinch {
                left = Some((tick, shipped.anim_state(limitator)));
                break;
            }
        }
        let seat = seated.expect("the limitator strafes");
        let at = flinched.unwrap_or_else(|| panic!("the aim breaks into `flinch`: {paths:?}"));
        let (left, state) = left.unwrap_or_else(|| panic!("the flinch ends: {paths:?}"));
        assert!(
            paths[seat + 9].ends_with("/shoot_aim"),
            "fixture: the second hit lands in the hold: {paths:?}"
        );
        assert_eq!(
            at,
            seat + 10,
            "the pose's summed damage breaks it on its first evaluation at 25: {paths:?}"
        );
        assert!(shots.is_empty(), "a broken aim fires nothing: {shots:?}");
        if lose_target {
            assert_eq!(left, at + 5, "losing its target leaves at once: {paths:?}");
            assert_eq!(paths[left], "idle", "{paths:?}");
            assert_eq!(state, "idle", "the idle activity sets its state");
        } else {
            // Time in activity counts from the tick after entry, and f32
            // ticks may fall a hair short of 500: within two ticks of it.
            let ms = (left - at) as f32 * DT * 1000.0;
            assert!(
                (500.0..500.0 + 2000.0 * DT).contains(&ms),
                "the flinch resumes on its 500 ms timer: {ms} ms, {paths:?}"
            );
            assert!(paths[left].ends_with("/move"), "{paths:?}");
            assert_eq!(state, "run", "the reposition sets its state");
        }
    }
}

// D5: within one engagement, each reposition's roll picks the next cycle:
// below 0.5 the limitator strafes and fires, else it stops and aims. Both
// branches come up, and each cycle's burst lands.
#[test]
fn shipped_limitator_reposition_roll_picks_strafe_and_fire_or_stop_and_aim() {
    let (mut shipped, limitator) = rifle_scene(dev_manifest(), "limitator", 0);
    let (mut paths, mut shots, mut rolls) = (Vec::<String>::new(), Vec::new(), Vec::new());
    for tick in 0..(45.0 / DT) as usize {
        let result = shipped.tick();
        shots.extend((!result.projectile_spawns.is_empty()).then_some(tick));
        let path = shipped.path(limitator);
        let entered = tick == 0 || !paths[tick - 1].ends_with("/move");
        if path.ends_with("/move") && entered {
            let brain = shipped
                .reg
                .get_component::<BrainComponent>(limitator)
                .unwrap();
            let roll = brain.activity_roll(1).expect("the offense level is active");
            rolls.push((tick, roll));
        }
        paths.push(path);
    }
    let engaged = paths
        .iter()
        .position(|path| path.starts_with("engage/"))
        .expect("the limitator engages");
    assert!(
        paths[engaged..]
            .iter()
            .all(|path| path.starts_with("engage/")),
        "one engagement throughout: {paths:?}"
    );

    // Every reposition but the last opens a whole cycle, ending at the next.
    let mut branches = Vec::new();
    for pair in rolls.windows(2) {
        let ((entered, roll), (next, _)) = (pair[0], pair[1]);
        let left = (entered..next)
            .find(|&tick| !paths[tick].ends_with("/move"))
            .expect("the reposition ends");
        let leaf = paths[left].rsplit('/').next().unwrap();
        let strafes = roll < 0.5;
        assert_eq!(
            leaf,
            if strafes { "wait" } else { "aim" },
            "the reposition entered on tick {entered} rolled {roll}: {:?}",
            &paths[entered..next]
        );
        let release = if strafes { "/burst" } else { "/shoot_release" };
        assert!(
            paths[left..next].iter().any(|path| path.ends_with(release)),
            "the cycle reaches its `{release}`: {:?}",
            &paths[left..next]
        );
        let fired = shots
            .iter()
            .filter(|&&shot| (left..next).contains(&shot))
            .count();
        assert_eq!(
            fired,
            3,
            "the cycle's burst lands: {:?}",
            &paths[left..next]
        );
        branches.push(strafes);
    }
    assert!(
        branches.len() >= 6,
        "fixture: enough whole cycles: {branches:?}"
    );
    assert!(
        branches.windows(2).any(|pair| pair[0] != pair[1])
            && branches.contains(&true)
            && branches.contains(&false),
        "consecutive cycles take both branches: {branches:?}"
    );
}

// The shipped brute held 2.2 m off, past jab reach and inside slam range, on
// a roll that names no tell there (0.6: neither the crouch, the roar nor a jab-
// reach fist) still reaches one: `press` rolls again and, held short of jab
// reach, takes the crouch. Stopped while it presses it plays its rest clip,
// not a run in place.
#[test]
fn shipped_brute_held_past_jab_reach_on_a_mid_roll_reaches_a_tell() {
    let mut shipped = Shipped::new(dev_manifest(), Vec3::new(2.2, 0.0, 0.0));
    let brute = shipped.spawn("reference_enemy");
    give_reference_armor(&mut shipped.reg, brute);
    // No navigation: the brute engages and chases, but cannot close.
    let mut engaged = false;
    for _ in 0..120 {
        shipped.tick();
        if shipped.path(brute).ends_with("/approach") {
            engaged = true;
            break;
        }
    }
    assert!(engaged, "the brute engages: {}", shipped.path(brute));
    let mut brain = shipped
        .reg
        .get_component::<BrainComponent>(brute)
        .unwrap()
        .clone();
    brain.activity_roll[1] = 0.6;
    shipped.reg.set_component(brute, brain).unwrap();

    let mut paths = Vec::new();
    let mut pressing_states = Vec::new();
    for _ in 0..(3.5 / DT) as usize {
        shipped.tick();
        let path = shipped.path(brute);
        if path.ends_with("/press") {
            pressing_states.push(shipped.anim_state(brute));
        }
        let still_closing = path.ends_with("/approach") || path.ends_with("/press");
        paths.push(path);
        if !still_closing {
            break;
        }
    }
    assert!(
        paths.iter().any(|path| path.ends_with("/press")),
        "the mid roll presses in: {paths:?}"
    );
    let last = paths.last().unwrap();
    assert!(
        !last.ends_with("/approach") && !last.ends_with("/press"),
        "the brute reaches a tell: {paths:?}"
    );
    assert!(
        pressing_states.iter().skip(1).all(|state| state == "idle"),
        "a stopped press plays rest: {pressing_states:?}"
    );
}
