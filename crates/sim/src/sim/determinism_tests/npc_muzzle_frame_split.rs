// E10 npc-weapon-mounts S4: an NPC's projectile launch point is a function of
// the fire tick. A gunner on the real limitator body and its AR_4 bake fires
// bursts through the whole sim tick; however frames split the ticks and however
// fast the animation clock runs, each shot leaves the same point, the bake's
// `muzzle` posed at the brain's clip time on the state its clip clock tracks.
// A child module of `determinism_tests` for its fixtures.
// See: context/lib/entity_model.md §2 (Weapon mounts) · §7c (Clip-timed attacks)

use super::*;
use crate::scripting_systems::hit_zones::ModelLoadWarningOwner;
use crate::scripting_systems::muzzle_pose::{ClipPose, MuzzleMount, posed_muzzle_world};
use postretro_ai::__postretro_sim::scripting_systems::hit_zones as ai_hit_zones;
use postretro_entities::components::mesh::MeshAnimation;
use postretro_entities::data_descriptors::MeshDescriptor;
use postretro_entities::{
    ComponentKind, EntityTypeDescriptor, FactionRegistry, FactionSentimentState,
};

const BODY: &str = "models/limitator/model.gltf";
const BAKE: &str = "models/ar_4/limitator.gltf";
const RIFLE: &str = "npc.rifle";
const CLASS: &str = "gunner";

/// A three-shot projectile burst, 120 ms apart, recovering 300 ms.
fn burst_rifle() -> EntityTypeDescriptor {
    let mut weapon: postretro_foundation::WeaponDescriptor = serde_json::from_value(
        serde_json::json!({
            "damage": 1.0,
            "range": 12.0,
            "primary": {
                "trigger": "press", "recoveryMs": 300,
                "steps": [{"kind": "shot"}, {"kind": "wait", "durationMs": 120},
                    {"kind": "shot"}, {"kind": "wait", "durationMs": 120}, {"kind": "shot"}]
            },
            "resolution": "projectile",
            "projectile": {
                "speed": 20.0, "radius": 0.15, "lifetimeMs": 2000.0,
                "visual": {"body": {"kind": "sprite", "sprite": "projectiles/test.png", "size": 0.25}}
            }
        }),
    )
    .expect("a valid projectile rifle");
    weapon.credit_source = Some("enemy.rifle".to_string());
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(RIFLE.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: Some(weapon),
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

/// The gunner's archetype: the limitator body with the rifle's bake on
/// `hand_r`, from its mount table.
fn gunner() -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(CLASS.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: Some(MeshDescriptor {
            model: BODY.to_string(),
            shadow_only: false,
            attachments: HashMap::new(),
            shadow_bias_scale: 1.0,
            animations: HashMap::new(),
            default_state: None,
            locomotion: None,
            mounts: Some(vec![
                postretro_entities::data_descriptors::MountDescriptor {
                    item: RIFLE.to_string(),
                    attachments: Some(HashMap::from([("hand_r".to_string(), BAKE.to_string())])),
                    muzzle_socket: None,
                },
            ]),
        }),
        health: None,
        behavior: None,
    }
}

/// `fire` on the rifle loop fires the burst; `rest` on the aim loop re-enters
/// it after 100 ms, and `fire` rests after 600 ms.
fn burst_graph() -> BehaviorGraphDescriptor {
    let mut graph = enemy_graph(0.0, "locomotion");
    let activity = |animation: &str, action| BehaviorActivityDescriptor {
        pose: None,
        stagger_to: None,
        passes: None,
        sound: None,
        animation: Some(animation.to_string()),
        motion: Some(MotionVerb::Hold),
        action,
        on_enter: None,
        facing: None,
        reposition: None,
        attack_turn: None,
        speed: None,
        layers: BTreeMap::new(),
    };
    let after_ms = |to: &str, ms: f32| postretro_foundation::GuardedRow {
        release_after: None,
        queue: None,
        after_moment: false,
        to: to.to_string(),
        when: IrNode::Ge {
            a: Box::new(IrNode::Input {
                name: postretro_foundation::BRAIN_TIME_IN_ACTIVITY_MS_INPUT.into(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(ms),
            }),
        },
    };
    graph.envelope = BehaviorGraphEnvelope {
        initial: "fire".to_string(),
        activities: BTreeMap::from([
            (
                "fire".to_string(),
                activity(
                    "shoot",
                    Some(ActionVerb::Attack {
                        name: "volley".to_string(),
                        at: None,
                        at_default: false,
                    }),
                ),
            ),
            ("rest".to_string(), activity("aim", None)),
        ]),
        transitions: BTreeMap::from([
            ("fire".to_string(), vec![after_ms("rest", 600.0)]),
            ("rest".to_string(), vec![after_ms("fire", 100.0)]),
        ]),
    };
    let mut volley = graph.attacks.values().next().expect("an attack").clone();
    volley.weapon = Some(RIFLE.to_string());
    (volley.damage, volley.max_range, volley.cooldown_ms) = (None, None, None);
    graph.attacks = BTreeMap::from([("volley".to_string(), volley)]);
    graph
}

/// One shot: its tick, its launch point, and the bake's `muzzle` posed on the
/// brain's tracked state at its clip time once the tick is done.
type Launch = (usize, Vec3, Vec3);

/// The launches of `ticks` ticks of the gunner bursting at the player, with
/// `ticks_per_frame` ticks sharing each frame's animation clock and that clock
/// running at `time_scale`. The models load into the sim's store and into one
/// of AI's own sim build, as `clip_timed_fire_ticks` loads them.
fn launches(ticks_per_frame: usize, time_scale: f64, ticks: usize) -> Vec<Launch> {
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let mut sim_clips = HitZoneStore::new();
    let mut ai_clips = ai_hit_zones::HitZoneStore::new();
    for model in [BODY, BAKE] {
        sim_clips.insert_from_load(model, &content, ModelLoadWarningOwner::GameSide);
        ai_clips.insert_from_load(
            model,
            &content,
            ai_hit_zones::ModelLoadWarningOwner::GameSide,
        );
        assert!(sim_clips.get_by_name(model).is_some(), "{model} loads");
    }
    let clips = &sim_clips.get_by_name(BODY).unwrap().clips;
    let state = |clip: &str| AnimationState {
        clip: clip.to_string(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        max_playback_rate: None,
        scale_with_speed: true,
        playback_rate: None,
        clip_index: clips.iter().position(|loaded| loaded.name == clip),
    };
    let states = HashMap::from([
        ("idle".to_string(), state("idle")),
        ("aim".to_string(), state("idle_aiming")),
        ("shoot".to_string(), state("firing_rifle")),
    ]);
    let descriptors = [burst_rifle(), gunner()];

    let world = floor_world();
    let nav_graph = open_floor_nav_graph();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = postretro_ai::AiRuntime::noticing_on_sight();
    let mut mover_states = MoverTickStateTable::default();
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let enemy = {
        let mut registry = registry.borrow_mut();
        spawn_player(&mut registry, Vec3::new(5.0, 1.21, 9.0));
        let enemy = registry.spawn(Transform {
            position: Vec3::new(5.0, 1.21, 5.0),
            ..Transform::default()
        });
        // Resting first: the agent settles onto the floor before its first
        // burst, so the pose after a fire tick is the pose it fired from.
        let mut brain = brain_in_state(&burst_graph(), "rest");
        brain.home_anchor = Vec3::new(5.0, 1.21, 5.0);
        registry.set_component(enemy, brain).unwrap();
        registry.entity_state_mut(enemy).unwrap().set(
            postretro_foundation::FACTION_STATE_FIELD,
            postretro_entities::DEFAULT_ENEMY_FACTION_INDEX,
        );
        registry
            .set_component(enemy, AgentComponent::new(0.35, 1.8, 0.4, 3.5))
            .unwrap();
        registry
            .set_component(
                enemy,
                MeshComponent::animated(
                    BODY.to_string(),
                    MeshAnimation::new(states, "idle".into()),
                ),
            )
            .unwrap();
        registry
            .set_component(
                enemy,
                DescriptorProvenance {
                    canonical_name: CLASS.to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        enemy
    };

    let idle = RecordedCommand {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        fire_pressed: false,
        fire_active: false,
    }
    .to_sim_command();
    let factions = FactionRegistry::default();
    let sentiment = RefCell::new(FactionSentimentState::default());
    let no_edges = HashMap::new();
    let mut fired = Vec::new();
    for tick in 0..ticks {
        let frame = tick / ticks_per_frame;
        let anim_time = (frame * ticks_per_frame) as f64 * f64::from(DT) * time_scale;
        let before: Vec<EntityId> = registry
            .borrow()
            .iter_with_kind(ComponentKind::Projectile)
            .map(|(id, _)| id)
            .collect();
        let _ = crate::sim::simulate_tick_with_presentation_aim(
            registry.clone(),
            &world,
            &sim_clips,
            Some(&nav_graph),
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            false,
            anim_time,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime, Some(&ai_clips)),
            &[],
            &mut mover_states,
            &[],
            &idle,
            |_| super::super::PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::Z,
            },
            DT,
            &mut crate::sim::touch::TouchSystem::default(),
            &mut Default::default(),
            &descriptors,
            0,
            &factions,
            &sentiment,
            None,
            &no_edges,
            &no_edges,
            None,
            |_, _, _| {},
            |_| {},
            crate::sim::TimingGate::OFF,
        );
        let registry = registry.borrow();
        // Host projectiles advance before the AI: a shot launched this tick
        // still stands at its launch point.
        for (projectile, _) in registry.iter_with_kind(ComponentKind::Projectile) {
            if before.contains(&projectile) {
                continue;
            }
            let origin = registry
                .get_component::<Transform>(projectile)
                .unwrap()
                .position;
            let clock = registry
                .get_component::<BrainComponent>(enemy)
                .unwrap()
                .clip_clock
                .clone();
            let mesh = registry.get_component::<MeshComponent>(enemy).unwrap();
            let transform = registry.get_component::<Transform>(enemy).unwrap();
            let muzzle = posed_muzzle_world(
                &sim_clips,
                model_matrix(transform, mesh.origin_offset, mesh.body_frame).unwrap(),
                mesh,
                ClipPose {
                    state: clock
                        .state
                        .as_deref()
                        .expect("the clock tracks the rifle loop"),
                    time: clock.time,
                },
                MuzzleMount::Mounted {
                    holder_socket: "hand_r",
                    bake: BAKE,
                },
                &mut Vec::new(),
            )
            .expect("the gunner's muzzle poses");
            fired.push((tick, origin, muzzle));
        }
    }
    fired
}

// S4: one fire tick, reached under different frame splits and animation time
// scales, launches from the same point, the bake's `muzzle` posed at the
// brain's clip time for the tracked state.
#[test]
fn npc_shots_launch_from_the_same_posed_muzzle_at_any_frame_split_and_animation_time_scale() {
    const TICKS: usize = 150;
    let baseline = launches(1, 1.0, TICKS);
    assert!(
        baseline.len() >= 6,
        "fixture: at least two bursts fire: {baseline:?}"
    );
    for (tick, origin, muzzle) in &baseline {
        assert!(
            origin.distance(*muzzle) < 1.0e-4,
            "tick {tick}: the shot leaves the bake's muzzle posed at the brain's clip time, \
             {muzzle}: {origin}"
        );
    }
    assert!(
        baseline[0].1.distance(baseline[1].1) > 1.0e-3,
        "fixture: the launch point moves with the clip: {baseline:?}"
    );
    for ticks_per_frame in [1, 2, 4] {
        for time_scale in [1.0, 0.25] {
            let run = launches(ticks_per_frame, time_scale, TICKS);
            assert_eq!(
                run.len(),
                baseline.len(),
                "{ticks_per_frame} ticks per frame at time scale {time_scale}: {run:?}"
            );
            for ((tick, origin, _), (base_tick, base_origin, _)) in run.iter().zip(&baseline) {
                assert_eq!(
                    tick, base_tick,
                    "{ticks_per_frame} ticks per frame at time scale {time_scale}"
                );
                assert!(
                    origin.distance(*base_origin) < 1.0e-5,
                    "{ticks_per_frame} ticks per frame at time scale {time_scale}, tick {tick}: \
                     {origin} vs {base_origin}"
                );
            }
        }
    }
}
