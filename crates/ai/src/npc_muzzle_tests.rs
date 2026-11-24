// E10 npc-weapon-mounts, end to end through the AI tick: an NPC's projectile
// shot leaves its fired weapon's posed muzzle, cover close to the gun holds it,
// the behavior sees the held shots, and it fires only while aiming. A gunner on
// the real limitator body and its AR_4 bake (and the arm-cannon mutant) with
// hand-written graphs at 60 Hz; the shipped rifle enemies on their committed
// descriptors.
// See: context/lib/entity_model.md §2 (Weapon mounts) · §7c (Clip-timed attacks, Sight sees movers)

use std::path::PathBuf;

use std::sync::Arc;

use glam::{Mat4, Quat};
use postretro_entities::components::brain::Interruption;
use postretro_entities::data_descriptors::{MeshDescriptor, MountDescriptor};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_foundation::BRAIN_ATTACKS_HELD_IN_ACTIVITY_INPUT;
use postretro_model::skeleton::Skeleton;

use super::*;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::collision::test_movers::{DoorPoses, door_slab};
use crate::facing::{ATTACK_FACING_TOLERANCE_RADIANS, FACING_TURN_RATE};
use crate::scripting::builtins::npc_mounts::npc_muzzle_mount;
use crate::scripting_systems::hit_zones::{ModelHitZones, ModelLoadWarningOwner, model_matrix};
use crate::scripting_systems::mesh_anim::MeshClipTables;
use crate::scripting_systems::muzzle_pose::{ClipPose, MuzzleMount, posed_muzzle_world};

const BODY: &str = "models/limitator/model.gltf";
const BAKE: &str = "models/ar_4/limitator.gltf";
const CANNON_BODY: &str = "models/mutant_arm_cannon/model.gltf";
const CANNON_TIP: &str = "cannon_tip";
const RIFLE: &str = "npc.rifle";
const VOLLEY: &str = "volley";

/// Gunner 0 stands here (`Scene::spawn`'s first slot), facing the player along +X.
const GUNNER_AT: Vec3 = Vec3::new(0.0, 0.0, -0.3);
/// The limitator's eye, its hitbox top: 1.82 m.
const EYE: Vec3 = Vec3::new(0.0, 1.82, -0.3);
/// The limitator's head top (research §NPC shot origin).
const LIMITATOR_HEAD_TOP: f32 = 1.74;
/// The player, raised so its aim point (eye 0.5 m above it) sits level with
/// the gun: the gun's line runs about level, the eye's slopes down to it.
const PLAYER: Vec3 = Vec3::new(6.0, 1.0, -0.3);
const AIM: Vec3 = Vec3::new(6.0, 1.5, -0.3);
/// Cover in the plane x = 1, 0.7 m ahead of the gun (inside the 2 m near
/// distance). The gun's line crosses it at about 1.47 m, the eye's at 1.77 m.
const COVER_X: f32 = 1.0;
/// Above the gun's line, below the eye's line and the hitbox top: holds the
/// muzzle shot, not the eye's view.
const LOW_LIP: f32 = 1.55;
/// Below both lines by more than the radius.
const UNDER_LIP: f32 = 1.2;
/// Above the eye's line: full cover.
const FULL_LIP: f32 = 1.95;
/// A column past the near distance: 2.7 m from the muzzle, taller than both.
const COLUMN_X: f32 = 3.0;
const DOOR: u32 = 7;

fn content_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

fn load(store: &mut HitZoneStore, model: &str) {
    store.insert_from_load(model, &content_root(), ModelLoadWarningOwner::GameSide);
    assert!(store.get_by_name(model).is_some(), "{model} loads");
}

/// The gunner's states, each on a clip of the limitator's library. `strike`
/// is the rifle loop as a one-shot, so a moment at its `kick` (0 s) is reached
/// on its entry tick.
const GUNNER_STATES: &[(&str, &str, bool)] = &[
    ("idle", "idle", true),
    ("aim", "idle_aiming", true),
    ("shoot", "firing_rifle", true),
    ("strike", "firing_rifle", false),
    ("walk", "walk_forward", true),
    ("walk_left", "walk_left", true),
    ("walk_right", "walk_right", true),
    ("hit_reaction", "hit_reaction", false),
    ("jump_loop", "jump_loop", true),
];

/// The arm-cannon mutant's states, on its `mixamo_brute` clips.
const CANNON_STATES: &[(&str, &str, bool)] = &[
    ("idle", "idle", true),
    ("aim", "idle", true),
    ("punch", "punch", false),
];

/// `body`'s clip index of `clip`.
fn clip_index(store: &HitZoneStore, body: &str, clip: &str) -> usize {
    store
        .get_by_name(body)
        .unwrap()
        .clips
        .iter()
        .position(|loaded| loaded.name == clip)
        .unwrap_or_else(|| panic!("{body} carries `{clip}`"))
}

fn animation_state(clip: &str, looping: bool, clip_index: Option<usize>) -> AnimationState {
    AnimationState {
        clip: clip.to_string(),
        looping,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        max_playback_rate: None,
        scale_with_speed: true,
        playback_rate: None,
        clip_index,
    }
}

fn rig_mesh(store: &HitZoneStore, body: &str, states: &[(&str, &str, bool)]) -> MeshComponent {
    let states = states
        .iter()
        .map(|&(state, clip, looping)| {
            (
                state.to_string(),
                animation_state(clip, looping, Some(clip_index(store, body, clip))),
            )
        })
        .collect();
    MeshComponent::animated(body.to_string(), MeshAnimation::new(states, "idle".into()))
}

fn rig_tables(store: &HitZoneStore, body: &str) -> MeshClipTables {
    let metadata: Vec<_> = store
        .get_by_name(body)
        .unwrap()
        .clips
        .iter()
        .map(|clip| postretro_model::ClipMetadata {
            name: clip.name.clone(),
            duration: clip.duration,
        })
        .collect();
    let mut tables = MeshClipTables::new();
    tables.insert_with_bounds(
        postretro_model::ModelHandle::from(body),
        &metadata,
        Default::default(),
    );
    tables
}

/// The rifle's entry on the gunner's table: the bake on `hand_r`.
fn rifle_mount() -> MountDescriptor {
    MountDescriptor {
        item: RIFLE.to_string(),
        attachments: Some(std::collections::HashMap::from([(
            "hand_r".to_string(),
            BAKE.to_string(),
        )])),
        muzzle_socket: None,
    }
}

/// The fixture archetype ([`CLASS`]) on `body`, carrying `mounts`.
fn archetype(body: &str, mounts: Option<Vec<MountDescriptor>>) -> EntityTypeDescriptor {
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
            model: body.to_string(),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: Default::default(),
            default_state: None,
            locomotion: None,
            mounts,
        }),
        health: None,
        behavior: None,
    }
}

/// A projectile rifle firing `shots` shots `wait_ms` apart, recovering
/// `recovery_ms`, from a magazine of `magazine`, at the enemy rifles' 0.15 m
/// radius.
fn rifle(shots: usize, wait_ms: f32, recovery_ms: f32, magazine: u32) -> EntityTypeDescriptor {
    let mut descriptor = projectile_weapon_descriptor(RIFLE, 12.0, 6.0, recovery_ms);
    let weapon = descriptor.weapon.as_mut().unwrap();
    let mut steps = Vec::new();
    for shot in 0..shots {
        if shot > 0 {
            steps.push(serde_json::json!({ "kind": "wait", "durationMs": wait_ms }));
        }
        steps.push(serde_json::json!({ "kind": "shot" }));
    }
    weapon.primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": recovery_ms, "steps": steps,
    }))
    .expect("a valid burst action");
    weapon.resource = Some(
        serde_json::from_value(serde_json::json!({
            "kind": "ammo", "type": "bullets", "magazine": magazine, "reserve": 0
        }))
        .unwrap(),
    );
    weapon.projectile.as_mut().unwrap().radius = 0.15;
    descriptor
}

/// The dev rifles' three-shot burst, 120 ms apart.
fn burst_rifle() -> EntityTypeDescriptor {
    rifle(3, 120.0, 1000.0, 30)
}

/// One shot, no recovery: an attack that proposes on every eligible dwell.
fn single_rifle() -> EntityTypeDescriptor {
    rifle(1, 0.0, 0.0, 30)
}

fn rifle_attack() -> AttackParams {
    AttackParams {
        sound: None,
        weapon: Some(RIFLE.to_string()),
        damage: None,
        max_range: None,
        cooldown_ms: None,
        engagement_radius: None,
        standoff_distance: None,
        queue_range: None,
        damage_type: None,
        draws: None,
    }
}

/// A graph over `activities` and `rows`, its one attack [`VOLLEY`] on the rifle.
fn volley_graph(
    initial: &str,
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
    rows: Vec<(&str, Vec<GuardedRow>)>,
) -> BehaviorGraphDescriptor {
    clip_graph(initial, activities, rows, vec![(VOLLEY, rifle_attack())])
}

/// An activity on `state` firing the volley with no clip moment.
fn firing(state: &str) -> BehaviorActivityDescriptor {
    act(state, Some(untimed(VOLLEY)))
}

/// An activity on `state` firing the volley at its clip's `kick`.
fn firing_at_kick(state: &str) -> BehaviorActivityDescriptor {
    act(state, Some(timed(VOLLEY, "kick")))
}

fn resting() -> BehaviorActivityDescriptor {
    act("aim", None)
}

fn number(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

fn at_least(input: &str, value: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(input)),
        b: Box::new(number(value)),
    }
}

fn target_not_visible() -> IrNode {
    IrNode::Eq {
        a: Box::new(brain_input(BRAIN_TARGET_VISIBLE_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Bool(false),
        }),
    }
}

/// The limitator's hitbox: its eye, the hitbox top, at 1.82 m.
fn limitator_hitbox() -> Hitbox {
    Hitbox {
        half_extents: Vec3::new(0.27, 0.91, 0.27),
        offset: Vec3::new(0.0, 0.91, 0.0),
    }
}

/// A scene on `body` (its `states`), the models in `models` loaded, the
/// archetype carrying `mounts`, armed with `weapon`, the player at `player_at`.
fn rig_scene(
    body: &str,
    states: &[(&str, &str, bool)],
    models: &[&str],
    mounts: Option<Vec<MountDescriptor>>,
    weapon: EntityTypeDescriptor,
    player_at: Vec3,
) -> Scene {
    let mut clips = HitZoneStore::new();
    for model in models {
        load(&mut clips, model);
    }
    let mesh = rig_mesh(&clips, body, states);
    let clip_tables = rig_tables(&clips, body);
    let mut reg = EntityRegistry::new();
    let pawn = spawn_player(&mut reg, player_at);
    Scene {
        reg,
        runtime: AiRuntime::deciding_every_tick(),
        mesh,
        clips,
        clip_tables,
        pawn,
        descriptors: vec![weapon, archetype(body, mounts)],
    }
}

/// Gunners on the limitator body with its AR_4 bake on `hand_r`.
fn gunner_scene(weapon: EntityTypeDescriptor) -> Scene {
    rig_scene(
        BODY,
        GUNNER_STATES,
        &[BODY, BAKE],
        Some(vec![rifle_mount()]),
        weapon,
        PLAYER,
    )
}

/// Spawn a gunner as `index` in `state`, its eye at the limitator's 1.82 m.
fn spawn_gunner(
    scene: &mut Scene,
    index: usize,
    graph: &BehaviorGraphDescriptor,
    state: &str,
) -> EntityId {
    let enemy = scene.spawn(index, graph, state);
    set_enemy_hitbox(&mut scene.reg, enemy, limitator_hitbox());
    enemy
}

/// Replace the loaded bake with one carrying no sockets: the same NPC, its
/// bake's `muzzle` removed.
fn strip_bake_muzzle(scene: &mut Scene) {
    scene.clips.insert_for_test(
        postretro_model::ModelHandle::from(BAKE),
        ModelHitZones::for_test(
            Arc::new(Skeleton { joints: Vec::new() }),
            Arc::new(Vec::new()),
            Vec::new(),
            std::collections::HashMap::new(),
            None,
            Vec::new(),
            Default::default(),
        ),
    );
}

fn seed_cooldown(scene: &mut Scene, enemy: EntityId, ms: f32) {
    let mut brain = scene.brain(enemy);
    brain
        .attack_cooldown_remaining_ms
        .insert(VOLLEY.to_string(), ms);
    scene.reg.set_component(enemy, brain).unwrap();
}

/// A wall in the cover plane topping out at `lip`.
fn cover(lip: f32) -> CollisionWorld {
    wall_world(COVER_X, -1.0, lip)
}

fn column() -> CollisionWorld {
    wall_world(COLUMN_X, -1.0, 4.0)
}

/// One AI tick of `dt` against `world`, with `movers` posed as `poses`.
fn tick_full(
    scene: &mut Scene,
    world: &CollisionWorld,
    movers: &[MoverCollider],
    poses: &DoorPoses,
    dt: f32,
    on_impact: impl FnMut(&mut EntityRegistry),
) -> AiTickResult {
    run_ai_tick_with_navigation_and_impact(
        &mut scene.reg,
        &mut scene.runtime,
        dt,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav {
                gate: crate::nav::NavGate::OPEN,
                movers,
                poses,
            },
            nav_graph: None,
            collision_world: Some(world),
            descriptors: &scene.descriptors,
            descriptor_generation: 0,
            factions: &FactionRegistry::default(),
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
            clips: Some(&scene.clips),
            schedule: Default::default(),
            cpu: None,
        },
        on_impact,
    )
}

/// One 60 Hz tick against `world`.
fn tick_in(scene: &mut Scene, world: &CollisionWorld) -> AiTickResult {
    tick_full(scene, world, &[], &DoorPoses::default(), DT, |_| {})
}

/// One shot as the tick that launched it left it.
#[derive(Debug, Clone, Copy)]
struct Shot {
    tick: usize,
    origin: Vec3,
    direction: Vec3,
    /// The bake's muzzle posed on the brain's tracked state at its clip time
    /// after the tick, at the facing and position the enemy holds then.
    muzzle: Option<Vec3>,
}

fn record(scene: &Scene, enemy: EntityId, tick: usize, result: &AiTickResult) -> Vec<Shot> {
    result
        .projectile_spawns
        .iter()
        .map(|spawn| Shot {
            tick,
            origin: scene
                .reg
                .get_component::<Transform>(spawn.projectile)
                .unwrap()
                .position,
            direction: Vec3::from_array(
                scene
                    .reg
                    .get_component::<ProjectileComponent>(spawn.projectile)
                    .unwrap()
                    .direction,
            ),
            muzzle: tracked_muzzle(scene, enemy, gunner_mount()),
        })
        .collect()
}

/// Run `ticks` ticks of `enemy`, calling `before` ahead of each with its
/// index; `before` names the world that tick runs against.
fn run<'w>(
    scene: &mut Scene,
    enemy: EntityId,
    ticks: usize,
    mut before: impl FnMut(&mut Scene, usize) -> &'w CollisionWorld,
) -> Vec<Shot> {
    let mut shots = Vec::new();
    for tick in 0..ticks {
        let world = before(scene, tick);
        let result = tick_in(scene, world);
        shots.extend(record(scene, enemy, tick, &result));
    }
    shots
}

fn gunner_mount() -> MuzzleMount<'static> {
    MuzzleMount::Mounted {
        holder_socket: "hand_r",
        bake: BAKE,
    }
}

/// `mount` on `enemy` posed on `state` at clip `time`, at `transform`.
fn posed_in(
    reg: &EntityRegistry,
    clips: &HitZoneStore,
    enemy: EntityId,
    transform: &Transform,
    state: &str,
    time: f64,
    mount: MuzzleMount<'_>,
) -> Option<Vec3> {
    let mesh = reg.get_component::<MeshComponent>(enemy).ok()?;
    let model_to_world = model_matrix(transform, mesh.origin_offset, mesh.body_frame)?;
    posed_muzzle_world(
        clips,
        model_to_world,
        mesh,
        ClipPose { state, time },
        mount,
        &mut Vec::<Mat4>::new(),
    )
}

/// `mount` posed on the state `enemy`'s clip clock tracks, at its clip time,
/// at the facing and position it holds now.
fn tracked_in(
    reg: &EntityRegistry,
    clips: &HitZoneStore,
    enemy: EntityId,
    mount: MuzzleMount<'_>,
) -> Option<Vec3> {
    let clock = reg
        .get_component::<BrainComponent>(enemy)
        .ok()?
        .clip_clock
        .clone();
    let transform = *reg.get_component::<Transform>(enemy).ok()?;
    posed_in(
        reg,
        clips,
        enemy,
        &transform,
        clock.state.as_deref()?,
        clock.time,
        mount,
    )
}

fn posed_at(
    scene: &Scene,
    enemy: EntityId,
    transform: &Transform,
    state: &str,
    time: f64,
    mount: MuzzleMount<'_>,
) -> Option<Vec3> {
    posed_in(
        &scene.reg,
        &scene.clips,
        enemy,
        transform,
        state,
        time,
        mount,
    )
}

/// `mount` posed on `state` at clip `time`, at the facing and position
/// `enemy` holds now.
fn posed(
    scene: &Scene,
    enemy: EntityId,
    state: &str,
    time: f64,
    mount: MuzzleMount<'_>,
) -> Option<Vec3> {
    let transform = *scene.reg.get_component::<Transform>(enemy).ok()?;
    posed_at(scene, enemy, &transform, state, time, mount)
}

/// `mount` posed on the state `enemy`'s clip clock tracks, at its clip time.
fn tracked_muzzle(scene: &Scene, enemy: EntityId, mount: MuzzleMount<'_>) -> Option<Vec3> {
    tracked_in(&scene.reg, &scene.clips, enemy, mount)
}

fn assert_from_muzzle(shot: &Shot, label: &str) {
    let muzzle = shot
        .muzzle
        .unwrap_or_else(|| panic!("{label}: the tracked state poses a muzzle: {shot:?}"));
    assert!(
        shot.origin.distance(muzzle) < 1.0e-4,
        "{label}: the shot leaves the muzzle posed at the brain's clip time, {muzzle}: {shot:?}"
    );
}

fn assert_toward(shot: &Shot, aim: Vec3, label: &str) {
    let expected = (aim - shot.origin).normalize();
    assert!(
        shot.direction.distance(expected) < 1.0e-4,
        "{label}: the shot flies from its origin at the target's aim point: {shot:?}"
    );
}

fn assert_from_eye(shot: &Shot, label: &str) {
    assert!(
        shot.origin.distance(EYE) < 1.0e-4,
        "{label}: the shot leaves the eye: {shot:?}"
    );
}

fn ticks_of(shots: &[Shot]) -> Vec<usize> {
    shots.iter().map(|shot| shot.tick).collect()
}

fn held(scene: &Scene, enemy: EntityId, depth: usize) -> u32 {
    scene.brain(enemy).activity_held_count(depth).unwrap()
}

fn fired(scene: &Scene, enemy: EntityId, depth: usize) -> u32 {
    scene.brain(enemy).activity_attack_count(depth).unwrap()
}

fn magazine(scene: &Scene, enemy: EntityId) -> u32 {
    scene.runtime.weapons.weapon(enemy, RIFLE).unwrap().magazine
}

fn interruption(scene: &Scene, enemy: EntityId) -> Interruption {
    scene.brain(enemy).clip_clock.interruption
}

/// The first tick whose path ends in `leaf`, given each tick's path.
fn first(paths: &[String], leaf: &str) -> usize {
    paths
        .iter()
        .position(|path| path.ends_with(leaf))
        .unwrap_or_else(|| panic!("enters `{leaf}`: {paths:?}"))
}

/// Shot ticks of an unobstructed three-shot burst from the entry tick.
fn burst_ticks() -> Vec<usize> {
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let open = CollisionWorld::new();
    let shots = run(&mut scene, enemy, 30, |_, _| &open);
    let ticks = ticks_of(&shots);
    assert_eq!(ticks.len(), 3, "fixture: a three-shot burst: {ticks:?}");
    assert_eq!(ticks[0], 0, "fixture: from the entry tick: {ticks:?}");
    ticks
}

// ---------------------------------------------------------------------------
// Shot origin
// ---------------------------------------------------------------------------

// S1, S4, N6 (clear half), A13 (no reaction): every shot of a clear burst
// leaves the bake's `muzzle` posed on the tracked state at the brain's clip
// time, below the head, and flies at the target's aim point; the origin moves
// with the clip pose from shot to shot.
#[test]
fn a_gunners_clear_burst_leaves_the_posed_bake_muzzle_below_its_head_on_every_shot() {
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let open = CollisionWorld::new();
    let shots = run(&mut scene, enemy, 30, |_, _| &open);
    assert_eq!(shots.len(), 3, "the whole burst fires: {shots:?}");
    for (index, shot) in shots.iter().enumerate() {
        let label = format!("shot {index}");
        assert_from_muzzle(shot, &label);
        assert_toward(shot, AIM, &label);
        assert!(
            shot.origin.y < LIMITATOR_HEAD_TOP - 0.1,
            "{label}: below the head top, well below the eye: {shot:?}"
        );
        assert!(
            shot.origin.distance(EYE) > 0.3,
            "{label}: not the eye: {shot:?}"
        );
    }
    assert!(
        shots[0].origin.distance(shots[1].origin) > 1.0e-3,
        "the origin moves with the clip pose: {shots:?}"
    );
    assert_eq!(magazine(&scene, enemy), 27, "three shots spent");
    assert_eq!(fired(&scene, enemy, 0), 3);
    assert_eq!(held(&scene, enemy, 0), 0);
}

// S1 (sight reads the eye), B4 (eye-gate failure): a soffit hides the target
// from the eye while the gun's whole path runs clear beneath it. The eye's
// verdict decides: `targetVisible` reads false, nothing fires from the eye or
// the muzzle, and no shot counts as held.
#[test]
fn a_soffit_over_a_clear_gun_hides_the_target_and_holds_fire_uncounted() {
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    // Hangs from 1.65 m: under the eye's line (1.77 m at x = 1), over the
    // gun's swept path (1.47 m + 0.15 m).
    let soffit = wall_world(COVER_X, 1.65, 3.0);
    let open = CollisionWorld::new();
    for hidden in [true, false] {
        let mut scene = gunner_scene(burst_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        // Sighted on the first tick, then hidden before the volley is ready.
        seed_cooldown(&mut scene, enemy, 150.0);
        let shots = run(&mut scene, enemy, 30, |_, tick| {
            if hidden && tick > 0 { &soffit } else { &open }
        });
        if hidden {
            assert!(
                shots.is_empty(),
                "the eye cannot see, so nothing fires: {shots:?}"
            );
            assert!(
                scene.brain(enemy).time_since_target_visible > 0.0,
                "`targetVisible` reads the eye: false"
            );
            assert_eq!(
                held(&scene, enemy, 0),
                0,
                "an eye-gate failure holds nothing"
            );
            assert_eq!(magazine(&scene, enemy), 30);
        } else {
            assert_eq!(shots.len(), 3, "fixture: in the open the volley fires");
        }
    }
}

// S3 (launch half): an entry naming `muzzleSocket` launches from that posed
// body socket, the arm cannon's `cannon_tip`, and the point moves with the
// clip.
#[test]
fn a_built_in_cannons_shots_leave_its_posed_body_socket_and_move_with_the_clip() {
    let mount = MountDescriptor {
        item: RIFLE.to_string(),
        attachments: None,
        muzzle_socket: Some(CANNON_TIP.to_string()),
    };
    let mut scene = rig_scene(
        CANNON_BODY,
        CANNON_STATES,
        &[CANNON_BODY],
        Some(vec![mount]),
        burst_rifle(),
        PLAYER,
    );
    let graph = volley_graph("fire", vec![("fire", firing("punch"))], vec![]);
    let enemy = scene.spawn(0, &graph, "fire");
    let open = CollisionWorld::new();
    let mut shots = Vec::new();
    for tick in 0..30 {
        let result = tick_in(&mut scene, &open);
        for spawn in &result.projectile_spawns {
            let origin = scene
                .reg
                .get_component::<Transform>(spawn.projectile)
                .unwrap()
                .position;
            let tip = tracked_muzzle(&scene, enemy, MuzzleMount::BodySocket(CANNON_TIP))
                .expect("the cannon tip poses");
            shots.push((tick, origin, tip));
        }
    }
    assert_eq!(shots.len(), 3, "the whole burst fires: {shots:?}");
    for (tick, origin, tip) in &shots {
        assert!(
            origin.distance(*tip) < 1.0e-4,
            "tick {tick}: the shot leaves `cannon_tip` posed at the brain's clip time, {tip}: {origin}"
        );
    }
    assert!(
        shots[0].1.distance(shots[2].1) > 0.05,
        "the punch swings the cannon between shots: {shots:?}"
    );
}

// S5 (NM-1): on the tick the gunner enters `fire`, turning to its target and
// drawing a new clip, the shot leaves the muzzle posed on that tick's clip at
// its start and that tick's facing, not the pre-turn facing or the old clip.
#[test]
fn a_gunner_turning_into_a_new_clip_on_its_fire_tick_launches_from_that_ticks_pose() {
    let graph = volley_graph(
        "rest",
        vec![("rest", resting()), ("fire", firing("strike"))],
        vec![("rest", vec![edge("fire", after_ms(100.0))])],
    );
    let open = CollisionWorld::new();
    let entry = {
        let mut scene = gunner_scene(burst_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
        let mut paths = Vec::new();
        run(&mut scene, enemy, 12, |scene, _| {
            paths.push(scene.state(enemy));
            &open
        });
        paths.push(scene.state(enemy));
        // Paths are read before each tick: the tick that entered it is one less.
        first(&paths, "fire") - 1
    };
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
    // Off the target by more than the fire tolerance, back within it after
    // one tick's turn.
    let off = ATTACK_FACING_TOLERANCE_RADIANS + 0.5 * FACING_TURN_RATE * DT;
    let mut before = None;
    let shots = run(&mut scene, enemy, entry + 1, |scene, tick| {
        if tick == entry {
            set_enemy_yaw(&mut scene.reg, enemy, std::f32::consts::FRAC_PI_2 + off);
            let clock = scene.brain(enemy).clip_clock.clone();
            before = Some((
                *scene.reg.get_component::<Transform>(enemy).unwrap(),
                clock.state.unwrap().to_string(),
                clock.time,
            ));
        }
        &open
    });
    let [shot] = shots.as_slice() else {
        panic!("one shot, on the entry tick: {shots:?}");
    };
    assert_eq!(shot.tick, entry, "fires on its entry tick");
    assert_eq!(
        scene.brain(enemy).clip_clock.state.as_deref(),
        Some("strike")
    );
    assert_from_muzzle(shot, "the entry tick");
    let new_clip_at_start = posed(&scene, enemy, "strike", 0.0, gunner_mount()).unwrap();
    assert!(
        shot.origin.distance(new_clip_at_start) < 1.0e-4,
        "posed at the clip's start"
    );
    let (turned_from, old_state, old_time) = before.unwrap();
    let turned = *scene.reg.get_component::<Transform>(enemy).unwrap();
    assert!(
        turned.rotation.angle_between(turned_from.rotation) > 0.01,
        "fixture: the gunner turned on the fire tick"
    );
    let unturned = posed_at(&scene, enemy, &turned_from, "strike", 0.0, gunner_mount()).unwrap();
    assert!(
        shot.origin.distance(unturned) > 0.01,
        "at the facing written this tick, not the tick-start facing: {shot:?}, {unturned}"
    );
    let old_clip = posed(&scene, enemy, &old_state, old_time, gunner_mount()).unwrap();
    assert!(
        shot.origin.distance(old_clip) > 0.01,
        "on the new clip, not the one drawn before: {shot:?}, {old_clip}"
    );
}

/// Each shipped rifle enemy's head top (research §NPC shot origin).
const HEAD_TOPS: [(&str, f32); 3] = [
    ("limitator", 1.74),
    ("rifle_trooper", 1.88),
    ("rifle_warrior", 1.80),
];

/// Give a shipped spawn what level load gives it for its shots: its
/// descriptor's identity, its hitbox (so its eye), and its mounted bakes
/// loaded. Returns its eye.
fn arm_shipped(shipped: &mut Shipped, enemy: EntityId, name: &str) -> Vec3 {
    let shipped_descriptor = descriptor(&shipped.manifest, name).clone();
    shipped
        .reg
        .set_component(
            enemy,
            DescriptorProvenance {
                canonical_name: name.to_string(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::MapPlacement,
            },
        )
        .unwrap();
    let authored = shipped_descriptor
        .health
        .as_ref()
        .and_then(|health| health.hitbox.as_ref())
        .unwrap_or_else(|| panic!("`{name}` authors a hitbox"));
    let hitbox = Hitbox {
        half_extents: Vec3::from_array(authored.half_extents),
        offset: Vec3::from_array(authored.offset.unwrap_or_default()),
    };
    set_enemy_hitbox(&mut shipped.reg, enemy, hitbox);
    let mesh = shipped_descriptor.mesh.as_ref().unwrap();
    for entry in mesh.mounts.as_ref().expect("a mount table") {
        for bake in entry
            .attachments
            .iter()
            .flat_map(|attachments| attachments.values())
        {
            if shipped.clips.get_by_name(bake).is_none() {
                load(&mut shipped.clips, bake);
            }
        }
    }
    let position = shipped
        .reg
        .get_component::<Transform>(enemy)
        .unwrap()
        .position;
    position + hitbox.offset + Vec3::Y * hitbox.half_extents.y
}

/// `name`'s first `count` shots of `weapon` with the player at `player_at`:
/// each origin, with its weapon's mount posed on the tracked state after the
/// shot's tick. Also its eye.
fn shipped_shots(
    manifest: &ModManifestResult,
    name: &str,
    player_at: Vec3,
    weapon: &str,
    count: usize,
) -> (Vec<(Vec3, Vec3)>, Vec3) {
    let (mut shipped, enemy) = rifle_scene_at(manifest.clone(), name, 0, player_at);
    let eye = arm_shipped(&mut shipped, enemy, name);
    let shipped_descriptor = descriptor(&shipped.manifest, name).clone();
    let mut shots = Vec::new();
    for _ in 0..(12.0 / DT) as usize {
        let result = shipped.tick();
        for spawn in &result.projectile_spawns {
            if spawn.descriptor_class != weapon {
                continue;
            }
            let mount = npc_muzzle_mount(&shipped_descriptor, weapon, &shipped.clips)
                .unwrap_or_else(|| panic!("`{name}` carries `{weapon}` with a muzzle"));
            let origin = shipped
                .reg
                .get_component::<Transform>(spawn.projectile)
                .unwrap()
                .position;
            let muzzle = tracked_in(&shipped.reg, &shipped.clips, enemy, mount)
                .unwrap_or_else(|| panic!("`{name}`'s tracked state poses its muzzle"));
            shots.push((origin, muzzle));
        }
        if shots.len() >= count {
            return (shots, eye);
        }
    }
    panic!("`{name}` fires {count} `{weapon}` shots within 12 s: {shots:?}");
}

// S1 (content), S2: the limitator and each rifle squad carry both their rifles
// on one rig bake on `hand_r`, and both rifles' shots, the burst's and the long
// shot's, leave that bake's `muzzle` posed at the brain's clip time: at the
// barrel tip, below the head top and well below the eye.
#[test]
fn shipped_rifle_enemies_fire_both_rifles_from_their_one_bakes_muzzle_below_the_head() {
    let manifest = dev_manifest();
    for (name, head_top) in HEAD_TOPS {
        let shipped_descriptor = descriptor(&manifest, name);
        let mut store = HitZoneStore::new();
        let mesh = shipped_descriptor.mesh.as_ref().unwrap();
        load(&mut store, &mesh.model);
        for entry in mesh.mounts.as_ref().unwrap() {
            for bake in entry
                .attachments
                .iter()
                .flat_map(|attachments| attachments.values())
            {
                if store.get_by_name(bake).is_none() {
                    load(&mut store, bake);
                }
            }
        }
        let burst_mount = npc_muzzle_mount(shipped_descriptor, "enemy_rifle_burst", &store);
        let long_mount = npc_muzzle_mount(shipped_descriptor, "enemy_rifle_long", &store);
        assert!(
            matches!(
                burst_mount,
                Some(MuzzleMount::Mounted {
                    holder_socket: "hand_r",
                    ..
                })
            ),
            "{name}: the burst rifle's muzzle is a bake on `hand_r`: {burst_mount:?}"
        );
        assert_eq!(
            burst_mount, long_mount,
            "{name}: both rifles share the one bake's muzzle"
        );

        for (weapon, player_at, count) in [
            ("enemy_rifle_burst", Vec3::new(6.0, 0.0, 0.0), 3),
            ("enemy_rifle_long", Vec3::new(15.0, 0.0, 0.0), 1),
        ] {
            let (shots, eye) = shipped_shots(&manifest, name, player_at, weapon, count);
            for (origin, muzzle) in &shots {
                assert!(
                    origin.distance(*muzzle) < 1.0e-4,
                    "{name} {weapon}: the shot leaves the bake's posed muzzle {muzzle}: {origin}"
                );
                assert!(
                    origin.y < head_top && origin.y < eye.y - 0.1,
                    "{name} {weapon}: below the head top {head_top} and the eye {eye}: {origin}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The near-path gate
// ---------------------------------------------------------------------------

/// What one run of the cover twin left.
#[derive(Debug)]
struct CoverRun {
    shots: Vec<Shot>,
    path: String,
    held: u32,
    fired: u32,
}

/// The meshed gunner's clip-timed shot (its moment on the entry tick) behind
/// `world`, with a row leaving `fire` once `targetVisible` reads false.
fn cover_run(world: &CollisionWorld, strip_muzzle: bool) -> CoverRun {
    let graph = volley_graph(
        "fire",
        vec![("fire", firing_at_kick("strike")), ("blind", resting())],
        vec![("fire", vec![edge("blind", target_not_visible())])],
    );
    let mut scene = gunner_scene(rifle(1, 0.0, 1000.0, 30));
    if strip_muzzle {
        strip_bake_muzzle(&mut scene);
    }
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let shots = run(&mut scene, enemy, 20, |_, _| world);
    CoverRun {
        shots,
        path: scene.state(enemy),
        held: held(&scene, enemy, 0),
        fired: fired(&scene, enemy, 0),
    }
}

// N8, X3, B1, B4: the meshed projectile twin of
// `enemy_eye_height_clears_low_cover_but_full_cover_blocks_fire`. A low wall
// near the gunner whose lip is above its gun and below its hitbox top, the
// eye's view clear: the clip moment whiffs, nothing launches from the eye or
// the muzzle, `targetVisible` reads true, fired stays 0 and held counts 1.
// A wall below both paths, or none, fires from the muzzle at the target's aim
// point. Full cover fails the eye gate as before, counting nothing held. The
// same gunner with its bake's `muzzle` removed fires from the eye over the low
// wall: a missing muzzle runs no near-path test and counts nothing held.
#[test]
fn a_meshed_gunner_behind_low_cover_whiffs_while_lower_walls_take_the_muzzle_shot() {
    for (label, world) in [
        ("no wall", CollisionWorld::new()),
        ("a wall below both paths", cover(UNDER_LIP)),
    ] {
        let run = cover_run(&world, false);
        let [shot] = run.shots.as_slice() else {
            panic!("{label}: one shot: {run:?}");
        };
        assert_eq!(shot.tick, 0, "{label}: at the entry tick's moment");
        assert_from_muzzle(shot, label);
        assert_toward(shot, AIM, label);
        assert_eq!((run.fired, run.held), (1, 0), "{label}: {run:?}");
    }

    let low = cover_run(&cover(LOW_LIP), false);
    assert!(
        low.shots.is_empty(),
        "low cover: no projectile from the eye or the muzzle: {low:?}"
    );
    assert_eq!(
        low.path, "fire",
        "low cover: `targetVisible` reads true: {low:?}"
    );
    assert_eq!(
        (low.fired, low.held),
        (0, 1),
        "low cover: one whiffed moment held: {low:?}"
    );

    let full = cover_run(&cover(FULL_LIP), false);
    assert!(full.shots.is_empty(), "full cover: {full:?}");
    assert_eq!(
        full.path, "blind",
        "full cover fails the eye gate: {full:?}"
    );
    assert_eq!(full.held, 0, "an eye-gate failure holds nothing: {full:?}");

    let stripped = cover_run(&cover(LOW_LIP), true);
    let [shot] = stripped.shots.as_slice() else {
        panic!("no muzzle: one shot from the eye: {stripped:?}");
    };
    assert_from_eye(shot, "no muzzle");
    assert_toward(shot, AIM, "no muzzle");
    assert_eq!(
        (stripped.fired, stripped.held),
        (1, 0),
        "a missing muzzle fires from the eye, holding nothing: {stripped:?}"
    );
}

fn closed_door() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(COVER_X, 0.0, GUNNER_AT.z), Quat::IDENTITY)
}

fn open_door() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(COVER_X, 20.0, GUNNER_AT.z), Quat::IDENTITY)
}

/// A door slab at the cover plane, as tall as the low lip.
fn low_door() -> [MoverCollider; 1] {
    [door_slab(DOOR, 0.05, LOW_LIP, 1.0)]
}

// N2 (mover half), N5, B3 (one count per hold, AI tick rate), NM-29: a closed
// door at the gun, crossing the muzzle's path but not the eye's, holds an
// attack with no clip moment as a wall would. Held across many ticks it counts
// once, at 60 Hz or 30 Hz; a reaction drawn over the hold and the brain's own
// stride drawing over the reaction add nothing. On the first tick the door
// stands open the shot leaves the muzzle.
#[test]
fn a_closed_door_at_the_gun_holds_the_shot_counting_once_until_it_opens() {
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let world = CollisionWorld::new();
    let movers = low_door();
    for (rate, dt) in [("60 Hz", DT), ("30 Hz", 2.0 * DT)] {
        let span = (0.5 / dt).round() as usize;
        let mut scene = gunner_scene(single_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        for tick in 0..span {
            let result = tick_full(&mut scene, &world, &movers, &closed_door(), dt, |_| {});
            assert!(
                result.projectile_spawns.is_empty(),
                "{rate} tick {tick}: the closed door holds the shot"
            );
            assert_eq!(
                held(&scene, enemy, 0),
                1,
                "{rate} tick {tick}: counted once"
            );
        }
        let result = tick_full(&mut scene, &world, &movers, &open_door(), dt, |_| {});
        let shots = record(&scene, enemy, span, &result);
        let [shot] = shots.as_slice() else {
            panic!("{rate}: the first open tick fires: {shots:?}");
        };
        assert_from_muzzle(shot, rate);
        assert_eq!(
            (fired(&scene, enemy, 0), held(&scene, enemy, 0)),
            (1, 1),
            "{rate}"
        );
    }

    // NM-29: a reaction over the hold, then the brain's own stride drawing
    // over the reaction, in the same activity, the door closed throughout.
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    for tick in 0..30 {
        match tick {
            5 => scene.switch(enemy, "hit_reaction"),
            15 => scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(1.0, 0.0, 0.0)),
            _ => {}
        }
        let result = tick_full(&mut scene, &world, &movers, &closed_door(), DT, |_| {});
        assert!(result.projectile_spawns.is_empty(), "tick {tick}: held");
        if tick == 10 {
            assert_eq!(
                interruption(&scene, enemy),
                Interruption::Foreign,
                "fixture"
            );
        }
        if tick == 15 {
            assert_eq!(
                interruption(&scene, enemy),
                Interruption::None,
                "fixture: the stride's own draw lands over the reaction"
            );
        }
    }
    assert_eq!(
        held(&scene, enemy, 0),
        1,
        "the not-aiming ticks and the own draw add nothing to the hold"
    );
}

// N4, B4 (far obstacle): a column the target ducks behind mid-burst, past the
// near distance. The burst's later shots launch from the muzzle and strike it,
// none skipped: one while the eye's loss grace still passes, one after the
// column has hidden the target past it. Nothing counts as held.
#[test]
fn a_column_past_the_near_distance_takes_the_bursts_later_shots() {
    let ticks = burst_ticks();
    let fly = |shots_world: &CollisionWorld, with_column: bool| {
        let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
        let mut scene = gunner_scene(burst_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        // A body the shots can strike.
        let pawn = scene.pawn;
        set_enemy_hitbox(
            &mut scene.reg,
            pawn,
            Hitbox {
                half_extents: Vec3::new(0.4, 0.8, 0.4),
                offset: Vec3::new(0.0, 0.8, 0.0),
            },
        );
        let open = CollisionWorld::new();
        let mut hidden_ms = Vec::new();
        let shots = run(&mut scene, enemy, 30, |scene, tick| {
            if tick > 0 {
                hidden_ms.push(scene.brain(enemy).time_since_target_visible);
            }
            if with_column && tick >= ticks[1] {
                shots_world
            } else {
                &open
            }
        });
        hidden_ms.push(scene.brain(enemy).time_since_target_visible);
        let state = (
            magazine(&scene, enemy),
            held(&scene, enemy, 0),
            fired(&scene, enemy, 0),
        );
        // Fly every shot through the column's world.
        let registry = Rc::new(RefCell::new(std::mem::replace(
            &mut scene.reg,
            EntityRegistry::new(),
        )));
        for _ in 0..120 {
            crate::sim::advance(
                &registry,
                &CombinedCollisionWorld::static_only(shots_world),
                &HitZoneStore::new(),
                0.0,
                DT,
                &mut |_| {},
            );
        }
        let registry = registry.borrow();
        let hp = player_hp(&registry, scene.pawn);
        let left = projectile_ids(&registry).len();
        (shots, hidden_ms, state, hp, left)
    };
    let col = column();
    let (shots, hidden_ms, (mag, held_count, fired_count), hp, left) = fly(&col, true);
    assert_eq!(ticks_of(&shots), ticks, "none skipped: {shots:?}");
    for (index, shot) in shots.iter().enumerate() {
        assert_from_muzzle(shot, &format!("shot {index}"));
    }
    // `hidden_ms[t]` is the clock after tick t.
    assert_eq!(
        hidden_ms[ticks[1]], 0.0,
        "fixture: the second shot fires inside the eye's loss grace"
    );
    assert!(
        hidden_ms[ticks[2]] > 0.0,
        "fixture: the third fires after the column hid the target past its grace: {hidden_ms:?}"
    );
    assert_eq!((mag, held_count, fired_count), (27, 0, 3));
    assert_eq!(left, 0, "every shot struck something");
    assert_eq!(hp, 100.0, "the column took them");
    let (_, _, _, hp, _) = fly(&CollisionWorld::new(), false);
    assert!(
        hp < 100.0,
        "fixture: unobstructed, the shots reach the player"
    );
}

// N6, B2 (one per skipped shot, visible next tick), N7 (first fires, the rest
// skipped, reads 1): a burst shot whose near path is blocked launches nothing
// and spends its shot; the next shot re-tests its paths and fires once clear.
#[test]
fn a_burst_skips_each_blocked_shot_spends_it_and_fires_the_clear_ones() {
    let ticks = burst_ticks();
    let low = cover(LOW_LIP);
    let open = CollisionWorld::new();
    for (label, blocked, fired_shots) in [
        (
            "the second blocked",
            vec![ticks[1]],
            vec![ticks[0], ticks[2]],
        ),
        (
            "both later blocked",
            vec![ticks[1], ticks[2]],
            vec![ticks[0]],
        ),
    ] {
        let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
        let mut scene = gunner_scene(burst_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let mut held_after = Vec::new();
        let shots = run(&mut scene, enemy, 30, |scene, tick| {
            if tick > 0 {
                held_after.push(held(scene, enemy, 0));
            }
            if blocked.contains(&tick) { &low } else { &open }
        });
        assert_eq!(ticks_of(&shots), fired_shots, "{label}: {shots:?}");
        for shot in &shots {
            assert_from_muzzle(shot, label);
        }
        assert_eq!(
            magazine(&scene, enemy),
            27,
            "{label}: each skipped shot spent its own"
        );
        assert_eq!(
            fired(&scene, enemy, 0),
            fired_shots.len() as u32,
            "{label}: only launched shots count as fired"
        );
        assert_eq!(
            held(&scene, enemy, 0),
            blocked.len() as u32,
            "{label}: one held per skipped shot"
        );
        // `held_after[t]` is the count after tick t: each skip counts on its
        // own tick, read by the next tick's guards.
        assert_eq!(held_after[blocked[0]], 1, "{label}: {held_after:?}");
        assert_eq!(held_after[blocked[0] - 1], 0, "{label}: {held_after:?}");
    }
}

/// What a fire-and-rest cycle left: its second burst's shot ticks, and, read
/// before tick 30 once the first burst ended, that burst's fired count and the
/// attack's cooldown.
#[derive(Debug, PartialEq)]
struct Cycle {
    second_burst: Vec<usize>,
    fired_first: u32,
    cooldown_ms: f32,
}

/// A fire-and-rest cycle whose first burst meets `obstruct` on its later
/// shots.
fn second_burst(mut obstruct: impl FnMut(&mut Scene, EntityId, usize) -> bool) -> Cycle {
    let graph = volley_graph(
        "fire",
        vec![("fire", firing("shoot")), ("rest", resting())],
        vec![
            ("fire", vec![edge("rest", after_ms(600.0))]),
            ("rest", vec![edge("fire", after_ms(100.0))]),
        ],
    );
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let low = cover(LOW_LIP);
    let open = CollisionWorld::new();
    let (mut fired_first, mut cooldown_ms) = (0, 0.0);
    let shots = run(&mut scene, enemy, 120, |scene, tick| {
        if tick == 30 {
            fired_first = fired(scene, enemy, 0);
            cooldown_ms = scene.brain(enemy).attack_cooldown_remaining_ms[VOLLEY];
        }
        if obstruct(scene, enemy, tick) {
            &low
        } else {
            &open
        }
    });
    Cycle {
        second_burst: ticks_of(&shots)
            .into_iter()
            .filter(|&tick| tick > 30)
            .collect(),
        fired_first,
        cooldown_ms,
    }
}

// N7 (NM-11): a burst whose last shots are skipped, for a blocked near path or
// because the enemy is not aiming, restarts the attack's cooldown on each
// skipped shot as a fired one does, and starts its next burst on the tick the
// same burst fired unobstructed does. A burst whose first shot fires and later
// ones are skipped reads `attacksFiredInActivity` 1.
#[test]
fn a_burst_whose_last_shots_are_skipped_starts_the_next_on_the_unobstructed_tick() {
    let ticks = burst_ticks();
    let clear = second_burst(|_, _, _| false);
    assert_eq!(
        clear.second_burst.len(),
        3,
        "fixture: the second burst fires: {clear:?}"
    );
    assert_eq!(clear.fired_first, 3);
    let blocked = second_burst(|_, _, tick| tick == ticks[1] || tick == ticks[2]);
    assert_eq!(
        (blocked.second_burst.clone(), blocked.cooldown_ms),
        (clear.second_burst.clone(), clear.cooldown_ms),
        "a blocked last shot restarts the cooldown: {blocked:?}, {clear:?}"
    );
    assert_eq!(blocked.fired_first, 1, "the first fired, the rest skipped");
    let unaimed = second_burst(|scene, enemy, tick| {
        if tick == ticks[2] {
            scene.switch(enemy, "hit_reaction");
        }
        false
    });
    assert_eq!(
        (unaimed.second_burst.clone(), unaimed.cooldown_ms),
        (clear.second_burst.clone(), clear.cooldown_ms),
        "a not-aiming last shot restarts the cooldown: {unaimed:?}, {clear:?}"
    );
    assert_eq!(unaimed.fired_first, 2);
}

// B5 (NM-9): a burst outlasting its activity skips a later shot on a blocked
// near path: it counts held in the activity entered, as a later fired shot
// counts fired there.
#[test]
fn a_burst_outlasting_its_activity_counts_its_skipped_shot_in_the_activity_entered() {
    let ticks = burst_ticks();
    let graph = volley_graph(
        "fire",
        vec![("fire", firing("shoot")), ("rest", resting())],
        vec![(
            "fire",
            vec![edge(
                "rest",
                at_least(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0),
            )],
        )],
    );
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let low = cover(LOW_LIP);
    let open = CollisionWorld::new();
    let mut paths = Vec::new();
    let shots = run(&mut scene, enemy, 30, |scene, tick| {
        paths.push(scene.state(enemy));
        if tick == ticks[1] { &low } else { &open }
    });
    assert!(
        paths[ticks[1]] == "rest",
        "fixture: the burst outlasts `fire`: {paths:?}"
    );
    assert_eq!(ticks_of(&shots), vec![ticks[0], ticks[2]]);
    assert_eq!(scene.state(enemy), "rest");
    assert_eq!(
        held(&scene, enemy, 0),
        1,
        "the skipped shot counts in `rest`"
    );
    assert_eq!(
        fired(&scene, enemy, 0),
        1,
        "the third shot counts fired in `rest`"
    );
}

// B6: a dev-shaped row guarded on
// `attacksFiredInActivity.ge(1).or(attacksHeldInActivity.ge(1))` leaves its
// peek on the tick after a held shot behind near cover, and on the tick after
// a fired shot in the open.
#[test]
fn a_fired_or_held_row_leaves_after_a_held_shot_and_after_a_fired_one() {
    let leave = IrNode::Or {
        a: Box::new(at_least(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0)),
        b: Box::new(at_least(BRAIN_ATTACKS_HELD_IN_ACTIVITY_INPUT, 1.0)),
    };
    let graph = volley_graph(
        "peek",
        vec![("peek", firing("shoot")), ("settle", resting())],
        vec![("peek", vec![edge("settle", leave)])],
    );
    for (label, world, shots_expected) in [
        ("behind near cover", cover(LOW_LIP), 0),
        ("in the open", CollisionWorld::new(), 1),
    ] {
        let mut scene = gunner_scene(single_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "peek");
        let mut paths = Vec::new();
        let shots = run(&mut scene, enemy, 4, |scene, _| {
            paths.push(scene.state(enemy));
            &world
        });
        paths.push(scene.state(enemy));
        assert_eq!(shots.len(), shots_expected, "{label}: {shots:?}");
        // `paths[t]` is the path before tick t.
        assert_eq!(
            paths[1], "peek",
            "{label}: shot on its entry tick: {paths:?}"
        );
        assert_eq!(
            paths[2], "settle",
            "{label}: left on the next tick: {paths:?}"
        );
    }
}

/// `engage` holding an `offense` layer whose leaves `a` and `b` both fire the
/// volley with no clip moment, swapping every `swap_ms`.
fn nested_graph(swap_ms: f32) -> BehaviorGraphDescriptor {
    let offense = BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
        initial: "a".to_string(),
        activities: BTreeMap::from([
            ("a".to_string(), firing("shoot")),
            ("b".to_string(), firing("shoot")),
        ]),
        transitions: BTreeMap::from([
            ("a".to_string(), vec![edge("b", after_ms(swap_ms))]),
            ("b".to_string(), vec![edge("a", after_ms(swap_ms))]),
        ]),
    });
    let mut engage = resting();
    engage.layers = BTreeMap::from([("offense".to_string(), offense)]);
    volley_graph("engage", vec![("engage", engage)], vec![])
}

// B3, B2 (scope-relative reset), NM-7: an attack with no clip moment held
// across a sibling leaf's entry counts again in the leaf entered, while the
// enclosing activity, not entered again, counts the hold once. Once the shot
// fires the hold ends, and the next hold counts again at both depths.
#[test]
fn a_hold_counts_again_per_entered_leaf_while_its_parent_counts_it_once() {
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &nested_graph(200.0), "engage");
    let world = CollisionWorld::new();
    let movers = low_door();
    let mut leaves = Vec::new();
    let mut counts = Vec::new();
    let mut shots = Vec::new();
    for tick in 0..40 {
        // Open for one tick in the second leaf.
        let poses = if tick == 18 {
            open_door()
        } else {
            closed_door()
        };
        let result = tick_full(&mut scene, &world, &movers, &poses, DT, |_| {});
        shots.extend(record(&scene, enemy, tick, &result));
        leaves.push(scene.state(enemy));
        counts.push((held(&scene, enemy, 0), held(&scene, enemy, 1)));
    }
    let b = first(&leaves, "/b");
    let a_again = b + first(&leaves[b..], "/a");
    assert!(
        b < 18 && a_again > 18,
        "fixture: the door opens in `b`: {leaves:?}"
    );
    assert_eq!(counts[b - 1], (1, 1), "the first hold: {counts:?}");
    assert_eq!(
        counts[b],
        (1, 1),
        "the leaf entered counts it again; the parent, not entered, once: {counts:?}"
    );
    assert_eq!(ticks_of(&shots), vec![18], "fires the tick the door opens");
    assert_from_muzzle(&shots[0], "the opened door");
    assert_eq!(
        counts[a_again],
        (2, 1),
        "after a fire the next hold counts again at both depths: {counts:?}"
    );
}

// B4, NM-8: a flinch (an activity routing flinches) cancels a burst in flight
// behind blocking cover: its owed shots never spend, nothing counts held. A
// flinch closing a clip moment behind the same cover counts nothing held.
#[test]
fn a_flinch_behind_blocking_cover_cancels_unspent_and_counts_nothing_held() {
    let ticks = burst_ticks();
    let mut fire = firing("shoot");
    fire.stagger_to = Some("rest".to_string());
    let graph = volley_graph("fire", vec![("fire", fire), ("rest", resting())], vec![]);
    let mut scene = gunner_scene(burst_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let low = cover(LOW_LIP);
    let open = CollisionWorld::new();
    let shots = run(&mut scene, enemy, 30, |scene, tick| {
        if tick == ticks[1] {
            scene.switch(enemy, "hit_reaction");
        }
        if tick >= ticks[1] { &low } else { &open }
    });
    assert_eq!(
        ticks_of(&shots),
        vec![ticks[0]],
        "the flinch cancels the burst"
    );
    assert_eq!(magazine(&scene, enemy), 29, "its owed shots never spend");
    assert_eq!(held(&scene, enemy, 0), 0);

    // A clip moment on the looping rifle clip: a flinch before it is reached.
    let mut fire = firing_at_kick("shoot");
    fire.stagger_to = Some("rest".to_string());
    let graph = volley_graph("fire", vec![("fire", fire), ("rest", resting())], vec![]);
    let moment = {
        let mut scene = gunner_scene(single_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let mut moment = None;
        for tick in 0..20 {
            tick_in(&mut scene, &low);
            if moment.is_none() && held(&scene, enemy, 0) == 1 {
                moment = Some(tick);
            }
        }
        moment.expect("fixture: unflinched, the moment whiffs and counts held")
    };
    assert!(moment > 0, "fixture: the moment falls after the entry tick");
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let shots = run(&mut scene, enemy, 20, |scene, tick| {
        if tick == moment {
            scene.switch(enemy, "hit_reaction");
        }
        &low
    });
    assert!(shots.is_empty());
    assert_eq!(
        held(&scene, enemy, 0),
        0,
        "the flinch closed the moment: nothing held"
    );
}

// ---------------------------------------------------------------------------
// Fire only while aiming
// ---------------------------------------------------------------------------

/// `fire` with the volley on cooldown 150 ms, then `again` after 400 ms, both
/// on the rifle loop.
fn reaction_graph(route: bool) -> BehaviorGraphDescriptor {
    let mut fire = firing("shoot");
    if route {
        fire.stagger_to = Some("rest".to_string());
    }
    volley_graph(
        "fire",
        vec![
            ("fire", fire),
            ("again", firing("shoot")),
            ("rest", resting()),
        ],
        vec![("fire", vec![edge("again", after_ms(400.0))])],
    )
}

/// The earlier NPC: from `wait` it strikes the player in contact on entering
/// `strike`, which a test enters, so its contact hit lands in this tick's
/// apply before the gunner's outcome. Spawned before the gunner.
fn spawn_striker(scene: &mut Scene) -> EntityId {
    let graph = clip_graph(
        "wait",
        vec![
            ("wait", act("idle", None)),
            ("strike", act("idle", Some(untimed("slam")))),
        ],
        vec![("strike", vec![edge("wait", after_ms(10.0))])],
        vec![("slam", contact_attack(0.0))],
    );
    let striker = scene.spawn(1, &graph, "wait");
    set_enemy_position(
        &mut scene.reg,
        striker,
        Vec3::new(PLAYER.x - 1.5, 0.0, PLAYER.z),
    );
    striker
}

/// What a reaction run left.
#[derive(Debug)]
struct ReactionRun {
    shots: Vec<Shot>,
    paths: Vec<String>,
    interruptions: Vec<Interruption>,
    magazines: Vec<u32>,
    held: u32,
}

/// The gunner in [`reaction_graph`], its volley on cooldown until about tick
/// 9, given a reaction: by script before tick `at`'s AI, or by an earlier
/// NPC's contact hit inside tick `at`'s apply. `extra` runs ahead of each tick.
fn reaction_run(
    route: bool,
    contact: bool,
    at: usize,
    mut extra: impl FnMut(&mut Scene, EntityId, usize),
) -> ReactionRun {
    let mut scene = gunner_scene(burst_rifle());
    let striker = contact.then(|| spawn_striker(&mut scene));
    let enemy = spawn_gunner(&mut scene, 0, &reaction_graph(route), "fire");
    seed_cooldown(&mut scene, enemy, 150.0);
    let mut run = ReactionRun {
        shots: Vec::new(),
        paths: Vec::new(),
        interruptions: Vec::new(),
        magazines: Vec::new(),
        held: 0,
    };
    for tick in 0..50 {
        if tick == at {
            match striker {
                Some(striker) => scene.enter(striker, "strike"),
                None => scene.switch(enemy, "hit_reaction"),
            }
        }
        extra(&mut scene, enemy, tick);
        let result = scene.tick_with_impact(|reg| {
            if contact && tick == at {
                let _ = switch_animation_state(reg, enemy, "hit_reaction");
            }
        });
        run.shots.extend(record(&scene, enemy, tick, &result));
        run.paths.push(scene.state(enemy));
        run.interruptions.push(interruption(&scene, enemy));
        run.magazines.push(magazine(&scene, enemy));
    }
    run.held = held(&scene, enemy, 0);
    run
}

// A1, A6 (NM-2, NM-5, NM-13, NM-16), A8 (NM-26), A13, B4 (not aiming): a
// reaction drawn in an activity with no flinch route, by script or by an
// earlier NPC's contact hit after this NPC's fire decision. The enemy is not
// aiming: the attack, ready from about tick 9, launches nothing from the eye
// or the muzzle while the reaction stays drawn, its activation never starting
// (nothing spent, no held count). The record holds until the brain's own
// draw: `again`'s entry, onto the state the reaction kept the clock on, fires
// the whole burst from the muzzle posed at the start that entry draws.
#[test]
fn a_reaction_without_a_flinch_route_silences_the_gunner_until_its_own_draw() {
    // Unreacted, the volley first fires once its cooldown runs out.
    let ready = reaction_run(false, false, usize::MAX, |_, _, _| {}).shots[0].tick;
    assert!(
        ready > 3,
        "fixture: the volley is ready after the script reaction"
    );
    for (label, contact, at) in [
        ("a script reaction", false, 3),
        ("a contact hit's reaction on the decision tick", true, ready),
    ] {
        let run = reaction_run(false, contact, at, |_, _, _| {});
        let entry = first(&run.paths, "again");
        let ticks = ticks_of(&run.shots);
        assert_eq!(
            ticks.first().copied(),
            Some(entry),
            "{label}: nothing launches before the own draw lands, then it fires: {run:?}"
        );
        assert_eq!(ticks.len(), 3, "{label}: the whole burst: {run:?}");
        assert!(
            run.magazines[..entry].iter().all(|&left| left == 30),
            "{label}: no activation started under the reaction: {run:?}"
        );
        assert!(
            run.interruptions[at..entry]
                .iter()
                .all(|&record| record == Interruption::Foreign),
            "{label}: the record holds until the own draw: {run:?}"
        );
        assert!(
            run.interruptions[entry..]
                .iter()
                .all(|&record| record == Interruption::None),
            "{label}: and clears on it: {run:?}"
        );
        assert_from_muzzle(&run.shots[0], label);
        assert_eq!(run.held, 0, "{label}: not aiming counts nothing held");
    }

    // NM-26: posed where the entry's switch starts the clip, not at the clip
    // time the clock kept under the reaction. The gunner stands still, so the
    // pose before the entry tick is its pose on it.
    let entry = first(&reaction_run(false, false, 3, |_, _, _| {}).paths, "again");
    let mut expected = None;
    let run = reaction_run(false, false, 3, |scene, enemy, tick| {
        if tick == entry {
            let clock = scene.brain(enemy).clip_clock.clone();
            let start = crate::clip_clock::start_time(&clock, enemy);
            expected = Some((
                posed(scene, enemy, "shoot", start, gunner_mount()).unwrap(),
                posed(
                    scene,
                    enemy,
                    "shoot",
                    clock.time + f64::from(DT),
                    gunner_mount(),
                )
                .unwrap(),
            ));
        }
    });
    let (start, kept) = expected.unwrap();
    assert!(
        run.shots[0].origin.distance(start) < 1.0e-4,
        "at the entry's clip start: {:?}, {start}",
        run.shots[0]
    );
    assert!(
        run.shots[0].origin.distance(kept) > 1.0e-3,
        "not at the clock's kept time: {:?}, {kept}",
        run.shots[0]
    );

    // A13: the record survives a save and restore, so a restored enemy fires
    // on the tick an unsaved run does.
    let unsaved = ticks_of(&reaction_run(false, false, 3, |_, _, _| {}).shots);
    let restored = reaction_run(false, false, 3, |scene, enemy, tick| {
        if tick == 6 {
            assert_eq!(interruption(scene, enemy), Interruption::Foreign, "fixture");
            save_and_restore(scene, enemy);
            assert_eq!(
                interruption(scene, enemy),
                Interruption::Foreign,
                "the record round-trips with the brain"
            );
        }
    });
    assert_eq!(
        ticks_of(&restored.shots),
        unsaved,
        "restored fires on the unsaved tick"
    );
}

// A1 (flinch route, NM-8): in an activity routing flinches, today's stagger
// applies: nothing fires while the reaction plays, and the route is taken.
#[test]
fn a_reaction_under_a_flinch_route_staggers_as_today() {
    let run = reaction_run(true, false, 3, |_, _, _| {});
    assert!(
        run.shots.is_empty(),
        "nothing fires through the stagger: {run:?}"
    );
    let rest = first(&run.paths, "rest");
    assert!(
        rest > 3 && rest <= 3 + ((0.45 / DT) as usize) + 1,
        "the route is taken once the reaction plays out: {run:?}"
    );
    assert_eq!(run.held, 0);
}

/// A burst in `fire` reached by an earlier NPC's contact hit on `at`, the
/// gunner's stride starting that tick when `stride`.
fn contact_burst(route: bool, stride: bool, at: usize) -> (Vec<Shot>, u32, Vec<Interruption>) {
    let mut fire = firing("shoot");
    if route {
        fire.stagger_to = Some("rest".to_string());
    }
    let graph = volley_graph("fire", vec![("fire", fire), ("rest", resting())], vec![]);
    let mut scene = gunner_scene(burst_rifle());
    let striker = spawn_striker(&mut scene);
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let mut shots = Vec::new();
    let mut records = Vec::new();
    for tick in 0..30 {
        if tick == at {
            scene.enter(striker, "strike");
            if stride {
                scene.set_agent(enemy, |agent| agent.velocity = Vec3::new(1.0, 0.0, 0.0));
            }
        }
        let result = scene.tick_with_impact(|reg| {
            if tick == at {
                let _ = switch_animation_state(reg, enemy, "hit_reaction");
            }
        });
        shots.extend(record(&scene, enemy, tick, &result));
        records.push(interruption(&scene, enemy));
    }
    (shots, magazine(&scene, enemy), records)
}

// A4 (NM-6), A5 (NM-28): an earlier NPC's contact hit runs a reaction on the
// gunner in the same apply, after its compute, on a due burst shot's tick.
// With no own draw the gunner is not aiming: the shot is skipped and spends,
// and with no flinch route the rest skip too; with one, the next tick's flinch
// cancels them unspent. Its own stride starting that tick draws over the
// reaction with no flinch route: aiming, the shot leaves the muzzle; with a
// route the stride does not draw, and the shot is skipped as before.
#[test]
fn an_earlier_npcs_contact_reaction_mid_burst_skips_unless_an_own_draw_lands() {
    let ticks = burst_ticks();
    let at = ticks[1];
    let (shots, left, records) = contact_burst(false, false, at);
    assert_eq!(ticks_of(&shots), vec![ticks[0]], "no route: the rest skip");
    assert_eq!(left, 27, "each skipped shot spends");
    assert_eq!(records[at], Interruption::Foreign);

    let (shots, left, _) = contact_burst(true, false, at);
    assert_eq!(
        ticks_of(&shots),
        vec![ticks[0]],
        "a route: skipped, then cancelled"
    );
    assert_eq!(
        left, 28,
        "the skipped shot spent, the cancelled one did not"
    );

    let (shots, left, records) = contact_burst(false, true, at);
    assert_eq!(ticks_of(&shots), ticks, "the stride's own draw: aiming");
    assert_from_muzzle(&shots[1], "the stride tick");
    assert_eq!(left, 27);
    assert_eq!(
        records[at],
        Interruption::None,
        "no record remains after apply"
    );

    let (shots, left, _) = contact_burst(true, true, at);
    assert_eq!(
        ticks_of(&shots),
        vec![ticks[0]],
        "a route keeps the reaction drawn over the stride"
    );
    assert_eq!(left, 28);
}

// A5 (NM-28, entry) and A7 (NM-16): a reaction drawn on the tick the brain's
// own activity entry lands, by script before the AI or by an earlier NPC's
// contact hit in its apply: the gunner is aiming, its clip moment reached on
// the entry tick fires from the muzzle posed on the entered state, and no
// record remains.
#[test]
fn a_reaction_on_an_entry_tick_is_drawn_over_and_the_entry_fires() {
    let graph = volley_graph(
        "rest",
        vec![("rest", resting()), ("fire", firing_at_kick("strike"))],
        vec![("rest", vec![edge("fire", after_ms(100.0))])],
    );
    let entry = {
        let mut scene = gunner_scene(single_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
        let open = CollisionWorld::new();
        let shots = run(&mut scene, enemy, 20, |_, _| &open);
        assert_eq!(shots.len(), 1, "fixture: one shot at the entry's moment");
        shots[0].tick
    };
    for contact in [false, true] {
        let mut scene = gunner_scene(single_rifle());
        let striker = contact.then(|| spawn_striker(&mut scene));
        let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
        let mut shots = Vec::new();
        for tick in 0..20 {
            if tick == entry {
                match striker {
                    Some(striker) => scene.enter(striker, "strike"),
                    None => scene.switch(enemy, "hit_reaction"),
                }
            }
            let result = scene.tick_with_impact(|reg| {
                if contact && tick == entry {
                    let _ = switch_animation_state(reg, enemy, "hit_reaction");
                }
            });
            shots.extend(record(&scene, enemy, tick, &result));
            if tick == entry {
                assert_eq!(
                    interruption(&scene, enemy),
                    Interruption::None,
                    "contact {contact}"
                );
            }
        }
        assert_eq!(
            ticks_of(&shots),
            vec![entry],
            "contact {contact}: the entry fires"
        );
        assert_from_muzzle(&shots[0], "the entry tick");
        assert!(
            shots[0]
                .origin
                .distance(posed(&scene, enemy, "strike", 0.0, gunner_mount()).unwrap())
                < 1.0e-4,
            "posed on the entered state at its start"
        );
    }
}

// A2 (NM-25), A3 (NM-31): a held gunner dropped while its tracked state is its
// mesh's default. Before the fresh mesh is redrawn a due shot launches nothing
// and counts nothing held; the tick after the redraw it fires from the muzzle.
// Given a reaction before the redraw (no flinch route), the redraw is dropped
// and nothing launches until the brain's next own draw, whose tick fires.
#[test]
fn a_dropped_gunner_fires_only_once_redrawn_or_after_a_reactions_own_draw() {
    let graph = volley_graph(
        "fire",
        vec![("fire", firing("idle")), ("again", firing("idle"))],
        vec![("fire", vec![edge("again", after_ms(300.0))])],
    );
    const DROP: usize = 6;
    for reaction in [false, true] {
        let mut scene = gunner_scene(single_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        // Out of range until the drop.
        set_enemy_position(&mut scene.reg, scene.pawn, Vec3::new(20.0, 1.0, PLAYER.z));
        let mut holder = None;
        let mut shots = Vec::new();
        let mut paths = Vec::new();
        let mut drawn = Vec::new();
        let open = CollisionWorld::new();
        for tick in 0..40 {
            match tick {
                2 => holder = Some(pick_up(&mut scene, enemy)),
                DROP => {
                    drop_from(&mut scene, enemy, holder.take().unwrap());
                    let pawn = scene.pawn;
                    set_enemy_position(&mut scene.reg, pawn, PLAYER);
                }
                _ if tick == DROP + 1 => {
                    resolve_dropped(&mut scene, enemy);
                    if reaction {
                        scene.switch(enemy, "hit_reaction");
                    }
                }
                _ => {}
            }
            let result = tick_in(&mut scene, &open);
            shots.extend(record(&scene, enemy, tick, &result));
            paths.push(scene.state(enemy));
            if scene.reg.get_component::<MeshComponent>(enemy).is_ok() {
                drawn.push((tick, mesh_state(&scene, enemy)));
            }
        }
        if !reaction {
            assert!(
                drawn.contains(&(DROP, "idle".to_string())),
                "fixture: the fresh mesh draws the tracked default: {drawn:?}"
            );
            assert_eq!(
                scene.brain(enemy).clip_clock.state.as_deref(),
                Some("idle"),
                "fixture: the tracked state is the mesh's default"
            );
            assert_eq!(
                ticks_of(&shots).first().copied(),
                Some(DROP + 2),
                "no launch on the drop or the redraw tick; the next fires: {shots:?}"
            );
        } else {
            let again = first(&paths, "again");
            assert_eq!(
                ticks_of(&shots),
                vec![again],
                "silent until the own draw of `again`: {shots:?}, {paths:?}"
            );
        }
        assert_from_muzzle(&shots[0], "after the drop");
        assert_eq!(held(&scene, enemy, 0), 0);
    }
}

// A9 (NM-17), A10 (NM-30): a gunner knocked airborne mid-burst. Before the
// airborne clip shows (the fall delay), it aims and fires from the muzzle on
// its tracked state; every tick whose launch sees `jump_loop` drawn, through
// the landing tick, is not aiming: the due shot is skipped and spends. After
// landing it fires again. A mesh with no usable `jump_loop` fires throughout.
#[test]
fn a_knocked_back_gunner_fires_until_its_airborne_clip_shows_and_after_landing() {
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let weapon = || rifle(12, 50.0, 1000.0, 30);
    let control = {
        let mut scene = gunner_scene(weapon());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let open = CollisionWorld::new();
        ticks_of(&run(&mut scene, enemy, 60, |_, _| &open))
    };
    assert_eq!(control.len(), 12, "fixture: {control:?}");
    for jump_loop in [true, false] {
        let mut scene = gunner_scene(weapon());
        if !jump_loop {
            scene
                .mesh
                .animation
                .as_mut()
                .unwrap()
                .states
                .get_mut("jump_loop")
                .unwrap()
                .clip_index = None;
        }
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let open = CollisionWorld::new();
        let (launch, land) = (control[1] + 1, control[8]);
        let mut drawn_before = Vec::new();
        let shots = run(&mut scene, enemy, 60, |scene, tick| {
            drawn_before.push(mesh_state(scene, enemy));
            if tick == launch {
                scene.set_agent(enemy, |agent| {
                    agent.knockback_velocity = Vec3::new(2.0, 0.0, 0.0);
                    agent.ground = postretro_foundation::GroundRef::Airborne;
                });
            }
            if tick == land {
                scene.set_agent(enemy, |agent| {
                    agent.knockback_velocity = Vec3::ZERO;
                    agent.ground = postretro_foundation::GroundRef::World;
                });
            }
            &open
        });
        let unaimed: Vec<usize> = control
            .iter()
            .copied()
            .filter(|&tick| drawn_before[tick] == "jump_loop")
            .collect();
        let expected: Vec<usize> = control
            .iter()
            .copied()
            .filter(|tick| !unaimed.contains(tick))
            .collect();
        assert_eq!(
            ticks_of(&shots),
            expected,
            "jump_loop {jump_loop}: {drawn_before:?}"
        );
        for shot in &shots {
            assert_from_muzzle(shot, "a shot while grounded or before the airborne clip");
        }
        assert_eq!(
            magazine(&scene, enemy),
            18,
            "jump_loop {jump_loop}: every due shot spent"
        );
        if jump_loop {
            assert!(
                !unaimed.is_empty(),
                "fixture: shots fall due under the airborne clip"
            );
            assert!(
                shots
                    .iter()
                    .any(|shot| shot.tick > launch && shot.tick < unaimed[0]),
                "fixture: a shot fires airborne before the fall delay: {shots:?}"
            );
            assert!(
                shots.iter().any(|shot| shot.tick > land),
                "fires again after landing: {shots:?}"
            );
        } else {
            assert!(unaimed.is_empty(), "no airborne clip shows");
        }
    }
}

// X2, A11: no pose launches from the eye with no new warning. An activity that
// draws no animation, so the clip clock tracks none; a drawn state whose clip
// does not resolve on the model; an own switch that fails with an unknown
// state, which only apply's own warning reports.
#[test]
fn a_gunner_with_no_pose_to_read_fires_from_the_eye_without_a_new_warning() {
    let mut no_animation = firing("aim");
    no_animation.animation = None;
    for (label, activity, warnings) in [
        ("an activity drawing no animation", no_animation, 0),
        ("a clip that does not resolve", firing("blank"), 0),
        ("a failed own switch", firing("undeclared"), 1),
    ] {
        let graph = volley_graph("fire", vec![("fire", activity)], vec![]);
        let mut scene = gunner_scene(single_rifle());
        // Declared, its clip index past the body's library.
        scene.mesh.animation.as_mut().unwrap().states.insert(
            "blank".to_string(),
            animation_state("no_such_clip", true, Some(999)),
        );
        let capture = postretro_test_log_capture::LogCapture::start();
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let open = CollisionWorld::new();
        let shots = run(&mut scene, enemy, 3, |_, _| &open);
        let [shot] = shots.as_slice() else {
            panic!("{label}: one shot: {shots:?}");
        };
        assert_from_eye(shot, label);
        let warned: Vec<_> = capture
            .records()
            .into_iter()
            .filter(|record| record.level <= log::Level::Warn)
            .collect();
        assert_eq!(warned.len(), warnings, "{label}: {warned:#?}");
        if warnings == 1 {
            assert!(
                warned[0].message.contains("could not be switched"),
                "{label}: apply's own warning: {warned:#?}"
            );
        }
    }
}

// A12 (NM-32): a reaction drawn, then the brain's own switch fails (an entry
// onto an undeclared state), the mesh keeping the reaction: still not aiming,
// nothing launches from the eye or the muzzle until a later own draw lands.
#[test]
fn a_failed_own_switch_under_a_reaction_stays_silent_until_an_own_draw() {
    let graph = volley_graph(
        "rest",
        vec![
            ("rest", resting()),
            ("broken", firing("undeclared")),
            ("fire", firing("shoot")),
        ],
        vec![
            ("rest", vec![edge("broken", after_ms(100.0))]),
            ("broken", vec![edge("fire", after_ms(300.0))]),
        ],
    );
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
    let open = CollisionWorld::new();
    let mut paths = Vec::new();
    let mut records = Vec::new();
    let shots = run(&mut scene, enemy, 40, |scene, tick| {
        if tick == 2 {
            scene.switch(enemy, "hit_reaction");
        }
        paths.push(scene.state(enemy));
        records.push(interruption(scene, enemy));
        &open
    });
    // `paths[t]` is the path before tick t: the tick that entered is one less.
    let broken = first(&paths, "broken") - 1;
    let fire = first(&paths, "fire") - 1;
    assert_eq!(
        records[broken + 1],
        Interruption::Foreign,
        "the failed switch keeps the reaction's record"
    );
    assert_eq!(
        ticks_of(&shots),
        vec![fire],
        "silent until `fire`'s own draw: {paths:?}"
    );
    assert_from_muzzle(&shots[0], "the own draw");
}

// A14 (NM-10): a strafing gunner mid-burst whose travel variant changes, or
// whose stride starts, on a due shot's tick fires that shot from the muzzle
// posed on the newly tracked state; no due shot is skipped for its own switch.
#[test]
fn a_strafing_gunners_own_switch_on_a_due_shot_fires_from_the_new_state() {
    let ticks = burst_ticks();
    let mut fire = firing("walk");
    fire.facing = Some(FacingPolicy::Target);
    let graph = volley_graph("fire", vec![("fire", fire)], vec![]);
    for (label, velocity, state) in [
        ("a travel variant", Vec3::new(0.0, 0.0, 2.0), "walk_right"),
        ("a stride starting", Vec3::new(1.0, 0.0, 0.0), "walk"),
    ] {
        let mut scene = gunner_scene(burst_rifle());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let open = CollisionWorld::new();
        let shots = run(&mut scene, enemy, 30, |scene, tick| {
            if tick == ticks[1] {
                scene.set_agent(enemy, |agent| agent.velocity = velocity);
            }
            &open
        });
        assert_eq!(ticks_of(&shots), ticks, "{label}: none skipped: {shots:?}");
        assert_eq!(mesh_state(&scene, enemy), state, "{label}: fixture");
        assert_eq!(scene.brain(enemy).clip_clock.state.as_deref(), Some(state));
        for shot in &shots[1..] {
            assert_from_muzzle(shot, label);
        }
    }
}

// M4, M5 (launch half, NM-3): a carried weapon with no muzzle to read launches
// from the eye, and the launch adds no warning to the install's: no table, a
// table with no entry for the weapon, an entry on a socket the rig lacks, and
// an entry whose bake the level did not load.
#[test]
fn a_weapon_with_no_usable_mount_launches_from_the_eye_without_a_warning() {
    let entry_on = |socket: &str, bake: &str| MountDescriptor {
        item: RIFLE.to_string(),
        attachments: Some(std::collections::HashMap::from([(
            socket.to_string(),
            bake.to_string(),
        )])),
        muzzle_socket: None,
    };
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    for (label, mounts) in [
        ("no table", None),
        ("no entry", Some(Vec::new())),
        (
            "a socket the rig lacks",
            Some(vec![entry_on("hand_x", BAKE)]),
        ),
        (
            "a bake the level did not load",
            Some(vec![entry_on("hand_r", "models/ar_4/never_loaded.gltf")]),
        ),
    ] {
        let mut scene = gunner_scene(single_rifle());
        scene.descriptors[1] = archetype(BODY, mounts);
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let capture = postretro_test_log_capture::LogCapture::start();
        let open = CollisionWorld::new();
        let shots = run(&mut scene, enemy, 3, |_, _| &open);
        let [shot] = shots.as_slice() else {
            panic!("{label}: one shot: {shots:?}");
        };
        assert_from_eye(shot, label);
        assert_toward(shot, AIM, label);
        let warned: Vec<_> = capture
            .records()
            .into_iter()
            .filter(|record| record.level <= log::Level::Warn)
            .collect();
        assert!(
            warned.is_empty(),
            "{label}: the launch warns nothing: {warned:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Regressions from review
// ---------------------------------------------------------------------------

/// `descriptor` renamed `name`, its projectile sweeping at `radius`.
fn renamed(mut descriptor: EntityTypeDescriptor, name: &str, radius: f32) -> EntityTypeDescriptor {
    descriptor.canonical_name = Some(name.to_string());
    descriptor
        .weapon
        .as_mut()
        .unwrap()
        .projectile
        .as_mut()
        .unwrap()
        .radius = radius;
    descriptor
}

fn attack_on(weapon: &str) -> AttackParams {
    AttackParams {
        weapon: Some(weapon.to_string()),
        ..rifle_attack()
    }
}

fn volley_cooldown(scene: &Scene, enemy: EntityId) -> f32 {
    scene
        .brain(enemy)
        .attack_cooldown_remaining_ms
        .get(VOLLEY)
        .copied()
        .unwrap_or(0.0)
}

fn seed_interruption(scene: &mut Scene, enemy: EntityId, record: Interruption) {
    let mut brain = scene.brain(enemy);
    brain.clip_clock.interruption = record;
    scene.reg.set_component(enemy, brain).unwrap();
}

// Regression: a charged weapon's first shot, reached on its release tick after
// the charge began, was skipped as a later burst shot when near cover held it
// or the enemy was not aiming: it spent, restarted the attack's cooldown and
// counted per skip instead of once per hold.
#[test]
fn a_charged_first_shot_that_cannot_launch_leaves_the_attack_unstarted() {
    let charged = || {
        let mut descriptor = rifle(1, 0.0, 300.0, 30);
        descriptor.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
            "trigger": "press", "recoveryMs": 300,
            "charge": {"minMs": 0, "fullMs": 50},
            "steps": [{"kind": "shot"}],
        }))
        .expect("a valid charged action");
        descriptor
    };
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let low = cover(LOW_LIP);
    let open = CollisionWorld::new();
    for (label, reaction) in [("near cover", false), ("not aiming", true)] {
        let mut scene = gunner_scene(charged());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let mut cooldowns = Vec::new();
        let shots = run(&mut scene, enemy, 60, |scene, tick| {
            if reaction && tick == 1 {
                scene.switch(enemy, "hit_reaction");
            }
            cooldowns.push(volley_cooldown(scene, enemy));
            if reaction { &open } else { &low }
        });
        assert!(shots.is_empty(), "{label}: nothing launches: {shots:?}");
        assert_eq!(magazine(&scene, enemy), 30, "{label}: nothing spent");
        assert!(
            cooldowns.iter().all(|&cooldown| cooldown <= 0.0),
            "{label}: the attack's cooldown never restarts: {cooldowns:?}"
        );
        assert_eq!(
            held(&scene, enemy, 0),
            u32::from(!reaction),
            "{label}: a hold counts once, not aiming counts nothing"
        );
        if !reaction {
            // It proposes again: once the path clears, the charge fires.
            let shots = run(&mut scene, enemy, 15, |_, _| &open);
            let [shot] = shots.as_slice() else {
                panic!("{label}: the cleared charge fires once: {shots:?}");
            };
            assert_from_muzzle(shot, label);
            assert_eq!(magazine(&scene, enemy), 29);
        }
    }
}

// Regression: a request for another weapon cancelled the burst in flight even
// when the new weapon's first shot could not launch, silencing both.
#[test]
fn a_held_request_for_another_weapon_leaves_the_burst_in_flight_firing() {
    use postretro_foundation::attack_budget::{AttackBudgetVocabulary, TokenDraw};
    const LONG: &str = "npc.long";
    const SNIPE: &str = "snipe";
    let drawing = |weapon: &str, token: &str| AttackParams {
        draws: Some(vec![TokenDraw(serde_json::json!({
            "kind": "attackToken", "name": token
        }))]),
        ..attack_on(weapon)
    };
    // Each attack draws its own token, so the long shot's turn is granted
    // while the burst's runs on.
    let graph = clip_graph(
        "spray",
        vec![
            ("spray", firing("shoot")),
            ("far", act("shoot", Some(untimed(SNIPE)))),
        ],
        vec![(
            "spray",
            vec![edge(
                "far",
                at_least(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0),
            )],
        )],
        vec![
            (VOLLEY, drawing(RIFLE, "burst")),
            (SNIPE, drawing(LONG, "long")),
        ],
    );
    // A lip 0.15 m under the gun's line: clear of the burst rifle's 0.05 m
    // shot, inside the long rifle's 0.3 m one.
    let lip = cover(1.33);
    let open = CollisionWorld::new();
    let run_with = |world: &CollisionWorld| {
        let mut scene = gunner_scene(renamed(burst_rifle(), RIFLE, 0.05));
        scene
            .descriptors
            .insert(1, renamed(rifle(1, 0.0, 1000.0, 30), LONG, 0.3));
        let mut mount = rifle_mount();
        mount.item = LONG.to_string();
        scene.descriptors[2] = archetype(BODY, Some(vec![rifle_mount(), mount]));
        let vocabulary = AttackBudgetVocabulary::from_manifest(
            Some(&serde_json::json!([
                { "kind": "attackToken", "name": "burst", "budget": { "limit": 1 } },
                { "kind": "attackToken", "name": "long", "budget": { "limit": 1 } },
            ])),
            None,
        )
        .expect("a valid token registration");
        scene.reg.set_attack_budgets(Arc::new(vocabulary));
        let enemy = spawn_gunner(&mut scene, 0, &graph, "spray");
        let mut shots = Vec::new();
        for tick in 0..40 {
            let result = tick_in(&mut scene, world);
            shots.extend(
                result
                    .projectile_spawns
                    .iter()
                    .map(|spawn| (tick, spawn.descriptor_class.clone())),
            );
        }
        let magazines = (
            magazine(&scene, enemy),
            scene.runtime.weapons.weapon(enemy, LONG).unwrap().magazine,
        );
        (shots, magazines, scene.state(enemy), held(&scene, enemy, 0))
    };
    let ticks = burst_ticks();
    let (shots, magazines, path, held_far) = run_with(&lip);
    assert_eq!(path, "far", "fixture: the burst outlasts `spray`");
    assert_eq!(
        shots,
        ticks
            .iter()
            .map(|&tick| (tick, RIFLE.to_string()))
            .collect::<Vec<_>>(),
        "the held long shot leaves the burst firing"
    );
    assert_eq!(magazines, (27, 30), "the long rifle's attack never started");
    assert!(held_far >= 1, "the long shot was held: {held_far}");

    let (shots, magazines, _, _) = run_with(&open);
    assert_eq!(
        shots,
        vec![
            (ticks[0], RIFLE.to_string()),
            (ticks[0] + 1, LONG.to_string())
        ],
        "a long shot that launches starts and cancels the burst, as today"
    );
    assert_eq!(magazines, (29, 29));
}

// Regression: a held enemy dropped while carrying a failed own switch's
// record fired from the eye before its fresh mesh was redrawn.
#[test]
fn a_dropped_gunner_with_a_failed_switch_record_fires_nothing_until_redrawn() {
    const DROP: usize = 6;
    let graph = volley_graph("fire", vec![("fire", firing("idle"))], vec![]);
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    // Out of range until the drop.
    let pawn = scene.pawn;
    set_enemy_position(&mut scene.reg, pawn, Vec3::new(20.0, 1.0, PLAYER.z));
    let open = CollisionWorld::new();
    let mut holder = None;
    let shots = run(&mut scene, enemy, 20, |scene, tick| {
        match tick {
            2 => seed_interruption(scene, enemy, Interruption::FailedOwnSwitch),
            3 => holder = Some(pick_up(scene, enemy)),
            DROP => {
                assert_eq!(
                    interruption(scene, enemy),
                    Interruption::FailedOwnSwitch,
                    "fixture: the record rides out the hold"
                );
                drop_from(scene, enemy, holder.take().unwrap());
                set_enemy_position(&mut scene.reg, pawn, PLAYER);
            }
            _ if tick == DROP + 1 => resolve_dropped(scene, enemy),
            _ => {}
        }
        &open
    });
    assert_eq!(
        ticks_of(&shots).first().copied(),
        Some(DROP + 2),
        "nothing launches on the drop or the redraw tick: {shots:?}"
    );
    assert_from_muzzle(&shots[0], "after the redraw");
}

// Regression: an entry landing on the tick a dropped gunner's redraw was
// pending, its fresh mesh already showing the tracked state's name, posed the
// shot at the clock's held time rather than where the redraw starts the clip.
#[test]
fn an_entry_on_a_pending_redraw_poses_the_shot_at_the_clips_start() {
    const DROP: usize = 10;
    let graph = volley_graph(
        "fire",
        vec![("fire", firing("shoot")), ("again", firing("shoot"))],
        vec![],
    );
    let mut scene = gunner_scene(single_rifle());
    // The rifle loop is the mesh's default, so the fresh mesh shows it.
    let states = scene.mesh.animation.as_ref().unwrap().states.clone();
    scene.mesh =
        MeshComponent::animated(BODY.to_string(), MeshAnimation::new(states, "shoot".into()));
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let pawn = scene.pawn;
    set_enemy_position(&mut scene.reg, pawn, Vec3::new(20.0, 1.0, PLAYER.z));
    let open = CollisionWorld::new();
    let mut holder = None;
    let mut expected = None;
    let shots = run(&mut scene, enemy, DROP + 4, |scene, tick| {
        match tick {
            5 => holder = Some(pick_up(scene, enemy)),
            DROP => {
                drop_from(scene, enemy, holder.take().unwrap());
                set_enemy_position(&mut scene.reg, pawn, PLAYER);
            }
            _ if tick == DROP + 1 => {
                resolve_dropped(scene, enemy);
                scene.enter(enemy, "again");
                assert_eq!(mesh_state(scene, enemy), "shoot", "fixture");
                let clock = scene.brain(enemy).clip_clock.clone();
                assert!(clock.redraw, "fixture: the redraw is pending");
                let start = crate::clip_clock::start_time(&clock, enemy);
                expected = Some((
                    posed(scene, enemy, "shoot", start, gunner_mount()).unwrap(),
                    posed(scene, enemy, "shoot", clock.time, gunner_mount()).unwrap(),
                ));
            }
            _ => {}
        }
        &open
    });
    let [shot] = shots.as_slice() else {
        panic!("one shot, on the entry tick: {shots:?}");
    };
    assert_eq!(shot.tick, DROP + 1, "the entry's own draw makes it aiming");
    let (start, held_time) = expected.unwrap();
    assert!(
        start.distance(held_time) > 1.0e-3,
        "fixture: the clip's start and the held time pose apart"
    );
    assert!(
        shot.origin.distance(start) < 1.0e-4,
        "posed at the clip's start, {start}, not the held time, {held_time}: {shot:?}"
    );
}

// Regression: a contact strike ended a near-path hold, so a projectile attack
// still held after it counted again in the enclosing activity.
#[test]
fn a_contact_strike_leaves_a_hold_running_and_a_launched_shot_ends_it() {
    let offense = BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
        initial: "shoot".to_string(),
        activities: BTreeMap::from([
            ("shoot".to_string(), firing("shoot")),
            ("melee".to_string(), act("aim", Some(untimed(JAB)))),
        ]),
        transitions: BTreeMap::from([
            ("shoot".to_string(), vec![edge("melee", after_ms(100.0))]),
            ("melee".to_string(), vec![edge("shoot", after_ms(100.0))]),
        ]),
    });
    let mut engage = resting();
    engage.layers = BTreeMap::from([("offense".to_string(), offense)]);
    let graph = clip_graph(
        "engage",
        vec![("engage", engage)],
        vec![],
        vec![(VOLLEY, rifle_attack()), (JAB, contact_attack(0.0))],
    );
    let mut scene = gunner_scene(single_rifle());
    // Within the jab's reach; the low door still sits between gun and aim.
    let pawn = scene.pawn;
    set_enemy_position(&mut scene.reg, pawn, Vec3::new(1.5, 1.0, PLAYER.z));
    let enemy = spawn_gunner(&mut scene, 0, &graph, "engage");
    let world = CollisionWorld::new();
    let movers = low_door();
    let (mut visits, mut since_entry, mut last_leaf) = (0, 0, String::new());
    // The enclosing count as each visit to `shoot` after the first begins.
    let mut held_at_visit = Vec::new();
    let mut shots = Vec::new();
    let mut struck = 0;
    for tick in 0..120 {
        // The door opens for one tick, three ticks into the second visit.
        let open = visits == 2 && since_entry == 3;
        let poses = if open { open_door() } else { closed_door() };
        let hp = player_hp(&scene.reg, pawn);
        let result = tick_full(&mut scene, &world, &movers, &poses, DT, |_| {});
        if player_hp(&scene.reg, pawn) < hp {
            struck += 1;
        }
        shots.extend(record(&scene, enemy, tick, &result));
        let leaf = scene.state(enemy);
        if leaf != last_leaf && leaf.ends_with("/shoot") {
            visits += 1;
            since_entry = 0;
            held_at_visit.push(held(&scene, enemy, 0));
        } else {
            since_entry += 1;
        }
        last_leaf = leaf;
        if visits == 3 && since_entry == 2 {
            break;
        }
    }
    assert!(
        struck >= 2,
        "fixture: the melee leaf's jab lands each visit"
    );
    assert_eq!(
        ticks_of(&shots).len(),
        1,
        "fixture: one shot, through the opened door: {shots:?}"
    );
    assert_eq!(
        held_at_visit,
        vec![1, 1, 2],
        "a jab between holds adds nothing; the launched shot ends the hold, so \
         the next counts again"
    );
}

// Regression: on the tick an own switch failed with no record yet, the launch
// read the failing state's pose and fired from a muzzle the mesh did not draw.
#[test]
fn the_tick_an_own_switch_fails_fires_from_the_eye_or_not_at_all_under_a_reaction() {
    let graph = volley_graph(
        "rest",
        vec![("rest", resting()), ("fire", firing("pending"))],
        vec![("rest", vec![edge("fire", after_ms(100.0))])],
    );
    for reaction in [false, true] {
        let mut scene = gunner_scene(single_rifle());
        // Declared on a clip the model carries but unresolved on the mesh, so
        // the switch fails while the clip would still pose by name.
        scene.mesh.animation.as_mut().unwrap().states.insert(
            "pending".to_string(),
            animation_state("firing_rifle", true, None),
        );
        let enemy = spawn_gunner(&mut scene, 0, &graph, "rest");
        let open = CollisionWorld::new();
        let mut paths = Vec::new();
        let mut records = Vec::new();
        let shots = run(&mut scene, enemy, 20, |scene, tick| {
            if reaction && tick == 2 {
                scene.switch(enemy, "hit_reaction");
            }
            paths.push(scene.state(enemy));
            records.push(interruption(scene, enemy));
            &open
        });
        // `paths[t]` is the path before tick t: the tick that entered is one less.
        let entry = first(&paths, "fire") - 1;
        if reaction {
            assert!(
                shots.is_empty(),
                "under a reaction nothing launches: {shots:?}"
            );
            assert_eq!(records[entry + 1], Interruption::Foreign);
        } else {
            let [shot] = shots.as_slice() else {
                panic!("one shot on the failing tick: {shots:?}");
            };
            assert_eq!(shot.tick, entry, "the failing switch's own tick");
            assert_from_eye(shot, "a failed own switch");
            assert_eq!(records[entry + 1], Interruption::FailedOwnSwitch);
        }
    }
}

// ---------------------------------------------------------------------------
// Regressions from review, round 2
// ---------------------------------------------------------------------------

const LONG: &str = "npc.long";
const SNIPE: &str = "snipe";

/// A lip 0.145 m under the gun's line: clear of a 0.05 m or 0.1 m shot,
/// inside a 0.3 m one.
fn under_lip() -> CollisionWorld {
    cover(1.33)
}

/// An attack on `weapon` drawing its own `token`, so two attacks of one
/// gunner hold turns side by side.
fn drawing(weapon: &str, token: &str) -> AttackParams {
    AttackParams {
        draws: Some(vec![postretro_foundation::attack_budget::TokenDraw(
            serde_json::json!({
                "kind": "attackToken", "name": token
            }),
        )]),
        ..attack_on(weapon)
    }
}

/// A gunner carrying the burst rifle (0.05 m shots) and `long`, both on the
/// one bake, with `burst` and `long` tokens registered.
fn two_rifle_scene(long: EntityTypeDescriptor) -> Scene {
    let mut scene = gunner_scene(renamed(burst_rifle(), RIFLE, 0.05));
    scene.descriptors.insert(1, long);
    let mut mount = rifle_mount();
    mount.item = LONG.to_string();
    scene.descriptors[2] = archetype(BODY, Some(vec![rifle_mount(), mount]));
    let vocabulary = postretro_foundation::attack_budget::AttackBudgetVocabulary::from_manifest(
        Some(&serde_json::json!([
            { "kind": "attackToken", "name": "burst", "budget": { "limit": 1 } },
            { "kind": "attackToken", "name": "long", "budget": { "limit": 1 } },
        ])),
        None,
    )
    .expect("a valid token registration");
    scene.reg.set_attack_budgets(Arc::new(vocabulary));
    scene
}

/// `spray` bursts the volley and hands over, once it fired, to `far`, whose
/// snipe on the long rifle is proposed while the burst runs on.
fn handover_graph() -> BehaviorGraphDescriptor {
    clip_graph(
        "spray",
        vec![
            ("spray", firing("shoot")),
            ("far", act("shoot", Some(untimed(SNIPE)))),
        ],
        vec![(
            "spray",
            vec![edge(
                "far",
                at_least(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0),
            )],
        )],
        vec![
            (VOLLEY, drawing(RIFLE, "burst")),
            (SNIPE, drawing(LONG, "long")),
        ],
    )
}

/// A one-shot long rifle at base `radius` whose shot step is `step`.
fn long_rifle(radius: f32, step: serde_json::Value) -> EntityTypeDescriptor {
    let mut descriptor = renamed(rifle(1, 0.0, 1000.0, 30), LONG, radius);
    descriptor.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": 1000, "steps": [step],
    }))
    .expect("a valid long-rifle action");
    descriptor
}

/// What a handover run left: each launch's tick and weapon, the two
/// magazines, and whether the long rifle's activation ran after tick 1.
#[derive(Debug)]
struct Handover {
    shots: Vec<(usize, String)>,
    magazines: (u32, u32),
    long_started: bool,
}

fn handover(long: EntityTypeDescriptor, world: &CollisionWorld) -> Handover {
    let mut scene = two_rifle_scene(long);
    let enemy = spawn_gunner(&mut scene, 0, &handover_graph(), "spray");
    let mut shots = Vec::new();
    let mut long_started = false;
    for tick in 0..40 {
        let result = tick_in(&mut scene, world);
        shots.extend(
            result
                .projectile_spawns
                .iter()
                .map(|spawn| (tick, spawn.descriptor_class.clone())),
        );
        if tick == 1 {
            long_started = scene
                .runtime
                .weapons
                .weapon(enemy, LONG)
                .unwrap()
                .state
                .activation_cursor()
                .is_some();
        }
    }
    Handover {
        shots,
        magazines: (
            magazine(&scene, enemy),
            scene.runtime.weapons.weapon(enemy, LONG).unwrap().magazine,
        ),
        long_started,
    }
}

// Regression: a request for another weapon decided its first shot at the
// weapon's base projectile radius, not the radius its first shot step scales
// to, so near cover the real shot met was judged by the wrong sweep. A charge,
// whose first shot is not due on the start tick, is not decided early.
#[test]
fn a_weapon_switch_decides_its_first_shot_at_the_shots_own_scaled_radius() {
    let ticks = burst_ticks();
    let burst_only: Vec<(usize, String)> = ticks
        .iter()
        .map(|&tick| (tick, RIFLE.to_string()))
        .collect();

    // Base 0.1 m clears the lip; scaled ×3 to 0.3 m, the shot is held.
    let scaled_up = long_rifle(
        0.1,
        serde_json::json!({ "kind": "shot", "scale": { "projectileRadius": 3 } }),
    );
    let run = handover(scaled_up, &under_lip());
    assert_eq!(run.shots, burst_only, "the burst runs on: {run:?}");
    assert_eq!(
        run.magazines,
        (27, 30),
        "the long attack never started: {run:?}"
    );

    // Base 0.3 m is held; scaled to 0.05 m, the shot clears.
    let scaled_down = long_rifle(
        0.3,
        serde_json::json!({ "kind": "shot", "scale": { "projectileRadius": 0.16666667 } }),
    );
    let run = handover(scaled_down, &under_lip());
    assert_eq!(
        run.shots,
        vec![
            (ticks[0], RIFLE.to_string()),
            (ticks[0] + 1, LONG.to_string())
        ],
        "the long shot launches and cancels the burst, as today: {run:?}"
    );
    assert_eq!(run.magazines, (29, 29), "{run:?}");

    // A charged long rifle whose shot would be held: no shot is due on the
    // start tick, so nothing is decided early; the burst is cancelled and the
    // charge starts, as before.
    let mut charged = renamed(rifle(1, 0.0, 1000.0, 30), LONG, 0.3);
    charged.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": 1000,
        "charge": { "minMs": 0, "fullMs": 50 },
        "steps": [{ "kind": "shot" }],
    }))
    .expect("a valid charged action");
    let run = handover(charged, &under_lip());
    assert!(run.long_started, "the charge starts on the switch: {run:?}");
    assert_eq!(
        run.shots,
        vec![(ticks[0], RIFLE.to_string())],
        "the burst is cancelled by the switch: {run:?}"
    );
    assert_eq!(
        run.magazines,
        (29, 30),
        "the burst stops; the held charge never spends: {run:?}"
    );
}

/// `engage` holding an `offense` layer: `spray` bursts the volley and, once it
/// fired, hands over to `a1`, `a2`, `a3` in turn, every 150 ms, each firing
/// the snipe with no clip moment; `a3` leads back to `a1`.
fn overcount_graph() -> BehaviorGraphDescriptor {
    let snipe = || act("shoot", Some(untimed(SNIPE)));
    let offense = BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
        initial: "spray".to_string(),
        activities: BTreeMap::from([
            ("spray".to_string(), firing("shoot")),
            ("a1".to_string(), snipe()),
            ("a2".to_string(), snipe()),
            ("a3".to_string(), snipe()),
        ]),
        transitions: BTreeMap::from([
            (
                "spray".to_string(),
                vec![edge(
                    "a1",
                    at_least(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0),
                )],
            ),
            ("a1".to_string(), vec![edge("a2", after_ms(150.0))]),
            ("a2".to_string(), vec![edge("a3", after_ms(150.0))]),
            ("a3".to_string(), vec![edge("a1", after_ms(150.0))]),
        ]),
    });
    let mut engage = resting();
    engage.layers = BTreeMap::from([("offense".to_string(), offense)]);
    clip_graph(
        "engage",
        vec![("engage", engage)],
        vec![],
        vec![
            (VOLLEY, drawing(RIFLE, "burst")),
            (SNIPE, drawing(LONG, "long")),
        ],
    )
}

// Regression: a burst shot of one attack, launched on a tick another attack's
// start was held, ended that hold, so the enclosing activity counted the
// still-held attack again at its next leaf's entry. Its own launched shot
// still ends it.
#[test]
fn another_attacks_burst_shots_leave_a_hold_running_and_its_own_shot_ends_it() {
    let ticks = burst_ticks();
    // The snipe's 0.3 m shot is held under the lip until `a3`, where the
    // path stands open; the burst's 0.05 m shots clear it throughout.
    let mut scene = two_rifle_scene(renamed(rifle(1, 0.0, 0.0, 30), LONG, 0.3));
    let enemy = spawn_gunner(&mut scene, 0, &overcount_graph(), "engage");
    let lip = under_lip();
    let open = CollisionWorld::new();
    let mut paths = Vec::new();
    let mut parent_held = Vec::new();
    let mut shots = Vec::new();
    let mut a3_entry = None;
    for tick in 0..60 {
        // Open for three ticks after `a3`'s entry, so the snipe's first chance
        // there launches.
        let world = if a3_entry.is_some_and(|entry| tick > entry && tick <= entry + 3) {
            &open
        } else {
            &lip
        };
        let result = tick_in(&mut scene, world);
        shots.extend(
            result
                .projectile_spawns
                .iter()
                .map(|spawn| (tick, spawn.descriptor_class.clone())),
        );
        paths.push(scene.state(enemy));
        parent_held.push(held(&scene, enemy, 0));
        if a3_entry.is_none() && paths[tick].ends_with("/a3") {
            a3_entry = Some(tick);
        }
    }
    let entered = |leaf: &str, from: usize| {
        from + paths[from..]
            .iter()
            .position(|path| path.ends_with(leaf))
            .unwrap_or_else(|| panic!("enters `{leaf}`: {paths:?}"))
    };
    let (a1, a2) = (entered("/a1", 0), entered("/a2", 0));
    let a3 = entered("/a3", a2);
    let a1_again = entered("/a1", a3);
    assert!(
        a1 <= ticks[1] && ticks[1] < a2 && a2 <= ticks[2],
        "fixture: burst shots land while the snipe is held in `a1` and `a2`: \
         {ticks:?}, {paths:?}"
    );
    let burst: Vec<usize> = shots
        .iter()
        .filter(|(_, weapon)| weapon == RIFLE)
        .map(|(tick, _)| *tick)
        .collect();
    assert_eq!(burst, ticks, "fixture: the burst launches every shot");
    assert_eq!(parent_held[a1], 1, "the first hold counts: {parent_held:?}");
    assert_eq!(
        parent_held[a3 - 1],
        1,
        "the burst's launched shots leave the snipe's hold running: {parent_held:?}"
    );
    assert!(
        shots.contains(&(a3 + 1, LONG.to_string())),
        "fixture: the snipe launches in `a3`, its path open: {shots:?}"
    );
    assert_eq!(
        parent_held[a1_again], 2,
        "the snipe's own shot ended the hold, so the next counts: {parent_held:?}"
    );
}

// Regression: a target aim point nearer the muzzle than the shot's radius
// swept the muzzle-to-target leg backwards, into a wall behind the gun.
#[test]
fn a_target_inside_the_shots_radius_sweeps_nothing_behind_the_muzzle() {
    // Run as a scene-free check of the gate the launch calls.
    let wall_behind = wall_world(0.3, -1.0, 3.0);
    let world = CombinedCollisionWorld::static_only(&wall_behind);
    let muzzle = Vec3::new(0.5, 1.2, 0.0);
    // The eye straight above the gun, its leg clear of the wall.
    let eye = Vec3::new(0.5, 1.8, 0.0);
    let target = Vec3::new(0.55, 1.2, 0.0);
    assert!(
        !crate::npc_launch::near_path_blocked(&world, eye, muzzle, target, 0.15),
        "nothing lies between the muzzle and a target at it"
    );
}

// Regression: a charged first shot whose launch failed to spawn was retried as
// a later burst shot, so when near cover then held it, it spent and restarted
// the attack's cooldown instead of leaving the attack unstarted.
#[test]
fn a_charged_first_shot_retried_after_a_failed_spawn_is_still_first() {
    let mut charged = rifle(1, 0.0, 300.0, 30);
    charged.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": 300,
        "charge": { "minMs": 0, "fullMs": 50 },
        "steps": [{ "kind": "shot" }],
    }))
    .expect("a valid charged action");
    let graph = volley_graph("fire", vec![("fire", firing("shoot"))], vec![]);
    let open = CollisionWorld::new();
    let release = {
        let mut scene = gunner_scene(charged.clone());
        let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
        let shots = run(&mut scene, enemy, 20, |_, _| &open);
        assert!(!shots.is_empty(), "fixture: unobstructed, the charge fires");
        shots[0].tick
    };
    assert!(
        release > 0,
        "fixture: the charge releases after its start tick"
    );

    let mut scene = gunner_scene(charged);
    let enemy = spawn_gunner(&mut scene, 0, &graph, "fire");
    let low = cover(LOW_LIP);
    let mut cooldowns = Vec::new();
    let shots = run(&mut scene, enemy, release + 10, |scene, tick| {
        if tick == release {
            // The player and the gunner: no room for the projectile.
            scene.reg.set_test_capacity_limit(2);
        } else if tick == release + 1 {
            scene.reg.set_test_capacity_limit(1024);
        }
        cooldowns.push(volley_cooldown(scene, enemy));
        // Clear on the release tick, so the launch is decided and fails to
        // spawn; held from the retry on.
        if tick <= release { &open } else { &low }
    });
    assert!(shots.is_empty(), "nothing launches: {shots:?}");
    assert_eq!(
        magazine(&scene, enemy),
        30,
        "the retried first shot spent nothing"
    );
    assert!(
        cooldowns.iter().all(|&cooldown| cooldown <= 0.0),
        "the attack's cooldown never restarts: {cooldowns:?}"
    );
    assert_eq!(held(&scene, enemy, 0), 1, "one hold, not a skipped shot");
}

// ---------------------------------------------------------------------------
// Regressions from review, round 3
// ---------------------------------------------------------------------------

/// A composite on the rifle loop whose `offense` layer is a selector picking
/// `attack`, with no clip moment.
fn selector_firing(attack: &str) -> BehaviorActivityDescriptor {
    let mut activity = act("shoot", None);
    activity.layers = BTreeMap::from([(
        "offense".to_string(),
        BehaviorLayerDescriptor::Selector(vec![BehaviorSelectorEntry::Row(BehaviorSelectorRow {
            when: None,
            motion: None,
            action: Some(untimed(attack)),
        })]),
    )]);
    activity
}

/// `engage` holding an `offense` graph of `x` and `y`, each leading to the
/// other after 150 ms.
fn two_leaf_graph(
    x: BehaviorActivityDescriptor,
    y: BehaviorActivityDescriptor,
    attacks: Vec<(&str, AttackParams)>,
) -> BehaviorGraphDescriptor {
    let offense = BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
        initial: "x".to_string(),
        activities: BTreeMap::from([("x".to_string(), x), ("y".to_string(), y)]),
        transitions: BTreeMap::from([
            ("x".to_string(), vec![edge("y", after_ms(150.0))]),
            ("y".to_string(), vec![edge("x", after_ms(150.0))]),
        ]),
    });
    let mut engage = resting();
    engage.layers = BTreeMap::from([("offense".to_string(), offense)]);
    clip_graph("engage", vec![("engage", engage)], vec![], attacks)
}

/// Per tick: the leaf path and the (parent, leaf) held counts after it.
type HeldTrace = (Vec<String>, Vec<(u32, u32)>);

/// The tick on which the path first ends in `/leaf` at or after `from`.
fn leaf_entry(paths: &[String], leaf: &str, from: usize) -> usize {
    let suffix = format!("/{leaf}");
    from + paths[from..]
        .iter()
        .position(|path| path.ends_with(&suffix))
        .unwrap_or_else(|| panic!("enters `{leaf}` after tick {from}: {paths:?}"))
}

// Regression: an attack a selector-form `offense` layer picks was not the
// path's own attack, so its launched shot never ended its hold and its next
// hold went uncounted.
#[test]
fn a_selector_offenses_hold_ends_on_its_shot_and_its_next_hold_counts() {
    let graph = two_leaf_graph(
        selector_firing(VOLLEY),
        selector_firing(VOLLEY),
        vec![(VOLLEY, rifle_attack())],
    );
    let mut scene = gunner_scene(single_rifle());
    let enemy = spawn_gunner(&mut scene, 0, &graph, "engage");
    let world = CollisionWorld::new();
    let movers = low_door();
    let (mut paths, mut counts, mut shots) = (Vec::new(), Vec::new(), Vec::new());
    for tick in 0..30 {
        // Open for one tick in `x`: the held volley fires.
        let poses = if tick == 4 {
            open_door()
        } else {
            closed_door()
        };
        let result = tick_full(&mut scene, &world, &movers, &poses, DT, |_| {});
        shots.extend(record(&scene, enemy, tick, &result));
        paths.push(scene.state(enemy));
        counts.push((held(&scene, enemy, 0), held(&scene, enemy, 1)));
    }
    let y = leaf_entry(&paths, "y", 0);
    assert!(paths[4].ends_with("/x") && y > 4, "fixture: {paths:?}");
    assert_eq!(counts[3], (1, 1), "held in `x`: {counts:?}");
    assert_eq!(ticks_of(&shots), vec![4], "fires the tick the door opens");
    assert_from_muzzle(&shots[0], "the selector's volley");
    assert_eq!(
        counts[y],
        (2, 1),
        "its shot ended the hold, so held again in `y` the parent counts 2: {counts:?}"
    );
}

/// The parent and leaf held counts of `x` holding the volley (a 0.3 m shot
/// under the lip) and `y` firing the snipe on the long rifle at `long_radius`,
/// over two visits to `x`.
fn enclosing_trace(long_radius: f32) -> HeldTrace {
    let graph = two_leaf_graph(
        firing("shoot"),
        act("shoot", Some(untimed(SNIPE))),
        vec![
            (VOLLEY, drawing(RIFLE, "burst")),
            (SNIPE, drawing(LONG, "long")),
        ],
    );
    let mut scene = two_rifle_scene(renamed(rifle(1, 0.0, 0.0, 30), LONG, long_radius));
    scene.descriptors[0] = renamed(single_rifle(), RIFLE, 0.3);
    let enemy = spawn_gunner(&mut scene, 0, &graph, "engage");
    let lip = under_lip();
    let (mut paths, mut counts) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        tick_in(&mut scene, &lip);
        paths.push(scene.state(enemy));
        counts.push((held(&scene, enemy, 0), held(&scene, enemy, 1)));
    }
    (paths, counts)
}

// Regression: NM-7 across sibling leaves holding different attacks. `x`
// holds attack A; in `y` attack C fires (its smaller shot clears) or is held
// too. C firing does not end A's hold: the parent, never entered again, keeps
// 1, and `x` entered afresh counts A once. C held is another attack's hold:
// the parent counts it (2), and A held again in `x` is a new hold (3).
#[test]
fn an_enclosing_activity_counts_each_attacks_hold_across_sibling_leaves() {
    let (paths, counts) = enclosing_trace(0.05);
    let y = leaf_entry(&paths, "y", 0);
    let x_again = leaf_entry(&paths, "x", y);
    assert_eq!(counts[y - 1], (1, 1), "A held in `x`: {counts:?}");
    assert_eq!(
        counts[y],
        (1, 0),
        "C fires in `y`, holding nothing: {counts:?}"
    );
    assert_eq!(
        counts[x_again],
        (1, 1),
        "A's hold still runs: `x` counts it afresh, the parent keeps 1: {counts:?}"
    );

    let (paths, counts) = enclosing_trace(0.3);
    let y = leaf_entry(&paths, "y", 0);
    let x_again = leaf_entry(&paths, "x", y);
    assert_eq!(counts[y - 1], (1, 1), "A held in `x`: {counts:?}");
    assert_eq!(
        counts[y],
        (2, 1),
        "C held in `y` is another hold: {counts:?}"
    );
    // NM-7: A's hold never ended (only its own shot ends it), so the parent,
    // not entered again, keeps its count; the re-entered leaf counts A afresh.
    assert_eq!(
        counts[x_again],
        (2, 1),
        "A still held after C's hold is the same hold at the parent: {counts:?}"
    );
}

// Regression: one tick could carry a single withhold, so a started charge's
// first shot held on the tick a request for another weapon was withheld lost
// one of the two.
#[test]
fn two_attacks_withheld_on_one_tick_both_count_and_neither_spends() {
    let mut charged = renamed(rifle(1, 0.0, 300.0, 30), RIFLE, 0.3);
    charged.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": 300,
        "charge": { "minMs": 0, "fullMs": 50 },
        "steps": [{ "kind": "shot" }],
    }))
    .expect("a valid charged action");
    let graph = clip_graph(
        "x",
        vec![
            ("x", firing("shoot")),
            ("y", act("shoot", Some(untimed(SNIPE)))),
        ],
        vec![],
        vec![
            (VOLLEY, drawing(RIFLE, "burst")),
            (SNIPE, drawing(LONG, "long")),
        ],
    );
    let scene_with = |charged: EntityTypeDescriptor| {
        let mut scene = two_rifle_scene(renamed(rifle(1, 0.0, 1000.0, 30), LONG, 0.3));
        scene.descriptors[0] = charged;
        let enemy = spawn_gunner(&mut scene, 0, &graph, "x");
        (scene, enemy)
    };
    let release = {
        let (mut scene, enemy) = scene_with(charged.clone());
        let open = CollisionWorld::new();
        let shots = run(&mut scene, enemy, 20, |_, _| &open);
        assert!(!shots.is_empty(), "fixture: unobstructed, the charge fires");
        shots[0].tick
    };
    assert!(
        release > 0,
        "fixture: the charge releases after its start tick"
    );

    let (mut scene, enemy) = scene_with(charged);
    let lip = under_lip();
    let shots = run(&mut scene, enemy, release + 1, |scene, tick| {
        if tick == release {
            // `y`'s snipe is requested on the charge's release tick.
            scene.enter(enemy, "y");
        }
        &lip
    });
    assert!(shots.is_empty(), "nothing launches: {shots:?}");
    assert_eq!(scene.state(enemy), "y", "fixture");
    assert_eq!(
        held(&scene, enemy, 0),
        2,
        "the snipe's withheld request and the charge's withheld first shot both count"
    );
    assert_eq!(magazine(&scene, enemy), 30, "the charge spent nothing");
    assert_eq!(
        scene.runtime.weapons.weapon(enemy, LONG).unwrap().magazine,
        30,
        "the snipe never started"
    );
}
