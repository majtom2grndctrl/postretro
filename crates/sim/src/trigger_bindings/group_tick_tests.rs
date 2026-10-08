// Group commands (`npcs`/`players`, lowered to `kind` on the wire) on the
// trigger-tick path: manifest JS → parsed descriptors → install-time binding →
// a player entering the volume on tick k → bound commands applied in that tick.
// The named and scheduled paths are covered in
// `scripting_systems/group_command_tests.rs`.
// See: context/lib/scripting.md §12.

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, ScriptCtx, Transform, TriggerActivation,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, Seat, SpeedParams,
};
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_test_log_capture::{CapturedRecord, LogCapture};

use super::{BoundTriggerCommandKind, TriggerBindingTable};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::spawner::SpawnContext;
use crate::trigger_commands::TriggerFireContext;
use crate::trigger_system::{
    AuthoritativePlayer, PlayerId, TriggerDispatchInputs, TriggerSystem, TriggerTickInputs,
};

const DT: f32 = 1.0 / 60.0;
const MAX_HEALTH: f32 = 100.0;
const START_HEALTH: f32 = 50.0;
const PLATE: &str = "plate";
const INSIDE: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const OUTSIDE: Vec3 = Vec3::new(20.0, 1.0, 20.0);

/// One level: a touch plate tagged `plate`, the authoritative trigger system,
/// and the bindings the install path builds from a parsed `setupLevel` return.
struct Level {
    /// The plate volume. A manifest names it as `PLATE_ID`, which `install`
    /// replaces with this id — the id a trigger member handle bakes.
    plate: EntityId,
    registry: EntityRegistry,
    bridge: TriggerVolumeBridge,
    system: TriggerSystem,
    script_ctx: ScriptCtx,
    bindings: TriggerBindingTable,
}

/// What one fixed tick did on the trigger path.
struct TickOutcome {
    commands: Vec<BoundTriggerCommandKind>,
    residual_fired: bool,
    logs: Vec<CapturedRecord>,
}

impl Level {
    fn new() -> Self {
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let plate = spawn_volume(&mut registry, &mut bridge, PLATE, Vec3::ZERO);
        Self {
            plate,
            registry,
            bridge,
            system: TriggerSystem::default(),
            script_ctx: ScriptCtx::new(),
            bindings: TriggerBindingTable::default(),
        }
    }

    /// Parse `manifest_js` (the object a `setupLevel` returns) through the
    /// QuickJS manifest converter and install its trigger bindings the way a
    /// level load does. Returns the records logged while binding.
    fn install(&mut self, manifest_js: &str) -> Vec<CapturedRecord> {
        self.install_with_spawn_context(manifest_js, SpawnContext::default())
    }

    fn install_with_spawn_context(
        &mut self,
        manifest_js: &str,
        spawn_context: SpawnContext,
    ) -> Vec<CapturedRecord> {
        let manifest_js = manifest_js.replace("PLATE_ID", &self.plate.to_raw().to_string());
        self.install_manifest(parse_manifest(&manifest_js), spawn_context)
    }

    /// Install an already-evaluated manifest — the SDK-authored path, where
    /// `sdk_manifest` ran a real level script.
    fn install_manifest(
        &mut self,
        manifest: LevelManifest,
        spawn_context: SpawnContext,
    ) -> Vec<CapturedRecord> {
        let mut data = DataRegistry::new();
        data.populate_level_with_trigger_events(
            manifest.reactions,
            manifest.crossings,
            manifest.trigger_events,
            manifest.trigger_pools,
            &[],
        );
        let capture = LogCapture::start();
        self.bindings = TriggerBindingTable::build_with_script_ctx_and_diagnostics(
            &self.registry,
            &data,
            &self.script_ctx,
            MoverCommandDiagnostics::default(),
            spawn_context,
        );
        self.bindings
            .install_manifest_events(&self.registry, &data, &self.script_ctx);
        capture.records()
    }

    /// Run one authoritative trigger tick with `players`, applying bound
    /// commands through the live script context as the session tick does.
    fn tick(&mut self, players: &[AuthoritativePlayer]) -> TickOutcome {
        let alive: HashSet<PlayerId> = players.iter().map(|player| player.id).collect();
        let bound_edges = self.bindings.bound_edges().clone();
        let mut commands = Vec::new();
        let mut residual_fired = false;
        let bindings = &self.bindings;
        let script_ctx = &self.script_ctx;
        let capture = LogCapture::start();
        self.system.run_authoritative_tick_with_dispatch(
            &mut self.registry,
            &self.bridge,
            TriggerTickInputs {
                players,
                use_pressed: &HashMap::new(),
                tick_dt: DT,
            },
            TriggerDispatchInputs {
                alive_players: &alive,
                bound_edges: &bound_edges,
            },
            |event, occupancy, registry| {
                // The session's mapping: the firing player's pawn is the activator.
                let activator = players
                    .iter()
                    .find(|player| player.id == event.fire.player)
                    .map(|player| player.pawn);
                let execution = bindings.execute_with_script_ctx(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    script_ctx,
                    &TriggerFireContext {
                        fired_trigger: Some(event.fire.trigger),
                        activator,
                        occupancy,
                    },
                );
                commands.extend(execution.commands.iter().copied());
                residual_fired |= execution.residual().is_some();
            },
        );
        TickOutcome {
            commands,
            residual_fired,
            logs: capture.records(),
        }
    }

    fn spawn_npc(&mut self, tags: &[&str]) -> EntityId {
        let id = self.spawn_with_health(tags, OUTSIDE);
        attach_brain(&mut self.registry, id);
        id
    }

    /// A player pawn standing outside the plate. The caller binds it as the
    /// local pawn or to a remote seat.
    fn spawn_pawn(&mut self, tags: &[&str]) -> EntityId {
        let id = self.spawn_with_health(tags, OUTSIDE);
        self.registry.set_component(id, movement()).unwrap();
        id
    }

    fn spawn_with_health(&mut self, tags: &[&str], position: Vec3) -> EntityId {
        let tags: Vec<String> = tags.iter().map(|tag| tag.to_string()).collect();
        let id = self
            .registry
            .try_spawn(
                Transform {
                    position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                &tags,
            )
            .expect("registry has room");
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: MAX_HEALTH,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        health.current = START_HEALTH;
        self.registry.set_component(id, health).unwrap();
        id
    }

    fn set_health(&mut self, id: EntityId, current: f32) {
        let mut health = self
            .registry
            .get_component::<HealthComponent>(id)
            .unwrap()
            .clone();
        health.current = current;
        self.registry.set_component(id, health).unwrap();
    }

    fn move_to(&mut self, id: EntityId, position: Vec3) {
        let mut transform = *self.registry.get_component::<Transform>(id).unwrap();
        transform.position = position;
        self.registry.set_component(id, transform).unwrap();
    }

    fn health(&self, id: EntityId) -> f32 {
        self.registry
            .get_component::<HealthComponent>(id)
            .unwrap()
            .current
    }

    fn aggro(&self, id: EntityId) -> bool {
        self.registry
            .get_component::<BrainComponent>(id)
            .unwrap()
            .aggro_armed
    }

    fn armed(&self, id: EntityId) -> bool {
        self.registry
            .get_component::<TriggerVolumeComponent>(id)
            .unwrap()
            .armed
    }
}

/// Bundle `source`, a TypeScript level script importing `"postretro"`, through
/// the `scripts-build` library and run its `setupLevel` the way level load
/// does, so a test installs exactly the wire the SDK emits.
fn sdk_manifest(source: &str) -> LevelManifest {
    let dir = tempfile::tempdir().expect("script dir");
    let entry = dir.path().join("level.ts");
    std::fs::write(&entry, source).expect("level script writes");
    let bundled = postretro_script_compiler::bundle_entry(&entry).expect("level script bundles");
    let runtime = ScriptRuntime::new(
        &PrimitiveRegistry::new(),
        &ScriptRuntimeConfig::default(),
        &ScriptCtx::new(),
    )
    .expect("script runtime constructs");
    runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: bundled.into_bytes(),
            source_path: entry.to_string_lossy().into_owned(),
        },
        dir.path(),
    )
}

pub(super) fn parse_manifest(manifest_js: &str) -> LevelManifest {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let value: rquickjs::Value = ctx.eval(manifest_js).expect("manifest literal evaluates");
        LevelManifest::from_js_value(&ctx, value).expect("manifest parses")
    })
}

fn spawn_volume(
    registry: &mut EntityRegistry,
    bridge: &mut TriggerVolumeBridge,
    tag: &str,
    center: Vec3,
) -> EntityId {
    let id = registry
        .try_spawn(Transform::default(), &[tag.to_string()])
        .expect("registry has room");
    registry
        .set_component(
            id,
            TriggerVolumeComponent::new(
                TriggerActivation::Touch,
                String::new(),
                String::new(),
                String::new(),
                MoverCommand::Start,
                TriggerFireMode::Multiple,
                0.0,
                true,
            ),
        )
        .unwrap();
    bridge.insert_for_test(
        id,
        center + Vec3::new(-1.0, 0.0, -1.0),
        center + Vec3::new(1.0, 2.0, 1.0),
    );
    id
}

fn attach_brain(registry: &mut EntityRegistry, id: EntityId) {
    let graph = postretro_foundation::BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: postretro_foundation::BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                postretro_foundation::BehaviorActivityDescriptor {
                    sound: None,
                    animation: None,
                    motion: Some(postretro_foundation::MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 3.0,
    };
    registry
        .set_component(id, BrainComponent::from_graph(&graph))
        .unwrap();
}

pub(super) fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        view_feel: None,
    })
}

pub(super) fn local(pawn: EntityId) -> AuthoritativePlayer {
    AuthoritativePlayer {
        id: PlayerId::Local(pawn),
        pawn,
    }
}

pub(super) fn warnings(records: &[CapturedRecord]) -> Vec<&CapturedRecord> {
    records
        .iter()
        .filter(|record| record.level <= log::Level::Warn)
        .collect()
}

// Trigger-tick path: the npc group keeps its kind in the
// tick. A kindless tag binding would damage the tagged pawn too.
#[test]
fn trigger_tick_npc_group_damage_skips_a_player_pawn_carrying_the_tag() {
    let mut level = Level::new();
    let npc = level.spawn_npc(&["x"]);
    let pawn = level.spawn_pawn(&["x"]);
    level.registry.mark_local_player_pawn(pawn).unwrap();

    let install_logs = level.install(
        r#"({
            reactions: [{
                name: "hurt",
                sequence: [{ primitive: "applyDamage", kind: "npc", tag: "x", args: { amount: 5 } }],
            }],
            triggerEvents: [{ trigger: PLATE_ID, event: "enter", fire: ["hurt"] }],
        })"#,
    );
    assert!(
        warnings(&install_logs).is_empty(),
        "a group step binds cleanly: {install_logs:?}"
    );

    let idle = level.tick(&[local(pawn)]);
    assert!(
        idle.commands.is_empty(),
        "nothing fires before the enter edge"
    );
    assert_eq!(level.health(npc), START_HEALTH);

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(entered.commands, vec![BoundTriggerCommandKind::Damage]);
    assert_eq!(
        level.health(npc),
        START_HEALTH - 5.0,
        "the NPC takes 5 on tick k"
    );
    assert_eq!(
        level.health(pawn),
        START_HEALTH,
        "the pawn carrying `x` is a player, never an npc group member"
    );
}

// Tagless player and npc groups bind at install — as a
// primitive body and as a step before a wait — and apply on the enter tick,
// with no missing-tag warning anywhere in binding.
#[test]
fn trigger_tick_tagless_groups_bind_at_install_and_apply_on_the_enter_tick() {
    let mut level = Level::new();
    let tagged_npc = level.spawn_npc(&["closet"]);
    let untagged_npc = level.spawn_npc(&[]);
    for npc in [tagged_npc, untagged_npc] {
        let mut brain = level
            .registry
            .get_component::<BrainComponent>(npc)
            .unwrap()
            .clone();
        brain.aggro_armed = false;
        level.registry.set_component(npc, brain).unwrap();
    }
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();

    let install_logs = level.install(
        r#"({
            reactions: [
                { name: "heal", primitive: "grantHealth", kind: "player", args: { amount: 10 } },
                {
                    name: "rouse",
                    sequence: [
                        { primitive: "updateNpcState", kind: "npc", args: { aggro: true } },
                        { id: "@wait", primitive: "wait", args: { durationMs: 800 } },
                        { primitive: "grantHealth", kind: "player", args: { amount: 1 } },
                    ],
                },
            ],
            triggerEvents: [{ trigger: PLATE_ID, event: "enter", fire: ["heal", "rouse"] }],
        })"#,
    );
    assert!(
        !install_logs
            .iter()
            .any(|record| record.message.contains("no target tag")
                || record.message.contains("requires a tag or group target")),
        "a tagless group is a complete target, never a missing tag: {install_logs:?}"
    );
    assert!(
        warnings(&install_logs).is_empty(),
        "both groups bind cleanly: {install_logs:?}"
    );

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::GrantHealth,
            BoundTriggerCommandKind::UpdateNpcState,
        ],
        "both bind at install and run on tick k, in binding order"
    );
    assert_eq!(level.health(pawn), START_HEALTH + 10.0);
    assert!(
        level.aggro(tagged_npc) && level.aggro(untagged_npc),
        "a tagless npc group reaches every NPC"
    );
    assert!(
        entered.residual_fired,
        "the wait and its tail go to the frame-end residual"
    );
    assert_eq!(
        level.health(pawn),
        START_HEALTH + 10.0,
        "the post-wait group step does not run in the tick"
    );
}

// `on.activators` half: in the tick, the activator token credits only the
// pawn whose entry fired, and the player group credits every pawn exactly once.
#[test]
fn trigger_tick_activators_credit_only_the_firer_while_the_player_group_credits_each_pawn_once() {
    let mut level = Level::new();
    let firer = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(firer).unwrap();
    level.registry.bind_pawn_seat(firer, Seat(0));
    let remote = level.spawn_pawn(&[]);
    level.registry.bind_pawn_seat(remote, Seat(1));
    let npc = level.spawn_npc(&[]);

    level.install(
        r#"({
            reactions: [{
                name: "heal",
                sequence: [
                    { id: "@activators", primitive: "grantHealth", args: { amount: 5 } },
                    { primitive: "grantHealth", kind: "player", args: { amount: 10 } },
                ],
            }],
            triggerEvents: [{ trigger: PLATE_ID, event: "enter", fire: ["heal"] }],
        })"#,
    );

    level.move_to(firer, INSIDE);
    let players = [
        local(firer),
        AuthoritativePlayer {
            id: PlayerId::Remote(7),
            pawn: remote,
        },
    ];
    let entered = level.tick(&players);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::GrantHealth,
            BoundTriggerCommandKind::GrantHealth,
        ]
    );
    assert_eq!(
        level.health(firer),
        START_HEALTH + 5.0 + 10.0,
        "the firer gets the activator grant and one group grant"
    );
    assert_eq!(
        level.health(remote),
        START_HEALTH + 10.0,
        "a pawn that did not fire gets only the group grant"
    );
    assert_eq!(level.health(npc), START_HEALTH, "an NPC is no player");
}

// In a trigger-fired sequence, activator and group steps before the wait
// apply inside the trigger's tick, in authored order; the tail does not.
#[test]
fn trigger_fired_sequence_applies_activator_and_group_steps_before_the_wait_in_the_tick() {
    let mut level = Level::new();
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();
    level.set_health(pawn, 95.0);
    let npc = level.spawn_npc(&["guard"]);

    level.install(
        r#"({
            reactions: [{
                name: "ambush",
                sequence: [
                    { id: "@activators", primitive: "grantHealth", args: { amount: 10 } },
                    { primitive: "applyDamage", kind: "player", args: { amount: 20 } },
                    { primitive: "applyDamage", kind: "npc", tag: "guard", args: { amount: 5 } },
                    { id: "@wait", primitive: "wait", args: { durationMs: 800 } },
                    { primitive: "grantHealth", kind: "player", args: { amount: 50 } },
                ],
            }],
            triggerEvents: [{ trigger: PLATE_ID, event: "enter", fire: ["ambush"] }],
        })"#,
    );

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::GrantHealth,
            BoundTriggerCommandKind::Damage,
            BoundTriggerCommandKind::Damage,
        ]
    );
    // Authored order: heal clamps at 100, then the group hit lands (80).
    // The reverse order would leave 85.
    assert_eq!(level.health(pawn), 80.0);
    assert_eq!(level.health(npc), START_HEALTH - 5.0);
    assert!(entered.residual_fired, "the wait tail drains at frame end");
}

// Through the SDK-authored form: subject-token and group commands are
// unspread sequence entries, and those before the wait apply inside the
// trigger's tick in authored order — `on.trigger.disarm()` included; the tail
// waits for the frame-end drain.
#[test]
fn sdk_authored_token_and_group_entries_before_the_wait_apply_in_the_trigger_tick() {
    let mut level = Level::new();
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();
    level.set_health(pawn, 95.0);
    let npc = level.spawn_npc(&["guard"]);

    let manifest = sdk_manifest(&format!(
        r#"
        import {{ defineReaction, npcs, players, wait, type TriggerEventParams }} from "postretro";
        const ambush = defineReaction("ambush", (on: TriggerEventParams) => ({{
          sequence: [
            on.activators.grantHealth(10),
            players().damage(20),
            npcs({{ tag: "guard" }}).damage(5),
            on.trigger.disarm(),
            ...wait(800),
            players().grantHealth(50),
          ],
        }}));
        export function setupLevel() {{
          return {{
            reactions: [ambush],
            triggerEvents: [{{ trigger: {plate}, event: "enter", fire: [ambush.name] }}],
          }};
        }}
        "#,
        plate = level.plate.to_raw(),
    ));
    assert_eq!(manifest.reactions.len(), 1, "the SDK sequence installs");
    let logs = level.install_manifest(manifest, SpawnContext::default());
    assert!(warnings(&logs).is_empty(), "{logs:?}");

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::GrantHealth,
            BoundTriggerCommandKind::Damage,
            BoundTriggerCommandKind::Damage,
            BoundTriggerCommandKind::Disarm,
        ]
    );
    // Authored order: heal clamps at 100, then the group hit lands (80).
    assert_eq!(level.health(pawn), 80.0);
    assert_eq!(level.health(npc), START_HEALTH - 5.0);
    assert!(
        !level.armed(level.plate),
        "`on.trigger` disarms the volume that fired"
    );
    assert!(entered.residual_fired, "the wait tail drains at frame end");
}

// Tick half: a group step bound after a tag-targeted spawn on the same
// edge sees the NPCs that spawn produced in the same tick. The `s.fire()`
// member form is covered below (`trigger_tick_spawner_member_*`).
#[test]
fn trigger_tick_npc_group_after_a_spawn_on_the_same_edge_reaches_the_spawned_npcs() {
    let mut level = Level::new();
    const SPAWN_COUNT: u32 = 2;
    let spawner = level
        .registry
        .try_spawn(Transform::default(), &["closet".to_string()])
        .unwrap();
    level
        .registry
        .set_component(
            spawner,
            SpawnerComponent {
                archetype_name: "cultist".into(),
                count: SPAWN_COUNT,
                spawned_tags: Vec::new(),
                resolved: true,
            },
        )
        .unwrap();
    let resident = level.spawn_npc(&["closet"]);
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();

    let spawn_context = SpawnContext::default();
    spawn_context.replace_level_data(
        [(
            "cultist".to_string(),
            crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
                "cultist",
            ),
        )]
        .into_iter()
        .collect(),
        Some(postretro_foundation::NavAgentParams {
            radius: 0.4,
            height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
        }),
    );
    level.install_with_spawn_context(
        r#"({
            reactions: [
                { name: "release", primitive: "spawnFromSpawner", tag: "closet" },
                { name: "calm", primitive: "updateNpcState", kind: "npc", args: { aggro: false } },
            ],
            triggerEvents: [{ trigger: PLATE_ID, event: "enter", fire: ["release", "calm"] }],
        })"#,
        spawn_context,
    );
    let before: HashSet<EntityId> = level
        .registry
        .iter_with_kind(ComponentKind::Brain)
        .map(|(id, _)| id)
        .collect();

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::Spawn,
            BoundTriggerCommandKind::UpdateNpcState,
        ]
    );
    let spawned: Vec<EntityId> = level
        .registry
        .iter_with_kind(ComponentKind::Brain)
        .map(|(id, _)| id)
        .filter(|id| !before.contains(id))
        .collect();
    assert_eq!(spawned.len(), SPAWN_COUNT as usize);
    assert!(
        spawned.iter().all(|&id| !level.aggro(id)),
        "every NPC spawned earlier in this tick is in the npc group"
    );
    assert!(!level.aggro(resident));
}

// On the tick path. A connected client never runs this path today: its
// frame advances prediction only and `continue`s before `simulate_tick`
// (`postretro/src/main.rs`, the `is_connected_client()` branch), so no trigger
// tick fires there. This pins the role rule the bound commands carry anyway:
// with the client role, group commands apply nothing and log at debug at most,
// while member and activator steps in the same sequence still apply.
#[test]
fn bound_group_commands_skip_silently_under_the_client_role_while_member_steps_apply() {
    let mut level = Level::new();
    let npc = level.spawn_npc(&[]);
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();
    let gate = spawn_volume(
        &mut level.registry,
        &mut level.bridge,
        "gate",
        OUTSIDE * 2.0,
    );

    level.install(&format!(
        r#"({{
            reactions: [{{
                name: "mixed",
                sequence: [
                    {{ id: {gate}, primitive: "disarmTrigger", args: {{}} }},
                    {{ primitive: "applyDamage", kind: "npc", args: {{ amount: 5 }} }},
                    {{ primitive: "grantHealth", kind: "player", args: {{ amount: 10 }} }},
                    {{ id: "@activators", primitive: "grantHealth", args: {{ amount: 3 }} }},
                ],
            }}],
            triggerEvents: [{{ trigger: PLATE_ID, event: "enter", fire: ["mixed"] }}],
        }})"#,
        gate = gate.to_raw(),
    ));
    level.script_ctx.owner_slot_writes_enabled.set(false);
    assert!(level.armed(gate));

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert!(!level.armed(gate), "the member step applies");
    assert_eq!(
        level.health(npc),
        START_HEALTH,
        "the npc group applies nothing"
    );
    assert_eq!(
        level.health(pawn),
        START_HEALTH + 3.0,
        "the player group skips even the local pawn; the activator step applies"
    );
    let loud: Vec<_> = entered
        .logs
        .iter()
        .filter(|record| record.level < log::Level::Debug)
        .collect();
    assert!(loud.is_empty(), "nothing above debug: {loud:?}");

    // The same reaction on the host applies every group step.
    level.script_ctx.owner_slot_writes_enabled.set(true);
    level.move_to(pawn, OUTSIDE);
    level.tick(&[local(pawn)]);
    level.move_to(pawn, INSIDE);
    level.tick(&[local(pawn)]);
    assert_eq!(level.health(npc), START_HEALTH - 5.0);
    assert_eq!(level.health(pawn), START_HEALTH + 3.0 + 10.0 + 3.0);
}

fn cultist_spawn_context() -> SpawnContext {
    let spawn_context = SpawnContext::default();
    spawn_context.replace_level_data(
        [(
            "cultist".to_string(),
            crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
                "cultist",
            ),
        )]
        .into_iter()
        .collect(),
        None,
    );
    spawn_context
}

fn add_spawner(registry: &mut EntityRegistry, count: u32, position: Vec3) -> EntityId {
    let id = registry
        .try_spawn(
            Transform {
                position,
                ..Transform::default()
            },
            &["closet".to_string()],
        )
        .unwrap();
    registry
        .set_component(
            id,
            SpawnerComponent {
                archetype_name: "cultist".into(),
                count,
                spawned_tags: Vec::new(),
                resolved: true,
            },
        )
        .unwrap();
    id
}

fn brains(registry: &EntityRegistry) -> HashSet<EntityId> {
    registry
        .iter_with_kind(ComponentKind::Brain)
        .map(|(id, _)| id)
        .collect()
}

fn near(registry: &EntityRegistry, ids: &HashSet<EntityId>, anchor: EntityId) -> usize {
    let origin = registry
        .get_component::<Transform>(anchor)
        .unwrap()
        .position;
    ids.iter()
        .filter(|&&id| {
            (registry.get_component::<Transform>(id).unwrap().position - origin).length() < 10.0
        })
        .count()
}

// Trigger tick: `[s.fire(), s.fire()]` Enter-bound spawns two
// batches from `s` on the enter tick and nothing from a sibling sharing its
// tag. The id step binds without the "requires a tag or group target"
// warning, and nothing warns while it applies.
#[test]
fn trigger_tick_spawner_member_steps_spawn_from_that_spawner_only_one_batch_per_step() {
    const COUNT: u32 = 2;
    let mut level = Level::new();
    let spawner = add_spawner(&mut level.registry, COUNT, OUTSIDE * 3.0);
    let sibling = add_spawner(&mut level.registry, 3, OUTSIDE * -3.0);
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();

    let install_logs = level.install_with_spawn_context(
        &format!(
            r#"({{
                reactions: [{{
                    name: "release",
                    sequence: [
                        {{ id: {s}, primitive: "spawnFromSpawner" }},
                        {{ id: {s}, primitive: "spawnFromSpawner" }},
                    ],
                }}],
                triggerEvents: [{{ trigger: PLATE_ID, event: "enter", fire: ["release"] }}],
            }})"#,
            s = spawner.to_raw(),
        ),
        cultist_spawn_context(),
    );
    assert!(
        warnings(&install_logs).is_empty(),
        "the member step binds cleanly, with no fire-time tag warning: {install_logs:?}"
    );

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::Spawn,
            BoundTriggerCommandKind::Spawn
        ]
    );
    assert!(warnings(&entered.logs).is_empty(), "{:?}", entered.logs);
    assert!(!entered.residual_fired, "both steps apply in the tick");
    let spawned = brains(&level.registry);
    assert_eq!(
        spawned.len(),
        (2 * COUNT) as usize,
        "two steps, two batches"
    );
    assert_eq!(
        near(&level.registry, &spawned, spawner),
        (2 * COUNT) as usize
    );
    assert_eq!(near(&level.registry, &spawned, sibling), 0);
}

// Trigger tick: `[s.fire(), npcs().update(…)]` before any wait
// reaches every NPC `s` spawned earlier in the same tick. Spawned NPCs arrive
// aggro armed, so the step disarms to make its reach observable.
#[test]
fn trigger_tick_npc_group_step_after_a_spawner_member_step_reaches_the_just_spawned_npcs() {
    const COUNT: u32 = 2;
    let mut level = Level::new();
    let spawner = add_spawner(&mut level.registry, COUNT, OUTSIDE * 3.0);
    let pawn = level.spawn_pawn(&[]);
    level.registry.mark_local_player_pawn(pawn).unwrap();

    let install_logs = level.install_with_spawn_context(
        &format!(
            r#"({{
                reactions: [{{
                    name: "release",
                    sequence: [
                        {{ id: {s}, primitive: "spawnFromSpawner" }},
                        {{ primitive: "updateNpcState", kind: "npc", args: {{ aggro: false }} }},
                        {{ id: "@wait", primitive: "wait", args: {{ durationMs: 800 }} }},
                        {{ id: {s}, primitive: "spawnFromSpawner" }},
                    ],
                }}],
                triggerEvents: [{{ trigger: PLATE_ID, event: "enter", fire: ["release"] }}],
            }})"#,
            s = spawner.to_raw(),
        ),
        cultist_spawn_context(),
    );
    assert!(warnings(&install_logs).is_empty(), "{install_logs:?}");

    level.move_to(pawn, INSIDE);
    let entered = level.tick(&[local(pawn)]);
    assert_eq!(
        entered.commands,
        vec![
            BoundTriggerCommandKind::Spawn,
            BoundTriggerCommandKind::UpdateNpcState,
        ],
        "both pre-wait steps apply in the tick, in authored order"
    );
    assert!(entered.residual_fired, "the wait and its tail drain later");
    let spawned = brains(&level.registry);
    assert_eq!(
        spawned.len(),
        COUNT as usize,
        "the post-wait spawn has not run"
    );
    assert!(
        spawned.iter().all(|&id| !level.aggro(id)),
        "every NPC spawned earlier in this tick is in the npc group"
    );
}
