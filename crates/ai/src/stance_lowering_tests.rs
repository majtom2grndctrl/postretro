// Stances and strikes lowered at load and ticked: authored TypeScript through
// scripts-build, mod init and `scripting-core` lowering into the AI tick on a
// clip rig at 60 Hz, and the stances brief's Scripting surface example as
// shipped `content/dev` content on the brute's committed clips.
// See: context/lib/entity_model.md §7c (Stances and attacks)

use postretro_entities::ScriptCtx;
use postretro_entities::components::brain::activity_at_path;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};

use super::*;
use crate::ai_tests::attack_turn_ticks::shipped_content::{
    dev_manifest, dev_registry, durable_player, spawn_shipped,
};

/// The strike's state: a one-second one-shot whose `hit` is at 0.5 s.
const STRIKE: &str = "smash_strike";
const STANCE_SOUND: &str = "fixtures/growl";

fn rig() -> Vec<ClipSpec> {
    vec![
        looping("smash_windup", 2.0, &[]),
        looping("swipe_windup", 2.0, &[]),
        one_shot(STRIKE, 1.0, &[("hit", 0.5)]),
    ]
}

/// `source`, a TypeScript start script default-exporting `defineMod`, bundled
/// by scripts-build and initialized as at boot: its first entity's lowered
/// behavior.
fn behavior_from_typescript(label: &str, source: &str) -> BehaviorGraphDescriptor {
    let root = std::env::temp_dir().join(format!(
        "postretro-ai-stance-lowering-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    std::fs::create_dir_all(&root).expect("temp mod root");
    let entry = root.join("start-script.ts");
    std::fs::write(&entry, source).expect("the start script writes");
    let bundle = postretro_script_compiler::bundle_entry(
        &std::fs::canonicalize(&entry).expect("the start script exists"),
    )
    .expect("the start script bundles");
    std::fs::remove_file(&entry).expect("the TypeScript source goes");
    std::fs::write(root.join("start-script.js"), bundle).expect("the bundle writes");

    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    let mut runtime = ScriptRuntime::new(
        &primitives,
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        &ctx,
    )
    .expect("script runtime constructs");
    let initialized = runtime.run_mod_init(&root);
    let _ = std::fs::remove_dir_all(&root);
    initialized.expect("the mod initializes");
    runtime
        .mod_manifest()
        .expect("mod init commits a manifest")
        .entities[0]
        .behavior
        .clone()
        .expect("the entity carries a behavior")
}

/// A brute that takes `overhead` from a chase and strikes `slam` from it,
/// landing in `ending` (TypeScript for a stance handle, or nothing for the
/// default: `slam` ends where it began).
fn strike_source(ending: Option<&str>) -> String {
    // `coil` keys rows only when a strike lands in it; a stance nothing
    // references names no activity.
    let (ending, coil_rows) = ending.map_or((String::new(), String::new()), |stance| {
        (
            format!("endingStance: {stance},"),
            format!("{stance}: [{{ to: \"approach\", when: brain.timeInActivityMs.ge(9000) }}],"),
        )
    });
    format!(
        r#"import {{ brain, defineAttack, defineEntity, defineMod, defineStance }} from "postretro";

        const overhead = defineStance({{ animation: "smash_windup", sound: "{STANCE_SOUND}", holdMs: 450 }});
        const coil = defineStance({{ animation: "swipe_windup", sound: "{STANCE_SOUND}" }});
        const slam = defineAttack({{
          damage: 8, maxRange: 2, cooldownMs: 1000,
          beginningStance: overhead, {ending}
          release: {{ animation: "{STRIKE}" }},
        }});

        export default defineMod({{ name: "Stances", id: "stances", version: "1", entities: [
          defineEntity({{ canonicalName: "brute", components: {{ behavior: {{
            initial: "approach", moveSpeed: 3,
            activities: {{ approach: {{ animation: "walk", motion: "chaseTarget" }} }},
            transitions: {{
              approach: [{{ to: overhead }}],
              overhead: [{{ to: slam }}, {{ to: "approach", when: brain.timeInActivityMs.ge(9000) }}],
              {coil_rows}
            }},
          }} }} }}),
        ] }});
        "#
    )
}

/// What one tick left on the enemy.
struct Tick {
    state: String,
    hit: bool,
    entered: Vec<String>,
    holds_turn: bool,
    planted: bool,
}

fn run(scene: &mut Scene, enemy: EntityId, ticks: usize) -> Vec<Tick> {
    (0..ticks)
        .map(|_| {
            let hp = player_hp(&scene.reg, scene.pawn);
            let result = scene.tick();
            let brain = scene.brain(enemy);
            Tick {
                state: scene.state(enemy),
                hit: player_hp(&scene.reg, scene.pawn) < hp,
                entered: entered(&result, enemy),
                holds_turn: brain.attack_turns.holds_any(),
                planted: scene.agent(enemy).destination.is_none(),
            }
        })
        .collect()
}

fn entered(result: &AiTickResult, enemy: EntityId) -> Vec<String> {
    result
        .events
        .iter()
        .filter(|emission| {
            matches!(emission.emitter, postretro_entities::Emitter::Entity { id, .. } if id == enemy)
        })
        .filter_map(|emission| match &emission.cue {
            postretro_entities::AiCue::Entered { graph, path } => {
                activity_at_path(graph, path).map(|(name, _)| name.to_string())
            }
            _ => None,
        })
        .collect()
}

/// The states visited, consecutive repeats collapsed.
fn visits(log: &[Tick]) -> Vec<&str> {
    let mut visits: Vec<&str> = Vec::new();
    for tick in log {
        if visits.last() != Some(&tick.state.as_str()) {
            visits.push(&tick.state);
        }
    }
    visits
}

fn first_in(log: &[Tick], state: &str) -> usize {
    log.iter()
        .position(|tick| tick.state == state)
        .unwrap_or_else(|| panic!("the enemy enters `{state}`"))
}

fn strike_scene(source: &str, label: &str) -> (Scene, EntityId, BehaviorGraphDescriptor) {
    let graph = behavior_from_typescript(label, source);
    let mut scene = Scene::new(rig());
    let enemy = scene.spawn(0, &graph, "approach");
    (scene, enemy, graph)
}

// End to end: authored TypeScript lowers at load and ticks. The enemy takes
// `overhead` from its chase, strikes `slam` from it once the pose has lasted
// its hold, releases at the strike clip's `hit`, recovers on the same clip,
// and lands in `coil`.
#[test]
fn an_authored_stance_and_strike_lower_at_load_and_tick_into_the_ending_stance() {
    let (mut scene, enemy, _) = strike_scene(&strike_source(Some("coil")), "e2e");
    let log = run(&mut scene, enemy, 150);
    assert_eq!(
        visits(&log),
        [
            "overhead",
            "slam_overhead",
            "slam_release",
            "slam_recover",
            "coil"
        ],
        "the strike's cycle, in order"
    );

    let rest = first_in(&log, "overhead");
    let release = first_in(&log, "slam_release");
    let recover = first_in(&log, "slam_recover");
    let coil = first_in(&log, "coil");
    let hits: Vec<usize> = (0..log.len()).filter(|&tick| log[tick].hit).collect();
    assert!(
        (rest + 27..=rest + 30).contains(&release),
        "the hold releases once the pose has lasted 450 ms: rest {rest}, release {release}"
    );
    assert_eq!(hits, [release + 30], "one hit, at the strike clip's `hit`");
    assert_eq!(
        recover,
        release + 31,
        "the release leaves the evaluation after its moment"
    );
    assert_eq!(
        coil,
        release + 61,
        "the recover continues the strike's clip and leaves the evaluation after it plays through"
    );
    assert!(log[coil + 1..].iter().all(|tick| tick.state == "coil"));
}

// S4: `{ to: overhead }` enters the rest, which holds no turn and plays the
// stance sound once; moving into the hold plays nothing. A level whose
// `initial` is a stance starts in its rest.
#[test]
fn s4_a_row_to_a_stance_enters_its_rest_holding_no_turn_with_one_sound() {
    let (mut scene, enemy, graph) = strike_scene(&strike_source(Some("coil")), "s4");
    assert_eq!(
        graph.envelope.activities["overhead"].sound.as_deref(),
        Some(STANCE_SOUND)
    );
    let log = run(&mut scene, enemy, 150);
    let rest = first_in(&log, "overhead");
    assert!(!log[rest].holds_turn, "the rest is unmarked");
    let hold = first_in(&log, "slam_overhead");
    let announced: Vec<(usize, &str)> = log[..hold + 5]
        .iter()
        .enumerate()
        .flat_map(|(tick, entry)| entry.entered.iter().map(move |name| (tick, name.as_str())))
        .collect();
    assert_eq!(
        announced,
        [(rest, "overhead")],
        "one announcement on entering the pose, none on moving into its hold"
    );

    let starting = behavior_from_typescript(
        "s4_initial",
        &strike_source(Some("coil")).replace("initial: \"approach\"", "initial: overhead"),
    );
    assert_eq!(starting.envelope.initial, "overhead");
    let mut reg = EntityRegistry::new();
    let started = spawn_enemy(
        &mut reg,
        Vec3::ZERO,
        BrainComponent::from_graph(&starting),
        50.0,
    );
    assert_eq!(enemy_state_path(&reg, started), "overhead");
}

// S9: an attack rests in its ending stance once its recovery ends, holding no
// turn there; with no `endingStance` it rests in its beginning stance, and its
// turn ends there.
#[test]
fn s9_a_strike_rests_in_its_ending_stance_holding_no_turn() {
    for (ending, rest) in [(Some("coil"), "coil"), (None, "overhead")] {
        let (mut scene, enemy, _) = strike_scene(&strike_source(ending), "s9");
        let log = run(&mut scene, enemy, 150);
        let release = first_in(&log, "slam_release");
        assert!(
            log[release..release + 61]
                .iter()
                .all(|tick| tick.holds_turn),
            "{rest}: the strike holds its turn through release and recover"
        );
        let landed = release + 61;
        assert_eq!(log[landed].state, rest);
        assert!(
            !log[landed].holds_turn,
            "{rest}: the turn ends as the strike lands in its ending stance"
        );
        assert_eq!(
            log[landed].entered,
            [rest],
            "{rest}: entering the ending stance from the recover is a fresh pose entry"
        );
    }
}

// S10: a strike entered from a chasing level stays planted: nested under a
// composite whose move layer chases, the rest, hold, release and recover never
// steer, while the level's own `approach` does.
#[test]
fn s10_a_strike_entered_from_a_chasing_level_stays_planted() {
    let source = format!(
        r#"import {{ brain, defineAttack, defineEntity, defineMod, defineStance }} from "postretro";

        const overhead = defineStance({{ animation: "smash_windup", holdMs: 450 }});
        const slam = defineAttack({{
          damage: 8, maxRange: 2, cooldownMs: 1000,
          beginningStance: overhead, release: {{ animation: "{STRIKE}" }},
        }});

        export default defineMod({{ name: "Stances", id: "stances", version: "1", entities: [
          defineEntity({{ canonicalName: "brute", components: {{ behavior: {{
            initial: "engage", moveSpeed: 3,
            activities: {{ engage: {{ animation: "walk", layers: {{
              move: ["chaseTarget"],
              offense: {{
                initial: "approach",
                activities: {{ approach: {{ animation: "walk" }} }},
                transitions: {{
                  approach: [{{ to: overhead, when: brain.timeInActivityMs.ge(200) }}],
                  overhead: [{{ to: slam }}, {{ to: "approach", when: brain.timeInActivityMs.ge(9000) }}],
                }},
              }},
            }} }} }},
            transitions: {{}},
          }} }} }}),
        ] }});
        "#
    );
    let graph = behavior_from_typescript("s10", &source);
    let mut scene = Scene::new(rig());
    let enemy = scene.spawn(0, &graph, "engage");
    let log = run(&mut scene, enemy, 150);
    let leaf = |tick: &Tick| {
        tick.state
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let rest = log
        .iter()
        .position(|tick| leaf(tick) == "overhead")
        .expect("the enemy takes the pose");
    assert!(
        log[..rest].iter().any(|tick| !tick.planted),
        "fixture: `approach` chases under its composite's move layer"
    );
    let strike: Vec<String> = log[rest..].iter().map(leaf).collect();
    for state in ["slam_overhead", "slam_release", "slam_recover"] {
        assert!(
            strike.iter().any(|leaf| leaf == state),
            "it enters `{state}`: {strike:?}"
        );
    }
    assert!(
        log[rest..].iter().all(|tick| tick.planted),
        "planted from the pose on: {:?}",
        log.iter()
            .map(|tick| (leaf(tick), tick.planted))
            .collect::<Vec<_>>()
    );
}

// S1: the stances brief's Scripting surface example runs as `content/dev`
// content: spawned from the dev mod's committed descriptor on the brute's
// clip library, it loads silently, takes a pose in reach of its target,
// strikes from it and lands the strike.
#[test]
fn s1_the_scripting_surface_example_runs_as_dev_content() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let manifest = dev_manifest();
    let loud: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Warn)
        // Predates stances; the dev mod's clean-load test allows the same lint.
        .filter(|record| {
            !record
                .message
                .contains("activities [startle] are unreachable")
        })
        .collect();
    assert!(
        loud.is_empty(),
        "the dev mod loads the example silently: {loud:#?}"
    );
    let mut reg = dev_registry(&manifest);
    let player = durable_player(&mut reg, Vec3::new(3.0, 0.0, 0.0));
    let mut clips = HitZoneStore::new();
    let brute = spawn_shipped(
        &mut reg,
        &manifest,
        &mut clips,
        "stance_surface_brute",
        Vec3::ZERO,
    );
    let mut runtime = AiRuntime::new();
    let (mut paths, mut hurt) = (Vec::new(), 0);
    for _ in 0..(10.0 / DT) as usize {
        let hp = player_hp(&reg, player);
        run_ai_tick_with_navigation_and_impact(
            &mut reg,
            &mut runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav::none(),
                nav_graph: None,
                collision_world: Some(&CollisionWorld::new()),
                descriptors: &manifest.entities,
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: Some(&clips),
            },
            |_| {},
        );
        hurt += usize::from(player_hp(&reg, player) < hp);
        paths.push(enemy_state_path(&reg, brute));
    }
    let reached = |leaf: &str| paths.iter().any(|path| path.ends_with(&format!("/{leaf}")));
    assert!(reached("overhead"), "it takes the pose: {paths:?}");
    assert!(
        reached("slam_overhead") || reached("sweep_overhead"),
        "it strikes from the pose: {paths:?}"
    );
    assert!(
        reached("slam_release") || reached("sweep_release"),
        "the strike releases: {paths:?}"
    );
    assert!(hurt > 0, "the strike lands: {paths:?}");
}
