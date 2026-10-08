// Spawner member steps (`s.fire()`, wire `{ id, primitive: "spawnFromSpawner" }`)
// on the named-dispatch and scheduled-landing paths: manifest JS → parsed
// descriptors → install validation → dispatch → the production spawn executor.
// The trigger-tick path is covered in `trigger_bindings/group_tick_tests.rs`.
// See: context/plans/in-progress/sdk-addressing-model/index.md — Members,
// Sequences; research.md Ordering pins A3, A4, A5.

#![cfg(test)]

use std::collections::HashSet;

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{ComponentKind, EntityId, ScriptCtx, Transform};
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::reaction_dispatch::{
    fire_named_event_with_sequences, validate_sequence_primitives,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::{CapturedRecord, LogCapture};

use super::reaction_scheduler::{ReactionScheduler, register_reaction_control_primitives};
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
use crate::scripting::reactions::registry::{
    register_npc_state_reaction_primitives, register_sequenced_spawner_primitives,
};
use crate::spawner::SpawnContext;

const COUNT: u32 = 2;
const SIBLING_COUNT: u32 = 3;

/// A `ScriptCtx` wired like `session/mod.rs` for these primitives: the
/// production sequenced spawner step, the `wait` control step on a live
/// scheduler, and the `updateNpcState` reaction handler.
struct Fixture {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    data: DataRegistry,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
    spawn_context: SpawnContext,
    /// Two resolved spawners sharing the tag `closet`.
    spawner: EntityId,
    sibling: EntityId,
}

impl Fixture {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let spawn_context = SpawnContext::default();
        spawn_context.replace_level_data(
            [("cultist".to_string(), behavior_enemy_descriptor("cultist"))]
                .into_iter()
                .collect(),
            None,
        );

        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        register_sequenced_spawner_primitives(
            &mut sequence_registry,
            ctx.clone(),
            spawn_context.clone(),
        );
        let mut reaction_registry = ReactionPrimitiveRegistry::new();
        register_npc_state_reaction_primitives(&mut reaction_registry);

        let spawner = add_spawner(&ctx, COUNT, glam::Vec3::ZERO);
        let sibling = add_spawner(&ctx, SIBLING_COUNT, glam::Vec3::new(50.0, 0.0, 0.0));
        Self {
            ctx,
            scheduler,
            data: DataRegistry::new(),
            sequence_registry,
            reaction_registry,
            system_registry: SystemReactionRegistry::new(),
            spawn_context,
            spawner,
            sibling,
        }
    }

    /// Parse `manifest_js` (a `setupLevel` return) and install its reactions
    /// through `validate_sequence_primitives`, as `setupLevel` does. Returns
    /// the installed reaction names.
    fn install(&mut self, manifest_js: &str) -> Vec<String> {
        let manifest = parse_manifest(manifest_js);
        let reactions = validate_sequence_primitives(manifest.reactions, &self.sequence_registry);
        let names = reactions.iter().map(|r| r.name.clone()).collect();
        self.ctx
            .data_registry
            .borrow_mut()
            .populate_level(reactions.clone(), Vec::new(), &[]);
        self.data.populate_level(reactions, Vec::new(), &[]);
        names
    }

    fn fire(&self, name: &str) -> Vec<CapturedRecord> {
        let capture = LogCapture::start();
        let _ = fire_named_event_with_sequences(
            name,
            &self.data,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
            None,
        );
        capture.records()
    }

    /// Advance past a 1 ms wait and run the frame-end landing drain.
    fn land(&self) -> Vec<CapturedRecord> {
        let capture = LogCapture::start();
        self.scheduler.begin_frame();
        self.scheduler.evaluate(&[]);
        self.scheduler.drain_landings(
            &self.data,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
        );
        capture.records()
    }

    /// A resident NPC with a brain, as a map-placed closet NPC would be.
    fn spawn_resident(&self) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry
            .try_spawn(Transform::default(), &["closet".to_string()])
            .unwrap();
        let mut brain = BrainComponent::from_graph(
            behavior_enemy_descriptor("cultist")
                .behavior
                .as_ref()
                .expect("fixture declares a graph"),
        );
        brain.aggro_armed = true;
        registry.set_component(id, brain).unwrap();
        id
    }

    fn npcs(&self) -> HashSet<EntityId> {
        self.ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::Brain)
            .map(|(id, _)| id)
            .collect()
    }

    /// NPCs whose feet sit nearer `spawner` than any other spawner — the
    /// spawn path fans copies out along the spawner's right axis.
    fn spawned_at(&self, spawner: EntityId, among: &HashSet<EntityId>) -> usize {
        let registry = self.ctx.registry.borrow();
        let origin = registry
            .get_component::<Transform>(spawner)
            .unwrap()
            .position;
        among
            .iter()
            .filter(|&&id| {
                let at = registry.get_component::<Transform>(id).unwrap().position;
                (at - origin).length() < 10.0
            })
            .count()
    }

    fn aggro(&self, id: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<BrainComponent>(id)
            .unwrap()
            .aggro_armed
    }
}

fn add_spawner(ctx: &ScriptCtx, count: u32, position: glam::Vec3) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
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
                archetype_name: "cultist".to_string(),
                count,
                spawned_tags: Vec::new(),
                resolved: true,
            },
        )
        .unwrap();
    id
}

fn parse_manifest(manifest_js: &str) -> LevelManifest {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let value: rquickjs::Value = ctx.eval(manifest_js).expect("manifest literal evaluates");
        LevelManifest::from_js_value(&ctx, value).expect("manifest parses")
    })
}

fn warnings(records: &[CapturedRecord]) -> Vec<&CapturedRecord> {
    records
        .iter()
        .filter(|record| record.level <= log::Level::Warn)
        .collect()
}

// M5 / A5, name-fired: `[s.fire(), s.fire()]` spawns two batches from `s`
// only; the sibling sharing `closet` spawns nothing.
#[test]
fn name_fired_spawner_member_steps_spawn_from_that_spawner_only_one_batch_per_step() {
    let mut fx = Fixture::new();
    let installed = fx.install(&format!(
        r#"({{
            reactions: [{{
                name: "release",
                sequence: [
                    {{ id: {s}, primitive: "spawnFromSpawner" }},
                    {{ id: {s}, primitive: "spawnFromSpawner" }},
                ],
            }}],
        }})"#,
        s = fx.spawner.to_raw(),
    ));
    assert_eq!(
        installed,
        vec!["release".to_string()],
        "install validation knows the id-keyed spawn step"
    );

    let logs = fx.fire("release");
    let spawned = fx.npcs();
    assert_eq!(
        spawned.len(),
        (2 * COUNT) as usize,
        "two steps, two batches"
    );
    assert_eq!(fx.spawned_at(fx.spawner, &spawned), (2 * COUNT) as usize);
    assert_eq!(
        fx.spawned_at(fx.sibling, &spawned),
        0,
        "nothing from the sibling"
    );
    assert!(warnings(&logs).is_empty(), "{logs:?}");
}

// M5, after a `wait`: the member step lands on the scheduler drain, and only
// then spawns — from that spawner only.
#[test]
fn spawner_member_step_after_a_wait_spawns_from_that_spawner_on_landing() {
    let mut fx = Fixture::new();
    fx.install(&format!(
        r#"({{
            reactions: [{{
                name: "release",
                sequence: [
                    {{ id: "@wait", primitive: "wait", args: {{ durationMs: 1 }} }},
                    {{ id: {s}, primitive: "spawnFromSpawner" }},
                ],
            }}],
        }})"#,
        s = fx.spawner.to_raw(),
    ));

    fx.fire("release");
    assert!(fx.npcs().is_empty(), "nothing spawns before the wait lands");

    let logs = fx.land();
    let spawned = fx.npcs();
    assert_eq!(spawned.len(), COUNT as usize);
    assert_eq!(fx.spawned_at(fx.spawner, &spawned), COUNT as usize);
    assert_eq!(fx.spawned_at(fx.sibling, &spawned), 0);
    assert!(warnings(&logs).is_empty(), "{logs:?}");
}

// A stale member id warn-skips, generation-checked like every member step.
#[test]
fn spawner_member_step_with_a_despawned_spawner_warn_skips() {
    let mut fx = Fixture::new();
    fx.install(&format!(
        r#"({{ reactions: [{{ name: "release", sequence: [{{ id: {s}, primitive: "spawnFromSpawner" }}] }}] }})"#,
        s = fx.spawner.to_raw(),
    ));
    fx.ctx.registry.borrow_mut().despawn(fx.spawner).unwrap();
    // A later spawn may reuse the freed slot under a new generation.
    add_spawner(&fx.ctx, COUNT, glam::Vec3::ZERO);

    let logs = fx.fire("release");
    assert!(
        fx.npcs().is_empty(),
        "a stale id never reaches a reused slot"
    );
    assert!(
        logs.iter()
            .any(|record| record.level == log::Level::Warn && record.message.contains("not found")),
        "{logs:?}"
    );
}

// Q2 (A4), named and after a wait: `[s.fire(), npcs().update(…)]` reaches
// every NPC `s` just spawned in the same drain. Spawned NPCs arrive with aggro
// armed (`enabled_on_spawn`), so the step disarms to make its reach observable;
// `aggro: true` resolves the same group at the same moment.
#[test]
fn npc_group_step_after_a_spawner_member_step_reaches_the_just_spawned_npcs() {
    for after_wait in [false, true] {
        let mut fx = Fixture::new();
        let resident = fx.spawn_resident();
        let wait = if after_wait {
            r#"{ id: "@wait", primitive: "wait", args: { durationMs: 1 } },"#
        } else {
            ""
        };
        fx.install(&format!(
            r#"({{
                reactions: [
                    {{
                        name: "release",
                        sequence: [
                            {wait}
                            {{ id: {s}, primitive: "spawnFromSpawner" }},
                            {{ primitive: "updateNpcState", kind: "npc", args: {{ aggro: false }} }},
                        ],
                    }},
                    {{ name: "control", sequence: [{{ id: {sib}, primitive: "spawnFromSpawner" }}] }},
                ],
            }})"#,
            s = fx.spawner.to_raw(),
            sib = fx.sibling.to_raw(),
        ));
        let before = fx.npcs();
        fx.fire("release");
        if after_wait {
            assert_eq!(fx.npcs(), before, "nothing before landing");
            fx.land();
        }
        let spawned: Vec<EntityId> = fx.npcs().difference(&before).copied().collect();
        assert_eq!(spawned.len(), COUNT as usize);
        assert!(
            spawned.iter().all(|&id| !fx.aggro(id)),
            "after_wait={after_wait}: every NPC the step just spawned is in the group"
        );
        assert!(!fx.aggro(resident));

        // Control: a spawn with no group step leaves its NPCs armed, so the
        // flip above came from the group step.
        let before = fx.npcs();
        fx.fire("control");
        let control: Vec<EntityId> = fx.npcs().difference(&before).copied().collect();
        assert_eq!(control.len(), SIBLING_COUNT as usize);
        assert!(control.iter().all(|&id| fx.aggro(id)));
    }
}

// `SpawnContext` authority still gates the member step: a connected client
// installs the reaction but materializes nothing.
#[test]
fn spawner_member_step_respects_runtime_spawn_authority() {
    let mut fx = Fixture::new();
    fx.install(&format!(
        r#"({{ reactions: [{{ name: "release", sequence: [{{ id: {s}, primitive: "spawnFromSpawner" }}] }}] }})"#,
        s = fx.spawner.to_raw(),
    ));
    fx.spawn_context.set_runtime_spawn_authority(false);

    fx.fire("release");
    assert!(fx.npcs().is_empty());
}
