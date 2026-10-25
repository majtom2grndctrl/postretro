// Player spawn points on real compiled dev levels: `prl-build` compiles each
// map, level install materializes the first player under the dev mod's real
// `player` descriptor, and the movement tick runs against the level's static
// collision. A spawn is a place, not a pawn, and the pawn stands on its floor.
// See: context/lib/entity_model.md §7b (Player spawn points)

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use glam::{Vec2, Vec3};
use postretro_entities::{ComponentKind, EntityId, ScriptCtx, Transform};
use postretro_foundation::PlayerMovementComponent;
use postretro_level_loader::LevelWorld;
use postretro_physics::collision::{CollisionCapsule, OVERLAP_TOLERANCE};
use postretro_physics::movement::{MovementInput, tick_with_immersion_and_armed};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting::builtins::PlayerSpawned;
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};
use crate::startup::teleporters_surface_tests::{run_prl_build_on, workspace};

const DT: f32 = 1.0 / 60.0;

/// Compile `content/dev/maps/<map>.map` once per test process and load a
/// private copy. The level's data script is dropped: spawning never reads it.
fn compile(map: &'static str, out: &Path) -> LevelWorld {
    static COMPILED: OnceLock<Mutex<HashMap<&'static str, Vec<u8>>>> = OnceLock::new();
    let bytes = COMPILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(map)
        .or_insert_with(|| {
            run_prl_build_on(
                &workspace().join(format!("content/dev/maps/{map}.map")),
                &[],
            )
        })
        .clone();
    let prl = out.join(format!("{map}.prl"));
    std::fs::write(&prl, bytes).expect("write the compiled PRL");
    let mut world = postretro_level_loader::load_prl(prl.to_str().unwrap()).expect("PRL loads");
    world.data_script = None;
    world
}

/// One single-player install of a compiled level under the dev mod.
struct Installed {
    app: crate::App,
    ctx: ScriptCtx,
    first_player: Option<PlayerSpawned>,
    spawn_count: usize,
}

impl Installed {
    fn new(world: LevelWorld) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        let mut primitives = PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
        initialize_dev_mod(&mut app, &primitives);
        // Level gravity, as level load sets it; no navmesh is needed to spawn.
        let _ = crate::startup::lifecycle::install_world_gravity_and_nav(&world, &ctx);
        app.collision_world =
            crate::collision::CollisionWorld::from_level(&world).expect("static collision builds");
        let content_root = workspace().join("content/dev");
        let mut first_player = None;
        let products = {
            let session = app.session.as_mut().unwrap();
            let handles = WorldInstallHandles {
                world: &world,
                script_ctx: &ctx,
                command_diagnostics: session.scripting.command_diagnostics.clone(),
                mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                spawn_context: session.scripting.spawn_context.clone(),
                content_root: &content_root,
                active_level_tags: &[],
                nav_graph: None,
                fog_volume_bridge: &mut session.fog_volume_bridge,
                trigger_volume_bridge: &mut session.trigger_volume_bridge,
                classname_dispatch: &session.classname_dispatch,
                script_runtime: &session.scripting.script_runtime,
                sequence_registry: &session.scripting.sequence_registry,
                reaction_registry: &session.scripting.reaction_registry,
                system_registry: &session.scripting.system_registry,
                modal_stack: &mut session.modal_stack,
                progress_tracker: &mut session.progress_tracker,
                crossing_detector: &mut session.crossing_detector,
                slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
                impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
                suppress_host_replicated: false,
                suppress_boot_pawn: false,
                local_carried_loadout: None,
            };
            install_world_cpu(
                handles,
                &mut StartupTimings::new(),
                |_models, _clips| {
                    crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
                },
                |first| first_player = first,
            )
        };
        Self {
            app,
            ctx,
            first_player,
            spawn_count: products.spawn_points.len(),
        }
    }

    fn players(&self) -> Vec<EntityId> {
        self.ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::PlayerMovement)
            .map(|(id, _)| id)
            .collect()
    }
}

// A map with two `player_spawn`s and one player gets one pawn, at the first.
#[test]
fn two_spawns_and_one_player_yield_one_player_pawn() {
    let out = tempfile::tempdir().unwrap();
    let level = Installed::new(compile("coop-presentation-test", out.path()));
    assert_eq!(level.spawn_count, 2, "the fixture map has two spawns");
    let spawned = level.first_player.expect("the first player spawns");
    assert_eq!(spawned.placement, 0);
    assert_eq!(
        level.players(),
        vec![spawned.pawn],
        "exactly one player pawn"
    );
    assert_eq!(
        level.ctx.registry.borrow().local_player_pawn(),
        Some(spawned.pawn)
    );
}

// The spawned pawn never starts inside the floor. A box resting on the floor
// (24 units under the origin) starts the pawn standing, grounded on its first
// tick and still there later; a box above the floor drops the pawn onto it.
// The raised case lifts a small map's spawns rather than compiling a large map
// whose box happens to float (testing_guide.md, slow maps).
#[test]
fn spawned_pawn_rests_on_the_floor_not_in_it() {
    const RAISE: f32 = 0.8;
    let out = tempfile::tempdir().unwrap();
    for (map, raise) in [
        ("trap-volley", 0.0),
        ("coop-presentation-test", 0.0),
        ("coop-presentation-test", RAISE),
    ] {
        let box_on_floor = raise == 0.0;
        let mut world = compile(map, out.path());
        for record in &mut world.map_entities {
            if record.classname == "player_spawn" {
                record.origin[1] += raise;
            }
        }
        let level = Installed::new(world);
        let pawn = level.first_player.expect("the first player spawns").pawn;
        let (mut position, mut movement) = {
            let registry = level.ctx.registry.borrow();
            (
                registry.get_component::<Transform>(pawn).unwrap().position,
                registry
                    .get_component::<PlayerMovementComponent>(pawn)
                    .unwrap()
                    .clone(),
            )
        };
        let capsule = CollisionCapsule::new(movement.capsule.radius, movement.capsule.half_height);
        let collision = &level.app.collision_world;
        let overlap = |at: Vec3| collision.overlap_depth_for_test(at, capsule);
        assert!(
            overlap(position) <= OVERLAP_TOLERANCE,
            "{map}: the pawn spawns clear of the floor, overlap {}",
            overlap(position)
        );
        let spawned_at = position;
        let input = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            use_pressed: false,
            drop_pressed: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
        };
        let environment = postretro_collision::EnvironmentView::level_only(level.ctx.gravity.get());
        for tick in 1..=90 {
            position = tick_with_immersion_and_armed(
                &mut movement,
                &input,
                collision,
                environment,
                DT,
                position,
                |_| true,
            )
            .0;
            assert!(
                overlap(position) <= OVERLAP_TOLERANCE,
                "{map}: tick {tick} ends clear of the floor at {position}, overlap {}",
                overlap(position)
            );
            if box_on_floor && matches!(tick, 1 | 5) {
                assert!(movement.is_grounded(), "{map}: grounded at tick {tick}");
                assert!(
                    (position.y - spawned_at.y).abs() < 0.05,
                    "{map}: standing where it spawned at tick {tick}: {position} vs {spawned_at}"
                );
            }
        }
        assert!(movement.is_grounded(), "{map}: resting on the floor");
        if box_on_floor {
            assert!(
                (position.y - spawned_at.y).abs() < 0.05,
                "{map}: still standing where it spawned: {position} vs {spawned_at}"
            );
        } else {
            assert!(
                (position.y - (spawned_at.y - raise)).abs() < 0.05,
                "{map}: a raised box drops the pawn onto the floor beneath it: \
                 {position} vs {spawned_at} less {raise} m"
            );
        }
    }
}

// The real player's hitbox covers its standing body. The pawn's transform is
// its capsule centre, so the authored box must reach the floor: the dev
// pistol's hitscan aimed at the shins or knees of a pawn standing on a
// compiled floor strikes it, and one aimed just over its head does not.
#[test]
fn hitscan_strikes_the_standing_players_legs_and_passes_over_its_head() {
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_sim::weapon::{FireButtonState, resolve_test_client_shot};

    let out = tempfile::tempdir().unwrap();
    let level = Installed::new(compile("coop-presentation-test", out.path()));
    let pawn = level.first_player.expect("the first player spawns").pawn;
    let pistol = level
        .ctx
        .data_registry
        .borrow()
        .entities
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("reference_pistol"))
        .and_then(|descriptor| descriptor.weapon.clone())
        .expect("the dev mod defines the reference pistol");
    let registry = level.ctx.registry.borrow();
    let position = registry.get_component::<Transform>(pawn).unwrap().position;
    let capsule = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .capsule
        .clone();
    // The spawn surface test proves this pawn stands on the floor where it
    // spawned, so its capsule's bottom is the floor.
    let floor = position.y - (capsule.half_height + capsule.radius);
    let head = position.y + capsule.half_height + capsule.radius;
    let store = &level.app.session.as_ref().unwrap().hit_zone_store;
    // No world geometry: only the entity query decides what the ray strikes.
    let collision = crate::collision::CollisionWorld::new();
    let struck = |height: f32| {
        let aim = Vec3::new(position.x, height, position.z);
        let mut weapon = WeaponComponent::from_descriptor(&pistol);
        resolve_test_client_shot(
            None,
            &mut weapon,
            "reference_pistol",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            aim + Vec3::new(0.0, 0.0, 2.0),
            Vec3::NEG_Z,
            &pistol.placement.clone().unwrap_or_default(),
            pistol.muzzle_offset.map(Vec3::from),
            1,
            &[0.0],
            &[],
            &collision,
            &registry,
            store,
            0.0,
            0.0,
        )
        .expect("the dev pistol fires")
        .hits
        .into_iter()
        .find(|hit| hit.target == pawn)
    };
    let hitbox = registry
        .get_component::<postretro_entities::components::health::HealthComponent>(pawn)
        .unwrap()
        .hitbox;
    for (part, height) in [("shins", floor + 0.3), ("knees", floor + 0.5)] {
        let hit = struck(height).unwrap_or_else(|| {
            panic!(
                "a shot at the {part} ({height} m, floor {floor} m, centre {}) strikes \
                 the player; hitbox {hitbox:?}",
                position.y
            )
        });
        assert!(hit.zone.is_none(), "the authored body box supplies the hit");
        assert!((hit.point.y - height).abs() < 1e-4);
    }
    assert!(
        struck(head + 0.3).is_none(),
        "a shot 0.3 m over the head ({} m, centre {}) passes over the player; hitbox {hitbox:?}",
        head + 0.3,
        position.y
    );
}
