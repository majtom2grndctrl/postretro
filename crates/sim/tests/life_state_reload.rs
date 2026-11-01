// Staged descriptor commit through the next real physics posture tick.
// See: context/lib/scripting.md and context/lib/movement.md.

#![cfg(debug_assertions)]

use std::collections::BTreeSet;
use std::fs;
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use postretro_collision::EnvironmentView;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{EntityId, ScriptCtx, Transform};
use postretro_foundation::{
    LifeStatePosture, MovementState, PlayerMovementComponent, enter_life_state,
};
use postretro_physics::collision::CollisionWorld;
use postretro_physics::movement::{MovementInput, tick_with_immersion_and_armed};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{
    ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::staged_manifest::{
    StagedManifestBuildResult, StagedManifestBuildStatus,
};
use serde_json::{Value, json};

const DT: f32 = 1.0 / 60.0;
const EPS: f32 = 1.0e-5;

fn manifest(declared: bool, crouch_height: Option<f32>, forced: bool) -> Value {
    let mut movement = json!({
        "capsule": { "radius": 0.35, "halfHeight": if crouch_height.is_some() { 2.0 } else { 0.9 }, "eyeHeight": 1.1 },
        "ground": { "speed": { "walk": 7, "run": 11, "crouch": 3 }, "accel": if crouch_height.is_some() { 50 } else { 12 }, "stepHeight": 0.35, "maxSlope": 45 },
        "air": { "forwardSteer": 0.3, "accel": 2, "maxControlSpeed": 4, "bunnyHop": true, "jumps": 1, "jumpVelocity": 5, "jumpCeiling": 2 },
        "fall": { "terminalVelocity": 50 }
    });
    if let Some(height) = crouch_height {
        movement["crouch"] = json!({ "halfHeight": height, "eyeHeight": 0.3, "transitionRate": 8 });
    }
    let mut components = json!({ "movement": movement, "health": { "max": 100 } });
    if declared {
        components["lifeStates"] = json!({
            "initial": "alive",
            "states": {
                "alive": { "movement": { "posture": if forced { "crouch" } else { "stand" } } },
                "down": { "movement": { "posture": if crouch_height.is_some() { "crouch" } else { "stand" } } }
            }
        });
    }
    json!({ "name": "Reload Posture", "id": "reload-posture", "version": "1", "entities": [{ "canonicalName": "player", "components": components }] })
}

struct World {
    ctx: ScriptCtx,
    runtime: ScriptRuntime,
    root: tempfile::TempDir,
    pawn: EntityId,
    collision: CollisionWorld,
}

impl World {
    fn new(declared: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let ctx = ScriptCtx::new();
        let pawn = ctx.registry.borrow_mut().spawn(Transform {
            position: Vec3::new(0.0, 1.26, 0.0),
            ..Default::default()
        });
        let runtime = ScriptRuntime::new(
            &PrimitiveRegistry::new(),
            &ScriptRuntimeConfig {
                skip_identity_enforcement: true,
                ..Default::default()
            },
            &ctx,
        )
        .unwrap();
        let mut world = Self {
            ctx,
            runtime,
            root,
            pawn,
            collision: CollisionWorld::from_triangles_for_test(
                vec![
                    Vec3::new(-50.0, 0.0, -50.0),
                    Vec3::new(50.0, 0.0, -50.0),
                    Vec3::new(50.0, 0.0, 50.0),
                    Vec3::new(-50.0, 0.0, 50.0),
                ],
                vec![[0, 2, 1], [0, 3, 2]],
            ),
        };
        assert!(matches!(
            world.reload(&manifest(declared, None, false)),
            StagedManifestCommitOutcome::Committed { .. }
        ));
        let descriptor = world.ctx.data_registry.borrow().entities[0].clone();
        let mut movement =
            PlayerMovementComponent::from_descriptor(descriptor.movement.as_ref().unwrap());
        if let Some(block) = descriptor.life_states.as_ref() {
            let (id, profile) = block.initial_state();
            enter_life_state(&mut movement, id, profile, block.table_revision);
        }
        {
            let mut registry = world.ctx.registry.borrow_mut();
            registry.set_component(pawn, movement).unwrap();
            registry
                .set_component(
                    pawn,
                    HealthComponent::from_descriptor(descriptor.health.as_ref().unwrap()),
                )
                .unwrap();
            registry
                .set_component(
                    pawn,
                    DescriptorProvenance {
                        canonical_name: "player".into(),
                        owned_components: BTreeSet::from([
                            DescriptorComponentKind::Movement,
                            DescriptorComponentKind::Health,
                        ]),
                        map_overrides: BTreeSet::new(),
                        spawn_path: DescriptorSpawnPath::PlayerSpawn,
                    },
                )
                .unwrap();
        }
        for _ in 0..10 {
            world.tick();
        }
        assert!(world.movement().is_grounded());
        world
    }

    fn staged(&mut self, manifest: &Value) -> StagedManifestBuildResult {
        fs::write(
            self.root.path().join("start-script.js"),
            format!("globalThis.__postretroModManifest = {};", manifest),
        )
        .unwrap();
        let generation = self
            .runtime
            .enqueue_staged_manifest_build(self.root.path())
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for result in self.runtime.poll_staged_manifest_builds() {
                if result.generation == generation {
                    assert!(
                        matches!(result.status, StagedManifestBuildStatus::Built(_)),
                        "fixture must parse: {result:?}"
                    );
                    return result;
                }
            }
            assert!(Instant::now() < deadline, "staged VM worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reload(&mut self, manifest: &Value) -> StagedManifestCommitOutcome {
        let result = self.staged(manifest);
        self.runtime.commit_staged_manifest_result(
            &result,
            &self.ctx,
            &SequencedPrimitiveRegistry::new(),
        )
    }

    fn movement(&self) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(self.pawn)
            .unwrap()
            .clone()
    }

    fn tick(&mut self) {
        let mut movement = self.movement();
        let mut transform = *self
            .ctx
            .registry
            .borrow()
            .get_component::<Transform>(self.pawn)
            .unwrap();
        transform.position = tick_with_immersion_and_armed(
            &mut movement,
            &MovementInput {
                wish_dir: Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: 0.0,
                facing_pitch: 0.0,
                use_pressed: false,
                drop_pressed: false,
            },
            &self.collision,
            EnvironmentView::level_only(-20.0),
            DT,
            transform.position,
            |_| false,
        )
        .0;
        let mut registry = self.ctx.registry.borrow_mut();
        registry.set_component(self.pawn, movement).unwrap();
        registry.set_component(self.pawn, transform).unwrap();
    }

    fn enter(&mut self, name: &str) {
        let data = self.ctx.data_registry.borrow();
        let block = data.entities[0].life_states.as_ref().unwrap();
        let (id, profile) = block.state(name).unwrap();
        let mut movement = self.movement();
        enter_life_state(&mut movement, id, profile, block.table_revision);
        self.ctx
            .registry
            .borrow_mut()
            .set_component(self.pawn, movement)
            .unwrap();
    }
}

fn assert_retained_tuning(before: &PlayerMovementComponent, after: &PlayerMovementComponent) {
    assert!((after.standing_half_height - before.standing_half_height).abs() < EPS);
    assert!((after.standing_eye_height - before.standing_eye_height).abs() < EPS);
    assert!((after.capsule.radius - before.capsule.radius).abs() < EPS);
    assert!((after.ground_params.accel - before.ground_params.accel).abs() < EPS);
    assert_eq!(after.velocity, before.velocity);
    assert_eq!(after.knockback_velocity, before.knockback_velocity);
    assert_eq!(after.push_velocity, before.push_velocity);
    assert_eq!(after.ground, before.ground);
    assert_eq!(after.teleport, before.teleport);
}

fn assert_next_tick_crouches(world: &mut World, before: &PlayerMovementComponent) {
    assert!(world.movement().crouch.is_some());
    world.tick();
    let after = world.movement();
    assert!(matches!(
        after.movement_state,
        MovementState::Crouching { .. }
    ));
    assert!((after.capsule.half_height - 0.4).abs() < EPS);
    assert!(after.capsule.eye_height < before.capsule.eye_height - EPS);
    assert!(
        after.capsule.eye_height > 0.3,
        "eye eases instead of snapping"
    );
}

// Regression: KeepOld retained crouch=None, so a newly stamped forced posture
// could never change the capsule, state or eye on any subsequent physics tick.
#[test]
fn life_state_reload_kept_movement_installs_safe_crouch_and_preserves_reload_continuity() {
    let mut world = World::new(true);
    let before = world.movement();
    assert!(before.crouch.is_none());
    assert!(matches!(
        world.reload(&manifest(true, Some(0.4), true)),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    assert_retained_tuning(&before, &world.movement());
    assert_eq!(
        world.movement().life_state_elapsed_ticks,
        before.life_state_elapsed_ticks
    );
    assert_next_tick_crouches(&mut world, &before);
    let crouched = world.movement();

    // A repeated rejected full movement replacement must retain the support and
    // current crouch/eye integration, including elapsed time for the kept name.
    assert!(matches!(
        world.reload(&manifest(true, Some(0.6), true)),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    let repeated = world.movement();
    assert_eq!(repeated.crouch, crouched.crouch);
    assert_eq!(repeated.movement_state, crouched.movement_state);
    assert!((repeated.capsule.eye_height - crouched.capsule.eye_height).abs() < EPS);
    assert_eq!(
        repeated.life_state_elapsed_ticks,
        crouched.life_state_elapsed_ticks
    );

    assert!(matches!(
        world.reload(&manifest(true, Some(0.4), false)),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    assert_eq!(world.movement().life_posture, LifeStatePosture::Stand);
    world.tick();
    let standing = world.movement();
    assert!(matches!(standing.movement_state, MovementState::Normal));
    assert!((standing.capsule.half_height - before.standing_half_height).abs() < EPS);
    assert!(standing.capsule.eye_height > crouched.capsule.eye_height);
}

#[test]
fn life_state_reload_skipped_movement_adds_initial_forced_crouch_before_next_physics_tick() {
    let mut world = World::new(false);
    let before = world.movement();
    // Reproduce the planner's missing-previous-snapshot lane with a live pawn
    // whose provenance still identifies the newly declared descriptor.
    world
        .ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(Vec::new());
    assert!(matches!(
        world.reload(&manifest(true, Some(0.4), true)),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    assert_retained_tuning(&before, &world.movement());
    assert_eq!(world.movement().life_state_elapsed_ticks, 0);
    assert_eq!(world.movement().life_posture, LifeStatePosture::Crouch);
    assert_next_tick_crouches(&mut world, &before);
}

#[test]
fn life_state_reload_prepares_retained_support_for_future_crouch_state_entry() {
    let mut world = World::new(true);
    let before = world.movement();
    assert!(matches!(
        world.reload(&manifest(true, Some(0.4), false)),
        StagedManifestCommitOutcome::Committed { .. }
    ));
    assert_eq!(world.movement().life_posture, LifeStatePosture::Stand);
    assert_retained_tuning(&before, &world.movement());
    world.tick();
    assert!(matches!(
        world.movement().movement_state,
        MovementState::Normal
    ));
    world.enter("down");
    assert_next_tick_crouches(&mut world, &before);
}

#[test]
fn life_state_reload_unsafe_retained_crouch_support_rejects_before_any_commit_mutation() {
    let mut world = World::new(true);
    let before = world.movement();
    let descriptors = world.ctx.data_registry.borrow().entities.clone();
    let names = world.ctx.data_registry.borrow().life_state_names.clone();
    let revision = world.ctx.data_registry.borrow().life_state_table_revision;
    let generation = world.ctx.data_registry.borrow().entity_types_generation();
    let committed_generation = world.runtime.committed_staged_manifest_generation();
    let slots = world.ctx.slot_table.borrow().len();
    let mut incoming = manifest(true, Some(1.5), true);
    incoming["entities"][0]["components"]["health"]["max"] = json!(200);
    incoming["entities"][0]["components"]["mesh"] = json!({ "model": "models/new.gltf" });
    let result = world.staged(&incoming);
    let outcome = world.runtime.commit_staged_manifest_result(
        &result,
        &world.ctx,
        &SequencedPrimitiveRegistry::new(),
    );
    let StagedManifestCommitOutcome::Rejected { reason, .. } = outcome else {
        panic!("oversized retained crouch must reject: {outcome:?}");
    };
    assert!(
        reason.contains("entities[0].components.lifeStates.states.alive.movement.posture"),
        "{reason}"
    );
    assert!(reason.contains("retained standing half-height"));
    assert!(reason.contains("reload the level"));
    assert_eq!(world.movement(), before);
    assert!(
        (world
            .ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(world.pawn)
            .unwrap()
            .max
            - 100.0)
            .abs()
            < EPS
    );
    let data = world.ctx.data_registry.borrow();
    assert_eq!(data.entities, descriptors);
    assert_eq!(data.life_state_names, names);
    assert_eq!(data.life_state_table_revision, revision);
    assert_eq!(data.entity_types_generation(), generation);
    drop(data);
    assert_eq!(world.ctx.slot_table.borrow().len(), slots);
    assert_eq!(
        world.runtime.committed_staged_manifest_generation(),
        committed_generation
    );
    assert!(!world.runtime.install_deferred_mesh_descriptors(&world.ctx));
}
