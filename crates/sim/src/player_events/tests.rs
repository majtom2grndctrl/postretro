// Player events end to end at the sim seam: composed descriptors → install →
// per-tick snapshot evaluation → in-tick commands on the event's player.
// See: context/lib/scripting.md §12 (Player events)

use std::collections::HashMap;

use glam::{Quat, Vec3};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{EntityId, ScriptCtx, Transform};
use postretro_foundation::{IrNode, IrValue, Seat};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PlayerEventDescriptor, PlayerEventEdge, PrimitiveDescriptor, ReactionDescriptor,
};
use serde_json::json;

use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use postretro_test_log_capture::LogCapture;

use super::{PlayerEventResidual, PlayerEventTable};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::spawner::SpawnContext;

const MAX_HEALTH: f32 = 100.0;

pub(crate) struct World {
    pub(crate) script_ctx: ScriptCtx,
    pub(crate) table: PlayerEventTable,
    pub(crate) residuals: Vec<PlayerEventResidual>,
}

impl World {
    pub(crate) fn new() -> Self {
        Self {
            script_ctx: ScriptCtx::new(),
            table: PlayerEventTable::default(),
            residuals: Vec::new(),
        }
    }

    /// A pawn bound to `seat`, or the marked local pawn when `seat` is `None`.
    pub(crate) fn spawn_player(&self, seat: Option<Seat>, health: f32) -> EntityId {
        let pawn = self.spawn_unbound(health);
        let mut registry = self.script_ctx.registry.borrow_mut();
        match seat {
            Some(seat) => registry.bind_pawn_seat(pawn, seat),
            None => registry.mark_local_player_pawn(pawn).unwrap(),
        }
        pawn
    }

    /// A player-shaped pawn no seat or local marker binds yet: a reclaim's
    /// replacement before the seat table rebinds it.
    pub(crate) fn spawn_unbound(&self, health: f32) -> EntityId {
        let mut registry = self.script_ctx.registry.borrow_mut();
        let pawn = registry
            .try_spawn(
                Transform {
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                &[],
            )
            .expect("registry has room");
        let mut component = HealthComponent::from_descriptor(&HealthDescriptor {
            max: MAX_HEALTH,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        component.current = health;
        registry.set_component(pawn, component).unwrap();
        registry
            .set_component(pawn, crate::trigger_bindings::group_tick_tests::movement())
            .unwrap();
        pawn
    }

    pub(crate) fn set_health(&self, pawn: EntityId, health: f32) {
        let mut registry = self.script_ctx.registry.borrow_mut();
        let mut component = registry
            .get_component::<HealthComponent>(pawn)
            .expect("pawn has health")
            .clone();
        component.current = health;
        registry.set_component(pawn, component).unwrap();
    }

    pub(crate) fn health(&self, pawn: EntityId) -> f32 {
        self.script_ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .expect("pawn has health")
            .current
    }

    /// Commit `reactions` and level `events` as a level install does, then
    /// bind a fresh table.
    pub(crate) fn install(
        &mut self,
        reactions: Vec<NamedReaction>,
        events: Vec<PlayerEventDescriptor>,
    ) {
        {
            let mut data = self.script_ctx.data_registry.borrow_mut();
            data.clear();
            data.set_level_reactions(reactions);
            data.set_level_player_events(events);
            data.recompose(&[]);
        }
        self.table = PlayerEventTable::build(
            &self.script_ctx,
            MoverCommandDiagnostics::default(),
            SpawnContext::default(),
            None,
        );
    }

    /// Commit mod-global and level player events, compose them for `tags`,
    /// and bind a fresh table.
    pub(crate) fn install_composed(
        &mut self,
        reactions: Vec<NamedReaction>,
        global: Vec<PlayerEventDescriptor>,
        level: Vec<PlayerEventDescriptor>,
        tags: &[String],
    ) {
        {
            let mut data = self.script_ctx.data_registry.borrow_mut();
            data.clear();
            data.replace_global_player_events(global);
            data.set_level_reactions(reactions);
            data.set_level_player_events(level);
            data.recompose(tags);
        }
        self.table = PlayerEventTable::build(
            &self.script_ctx,
            MoverCommandDiagnostics::default(),
            SpawnContext::default(),
            None,
        );
    }

    /// The reaction names this tick's fires left for the frame-end drain, in
    /// drain order, then clear them.
    pub(crate) fn take_residual_reactions(&mut self) -> Vec<String> {
        let names = self
            .residuals
            .iter()
            .flat_map(|residual| {
                self.table
                    .residual(residual.handle)
                    .expect("residual handle resolves")
                    .iter()
                    .map(|step| match step {
                        PrepartitionedReactionStep::Descriptor(name, _, _) => name.clone(),
                        PrepartitionedReactionStep::DeferredEvent(name) => format!("->{name}"),
                    })
            })
            .collect();
        self.residuals.clear();
        names
    }

    pub(crate) fn tick(&mut self) {
        self.table.run_tick(&self.script_ctx, &mut self.residuals);
    }
}

pub(crate) fn input(name: &str) -> IrNode {
    IrNode::Input {
        name: name.to_string(),
        owner: None,
    }
}

pub(crate) fn number(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

pub(crate) fn lt(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Lt {
        a: Box::new(a),
        b: Box::new(b),
    }
}

pub(crate) fn player_event(
    edge: PlayerEventEdge,
    condition: IrNode,
    fire: &[&str],
) -> PlayerEventDescriptor {
    PlayerEventDescriptor {
        edge,
        condition,
        fire: fire.iter().map(|name| name.to_string()).collect(),
        levels: Vec::new(),
        authored_index: 0,
    }
}

pub(crate) fn on_player(name: &str, primitive: &str, args: serde_json::Value) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: Some("@player".to_string()),
            kind: None,
            tag: None,
            on_complete: None,
            args,
        }),
    }
}

pub(crate) fn play_sound(name: &str, sound: &str) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "playSound".to_string(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args: json!({ "sound": sound }),
        }),
    }
}

#[test]
fn becomes_fires_for_the_crossing_player_and_damages_only_their_pawn() {
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(1)), 80.0);
    let second = world.spawn_player(Some(Seat(2)), 80.0);
    world.install(
        vec![on_player("scald", "applyDamage", json!({ "amount": 5.0 }))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            lt(input("player.health"), number(50.0)),
            &["scald"],
        )],
    );

    world.tick();
    assert_eq!(world.health(first), 80.0, "neither player crossed yet");
    assert_eq!(world.health(second), 80.0);

    world.set_health(second, 40.0);
    world.tick();
    assert_eq!(
        world.health(first),
        80.0,
        "only the crossing player is the target"
    );
    assert_eq!(
        world.health(second),
        35.0,
        "the fire damages the event's player in-tick"
    );

    world.tick();
    assert_eq!(
        world.health(second),
        35.0,
        "a held condition does not fire again"
    );
}

#[test]
fn a_non_bool_condition_is_rejected_naming_the_event_and_its_bool_sibling_installs() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 10.0);
    let capture = LogCapture::start();
    world.install(
        vec![on_player("scald", "applyDamage", json!({ "amount": 1.0 }))],
        vec![
            player_event(PlayerEventEdge::Becomes, input("player.health"), &["scald"]),
            player_event(
                PlayerEventEdge::Becomes,
                lt(input("player.health"), number(50.0)),
                &["scald"],
            ),
        ],
    );
    capture.assert_logged(
        log::Level::Error,
        "player event setupLevel().playerEvents[0]: condition must produce Bool",
    );
    world.tick();
    assert_eq!(
        world.health(pawn),
        9.0,
        "the Bool sibling installs and fires once"
    );
}

#[test]
fn a_rejection_names_the_authored_position_after_a_skipped_sibling() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 10.0);
    let capture = LogCapture::start();
    // Authored entries 1 and 3 were skipped at parse; the survivors keep
    // their authored positions, so the non-Bool one is entry 2, not 1.
    world.install(
        vec![on_player("scald", "applyDamage", json!({ "amount": 1.0 }))],
        vec![
            player_event(
                PlayerEventEdge::Becomes,
                lt(input("player.health"), number(50.0)),
                &["scald"],
            ),
            PlayerEventDescriptor {
                authored_index: 2,
                ..player_event(PlayerEventEdge::Becomes, input("player.health"), &["scald"])
            },
        ],
    );
    capture.assert_logged(
        log::Level::Error,
        "player event setupLevel().playerEvents[2]: condition must produce Bool",
    );
}

#[test]
fn one_condition_edge_and_reaction_bind_once_and_distinct_entries_fire_mod_global_first() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 10.0);
    let low = || lt(input("player.health"), number(50.0));
    let capture = LogCapture::start();
    world.install_composed(
        vec![
            play_sound("fanfare", "level_up"),
            play_sound("hiss", "scald"),
        ],
        vec![player_event(PlayerEventEdge::Becomes, low(), &["fanfare"])],
        vec![
            player_event(PlayerEventEdge::Becomes, low(), &["hiss"]),
            PlayerEventDescriptor {
                authored_index: 1,
                ..player_event(PlayerEventEdge::Becomes, low(), &["fanfare"])
            },
        ],
        &[],
    );
    let warnings: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.message.contains("is bound by both"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "one warning for the shared triple: {warnings:?}"
    );
    assert!(
        warnings[0].message.contains("ModManifest.playerEvents[0]")
            && warnings[0].message.contains("setupLevel().playerEvents[1]"),
        "the warning names both entries: {}",
        warnings[0].message
    );

    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["fanfare".to_string(), "hiss".to_string()],
        "the shared triple fires once; mod-global entries fire before the level's"
    );
}

#[test]
fn a_levels_scoped_mod_event_fires_only_in_matching_levels() {
    let low = || lt(input("player.health"), number(50.0));
    let scoped = PlayerEventDescriptor {
        levels: vec!["arena".to_string()],
        ..player_event(PlayerEventEdge::Becomes, low(), &["fanfare"])
    };
    for (tags, expected) in [
        (
            vec!["arena".to_string()],
            vec!["fanfare".to_string(), "hiss".to_string()],
        ),
        (vec!["campaign".to_string()], vec!["hiss".to_string()]),
    ] {
        let mut world = World::new();
        world.spawn_player(Some(Seat(1)), 10.0);
        world.install_composed(
            vec![
                play_sound("fanfare", "level_up"),
                play_sound("hiss", "scald"),
            ],
            vec![scoped.clone()],
            vec![player_event(PlayerEventEdge::Becomes, low(), &["hiss"])],
            &tags,
        );
        world.tick();
        assert_eq!(
            world.take_residual_reactions(),
            expected,
            "level tags {tags:?}"
        );
    }
}
