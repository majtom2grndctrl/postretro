// Trigger events in both wire forms, from parsed manifests to the fixed tick:
// a level script's volume-keyed member events (`t.on`, `{ trigger, event,
// fire }`) and the mod manifest's tag-keyed standing rules
// (`defineTriggerEvent`, `{ tag, event, fire, levels? }`). Each test parses
// manifest JS through the production drains, composes through `DataRegistry`,
// installs through `TriggerBindingTable`, and walks a pawn through the volumes.
// See: context/plans/in-progress/sdk-addressing-model/index.md — Trigger events
// and sources; research.md Ordering pins A8, A9.

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};
use postretro_entities::{
    EntityId, EntityRegistry, MoverCommand, ScriptCtx, Transform, TriggerActivation,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::drain_mod_trigger_events_js;
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use postretro_test_log_capture::{CapturedRecord, LogCapture};

use super::TriggerBindingTable;
use super::group_tick_tests::{local, movement, parse_manifest, warnings};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::spawner::SpawnContext;
use crate::trigger_commands::TriggerFireContext;
use crate::trigger_system::{
    PlayerId, TriggerDispatchInputs, TriggerEventEdge, TriggerSystem, TriggerTickInputs,
};

const DT: f32 = 1.0 / 60.0;
const OUTSIDE: Vec3 = Vec3::new(-20.0, 1.0, -20.0);

/// One fire the tick dispatched to a bound edge, with the reaction names its
/// residual carries (presentation bodies) and how many in-tick commands ran.
#[derive(Debug, PartialEq)]
struct Fired {
    trigger: EntityId,
    edge: TriggerEventEdge,
    residual: Vec<String>,
    commands: usize,
}

/// A level of named trigger volumes. A manifest names a volume `ID_<NAME>`;
/// `install` replaces that token with the volume's id, the id a trigger member
/// handle bakes when `setupLevel` runs.
struct World {
    registry: EntityRegistry,
    bridge: TriggerVolumeBridge,
    system: TriggerSystem,
    script_ctx: ScriptCtx,
    data: DataRegistry,
    bindings: TriggerBindingTable,
    names: Vec<(String, EntityId)>,
    pawn: EntityId,
}

impl World {
    fn new() -> Self {
        let mut registry = EntityRegistry::new();
        let pawn = registry
            .try_spawn(
                Transform {
                    position: OUTSIDE,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                &[],
            )
            .expect("registry has room");
        registry.set_component(pawn, movement()).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        Self {
            registry,
            bridge: TriggerVolumeBridge::new(),
            system: TriggerSystem::default(),
            script_ctx: ScriptCtx::new(),
            data: DataRegistry::new(),
            bindings: TriggerBindingTable::default(),
            names: Vec::new(),
            pawn,
        }
    }

    /// A touch volume named `name`, tagged `tags`, centred at `x` on the X
    /// axis, with an optional brush `on_fire` reaction name.
    fn volume(
        &mut self,
        name: &str,
        tags: &[&str],
        x: f32,
        fire_mode: TriggerFireMode,
        on_fire: &str,
    ) -> EntityId {
        let tags: Vec<String> = tags.iter().map(|tag| tag.to_string()).collect();
        let id = self
            .registry
            .try_spawn(Transform::default(), &tags)
            .expect("registry has room");
        self.registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    on_fire.to_string(),
                    String::new(),
                    MoverCommand::Start,
                    fire_mode,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        let center = Vec3::new(x, 0.0, 0.0);
        self.bridge.insert_for_test(
            id,
            center + Vec3::new(-1.0, 0.0, -1.0),
            center + Vec3::new(1.0, 2.0, 1.0),
        );
        self.names.push((name.to_uppercase(), id));
        id
    }

    fn substitute(&self, js: &str) -> String {
        let mut js = js.to_string();
        // Longest names first, so `ID_PLATE` never rewrites part of `ID_PLATE2`.
        let mut names = self.names.clone();
        names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        for (name, id) in names {
            js = js.replace(&format!("ID_{name}"), &id.to_raw().to_string());
        }
        js
    }

    /// Parse the mod manifest's `triggerEvents` (`mod_js`) and a level's
    /// `setupLevel` return (`level_js`) through the production drains, compose
    /// them for a level carrying `level_tags`, and install the bindings the way
    /// a level load does. Returns every record logged from parse to install.
    fn install(
        &mut self,
        mod_js: &str,
        level_js: &str,
        level_tags: &[&str],
    ) -> Vec<CapturedRecord> {
        let capture = LogCapture::start();
        let global_events = parse_mod_trigger_events(&self.substitute(mod_js));
        let level = parse_manifest(&self.substitute(level_js));
        let tags: Vec<String> = level_tags.iter().map(|tag| tag.to_string()).collect();
        self.data = DataRegistry::new();
        self.data.replace_global_trigger_events(global_events);
        self.data.populate_level_with_trigger_events(
            level.reactions,
            level.crossings,
            level.trigger_events,
            level.trigger_pools,
            &tags,
        );
        self.rebind();
        capture.records()
    }

    /// Rebuild brush and manifest bindings from the composed sets, as level
    /// install and the hot-reload rebind both do.
    fn rebind(&mut self) {
        self.bindings = TriggerBindingTable::build_with_script_ctx_and_diagnostics(
            &self.registry,
            &self.data,
            &self.script_ctx,
            MoverCommandDiagnostics::default(),
            SpawnContext::default(),
        );
        self.bindings
            .install_manifest_events(&self.registry, &self.data, &self.script_ctx);
    }

    /// Move the pawn to `position` and run one authoritative trigger tick.
    fn step_to(&mut self, position: Vec3) -> Vec<Fired> {
        let mut transform = *self.registry.get_component::<Transform>(self.pawn).unwrap();
        transform.position = position;
        self.registry.set_component(self.pawn, transform).unwrap();

        let players = [local(self.pawn)];
        let alive: HashSet<PlayerId> = players.iter().map(|player| player.id).collect();
        let bound_edges = self.bindings.bound_edges().clone();
        let bindings = &self.bindings;
        let script_ctx = &self.script_ctx;
        let pawn = self.pawn;
        let mut fired = Vec::new();
        self.system.run_authoritative_tick_with_dispatch(
            &mut self.registry,
            &self.bridge,
            TriggerTickInputs {
                players: &players,
                use_pressed: &HashMap::new(),
                tick_dt: DT,
            },
            TriggerDispatchInputs {
                alive_players: &alive,
                bound_edges: &bound_edges,
            },
            |event, occupancy, registry| {
                let execution = bindings.execute_with_script_ctx(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    script_ctx,
                    &TriggerFireContext {
                        fired_trigger: Some(event.fire.trigger),
                        activator: Some(pawn),
                        occupancy,
                    },
                );
                let commands = execution.commands.len();
                let residual = execution
                    .residual()
                    .and_then(|handle| bindings.residual(handle))
                    .map(|residual| residual_names(residual.steps()))
                    .unwrap_or_default();
                fired.push(Fired {
                    trigger: event.fire.trigger,
                    edge: event.edge,
                    residual,
                    commands,
                });
            },
        );
        fired
    }

    /// Walk into the volume centred at `x`, then back out, returning what the
    /// enter tick and the exit tick dispatched.
    fn visit(&mut self, x: f32) -> (Vec<Fired>, Vec<Fired>) {
        let entered = self.step_to(Vec3::new(x, 1.0, 0.0));
        let left = self.step_to(OUTSIDE);
        (entered, left)
    }

    fn armed(&self, id: EntityId) -> bool {
        self.registry
            .get_component::<TriggerVolumeComponent>(id)
            .unwrap()
            .armed
    }
}

fn parse_mod_trigger_events(
    mod_js: &str,
) -> Vec<postretro_scripting_core::data_descriptors::TriggerEventDescriptor> {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let value: rquickjs::Value = ctx.eval(mod_js).expect("mod manifest literal evaluates");
        let obj = rquickjs::Object::from_value(value).expect("mod manifest is an object");
        drain_mod_trigger_events_js(&obj, "default mod manifest export")
            .expect("mod triggerEvents drain")
    })
}

fn residual_names(steps: &[PrepartitionedReactionStep]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| match step {
            PrepartitionedReactionStep::Descriptor(name, _, _) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// A presentation reaction named `name`: it partitions to the residual, so the
/// residual's reaction names show which bindings ran on an edge, in order.
fn cue(name: &str) -> String {
    format!(r#"{{ name: "{name}", primitive: "playSound", args: {{ sound: "{name}" }} }}"#)
}

fn level_js(reactions: &[&str], trigger_events: &str) -> String {
    let reactions: Vec<String> = reactions.iter().map(|name| cue(name)).collect();
    format!(
        "({{ reactions: [{}], triggerEvents: [{trigger_events}] }})",
        reactions.join(", ")
    )
}

fn enter(trigger: EntityId, residual: &[&str]) -> Fired {
    Fired {
        trigger,
        edge: TriggerEventEdge::Enter,
        residual: residual.iter().map(|name| name.to_string()).collect(),
        commands: 0,
    }
}

fn warned_containing<'a>(
    records: &'a [CapturedRecord],
    needles: &[&str],
) -> Vec<&'a CapturedRecord> {
    warnings(records)
        .into_iter()
        .filter(|record| needles.iter().all(|needle| record.message.contains(needle)))
        .collect()
}

// M6 / A8 (sibling half): a trigger member's `on("enter", …)` binds its own
// volume only. A sibling volume carrying the same tag neither fires nor gains
// an edge.
#[test]
fn level_member_enter_fires_for_its_volume_only_not_a_sibling_with_the_same_tag() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    let sibling = world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        "({})",
        &level_js(
            &["reveal"],
            r#"{ trigger: ID_PLATE, event: "enter", fire: ["reveal"] }"#,
        ),
        &[],
    );
    assert!(warnings(&logs).is_empty(), "{logs:?}");

    assert_eq!(
        world.bindings.bound_edges(),
        &HashSet::from([(plate, TriggerEventEdge::Enter)]),
        "only the member's volume is bound"
    );
    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["reveal"])]);
    let (entered_sibling, left_sibling) = world.visit(10.0);
    assert!(
        entered_sibling.is_empty() && left_sibling.is_empty(),
        "the sibling {sibling} carrying the same tag does nothing"
    );
}

// T1: a mod-global `defineTriggerEvent` binds every volume carrying its tag in
// a level its `levels` selector matches, and fires on each as the retired
// level tag-keyed trigger event did. An untagged volume stays unbound.
#[test]
fn mod_global_trigger_event_binds_every_tagged_volume_in_a_matching_level() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    let sibling = world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    world.volume("other", &["other"], 20.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        r#"({ triggerEvents: [
            { tag: "plate", event: "enter", fire: ["storyBeat"], levels: ["campaign"] }
        ] })"#,
        &level_js(&["storyBeat"], ""),
        &["campaign", "hub"],
    );
    assert!(warnings(&logs).is_empty(), "{logs:?}");

    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["storyBeat"])]);
    assert_eq!(world.visit(10.0).0, vec![enter(sibling, &["storyBeat"])]);
    assert!(
        world.visit(20.0).0.is_empty(),
        "an untagged volume is unbound"
    );
}

// T3 (selector half): a `defineTriggerEvent` binds nothing in a level whose
// catalog tags its `levels` selector excludes.
#[test]
fn mod_global_trigger_event_binds_nothing_in_a_level_its_selector_excludes() {
    let mut world = World::new();
    world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    world.install(
        r#"({ triggerEvents: [
            { tag: "plate", event: "enter", fire: ["storyBeat"], levels: ["campaign"] }
        ] })"#,
        &level_js(&["storyBeat"], ""),
        &["deathmatch"],
    );

    assert!(world.bindings.bound_edges().is_empty());
    assert!(world.visit(0.0).0.is_empty());
}

// T3 (rejection half): a volume-keyed entry in `ModManifest` is rejected at
// load with a warning naming the manifest; the tag-keyed rule beside it binds.
#[test]
fn volume_keyed_trigger_event_in_the_mod_manifest_is_rejected_and_tag_rules_beside_it_bind() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        r#"({ triggerEvents: [
            { trigger: ID_PLATE, event: "enter", fire: ["member"] },
            { tag: "plate", event: "enter", fire: ["rule"] }
        ] })"#,
        &level_js(&["member", "rule"], ""),
        &[],
    );
    assert_eq!(
        warned_containing(&logs, &["mod manifest", "keyed by trigger volume"]).len(),
        1,
        "{logs:?}"
    );

    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["rule"])]);
}

// T2: a tag-keyed trigger event returned from `setupLevel` is rejected at load
// with a warning naming the level script; the volume-keyed event beside it
// installs, and the tag's sibling volume stays unbound.
#[test]
fn tag_keyed_trigger_event_from_setup_level_is_rejected_and_volume_events_beside_it_install() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        "({})",
        &level_js(
            &["member", "rule"],
            r#"{ tag: "plate", event: "enter", fire: ["rule"] },
               { trigger: ID_PLATE, event: "enter", fire: ["member"] }"#,
        ),
        &[],
    );
    assert_eq!(
        warned_containing(
            &logs,
            &["level script `setupLevel`", "keyed by tag `plate`"]
        )
        .len(),
        1,
        "{logs:?}"
    );

    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["member"])]);
    assert!(world.visit(10.0).0.is_empty());
}

// T5 (research A9): on one edge the brush `on_fire` runs first, then the
// mod-global rules, then the level member events — each in authored order,
// independent of the order the reactions were declared in.
#[test]
fn one_edge_runs_brush_then_mod_global_then_level_member_each_in_authored_order() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "brush");
    let logs = world.install(
        r#"({ triggerEvents: [
            { tag: "plate", event: "enter", fire: ["global1", "global2"] },
            { tag: "plate", event: "enter", fire: ["global3"] }
        ] })"#,
        &level_js(
            &["level2", "global2", "brush", "level1", "global3", "global1"],
            r#"{ trigger: ID_PLATE, event: "enter", fire: ["level1"] },
               { trigger: ID_PLATE, event: "enter", fire: ["level2"] }"#,
        ),
        &[],
    );
    assert!(warnings(&logs).is_empty(), "{logs:?}");

    assert_eq!(
        world.visit(0.0).0,
        vec![enter(
            plate,
            &["brush", "global1", "global2", "global3", "level1", "level2"]
        )]
    );
}

// T7: a mod-global rule and a level member event that resolve to the same
// volume, edge and reaction bind it once, with one warning naming the
// reaction and both sources. The rule's other tagged volume still binds it,
// unwarned.
#[test]
fn same_reaction_from_mod_global_and_level_member_on_one_volume_binds_once_with_one_warning() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    let sibling = world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        r#"({ triggerEvents: [{ tag: "plate", event: "enter", fire: ["reveal"] }] })"#,
        &level_js(
            &["reveal"],
            r#"{ trigger: ID_PLATE, event: "enter", fire: ["reveal"] }"#,
        ),
        &[],
    );
    let dedupe = warned_containing(
        &logs,
        &[
            "reaction `reveal`",
            "the mod manifest's `defineTriggerEvent` for tag `plate`",
            "the level script's `on`",
            "binding it once",
        ],
    );
    assert_eq!(dedupe.len(), 1, "{logs:?}");
    assert_eq!(warnings(&logs).len(), 1, "no other warning: {logs:?}");

    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["reveal"])]);
    assert_eq!(world.visit(10.0).0, vec![enter(sibling, &["reveal"])]);
}

// T8: `on.trigger.disarm()` / `on.trigger.arm()` lower to `@trigger` sequence
// steps. Bound through member events on two volumes sharing a tag, the token
// resolves to the volume that fired: entering one disarms it and leaves its
// sibling armed, and leaving it re-arms it.
#[test]
fn on_trigger_disarm_and_arm_target_the_volume_that_fired_through_member_events() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    let sibling = world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    let logs = world.install(
        "({})",
        r#"({
            reactions: [
                { name: "shut", sequence: [{ id: "@trigger", primitive: "disarmTrigger", args: {} }] },
                { name: "reopen", sequence: [{ id: "@trigger", primitive: "armTrigger", args: {} }] },
            ],
            triggerEvents: [
                { trigger: ID_PLATE, event: "enter", fire: ["shut"] },
                { trigger: ID_PLATE, event: "exit", fire: ["reopen"] },
                { trigger: ID_SIBLING, event: "enter", fire: ["shut"] },
                { trigger: ID_SIBLING, event: "exit", fire: ["reopen"] },
            ],
        })"#,
        &[],
    );
    assert!(warnings(&logs).is_empty(), "{logs:?}");

    let entered = world.step_to(Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(entered.len(), 1);
    assert_eq!((entered[0].trigger, entered[0].commands), (plate, 1));
    assert!(!world.armed(plate), "the volume that fired is disarmed");
    assert!(
        world.armed(sibling),
        "its sibling with the same tag stays armed"
    );

    let left = world.step_to(OUTSIDE);
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].trigger, left[0].edge, left[0].commands),
        (plate, TriggerEventEdge::Exit, 1)
    );
    assert!(world.armed(plate), "leaving re-arms the volume that fired");
    assert!(world.armed(sibling));
}

// T6 (research A10, sim half): a mod hot reload replaces the mod-global rules
// and recomposes the active sets; the rebind keeps a level member's
// `on("enter", …)` on its volume only. The binary's
// `rebuild_active_trigger_bindings` test drives the same path through `App`.
#[test]
fn level_member_enter_survives_a_mod_reload_recompose_for_its_volume_only() {
    let mut world = World::new();
    let plate = world.volume("plate", &["plate"], 0.0, TriggerFireMode::Multiple, "");
    world.volume("sibling", &["plate"], 10.0, TriggerFireMode::Multiple, "");
    world.install(
        r#"({ triggerEvents: [{ tag: "door", event: "enter", fire: ["reveal"] }] })"#,
        &level_js(
            &["reveal"],
            r#"{ trigger: ID_PLATE, event: "enter", fire: ["reveal"] }"#,
        ),
        &["campaign"],
    );

    world
        .data
        .replace_global_trigger_events(parse_mod_trigger_events("({ triggerEvents: [] })"));
    world.data.recompose_active_sets(&["campaign".to_string()]);
    world.rebind();

    assert_eq!(world.visit(0.0).0, vec![enter(plate, &["reveal"])]);
    assert!(world.visit(10.0).0.is_empty());
}
