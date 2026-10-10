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
    NamedReaction, PlayerEventDescriptor, PlayerEventEdge, PrimitiveDescriptor,
    ReactionDescriptor,
};
use serde_json::json;

use super::{PlayerEventResidual, PlayerEventTable};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::spawner::SpawnContext;

const MAX_HEALTH: f32 = 100.0;

pub(super) struct World {
    pub(super) script_ctx: ScriptCtx,
    pub(super) table: PlayerEventTable,
    pub(super) residuals: Vec<PlayerEventResidual>,
}

impl World {
    pub(super) fn new() -> Self {
        Self {
            script_ctx: ScriptCtx::new(),
            table: PlayerEventTable::default(),
            residuals: Vec::new(),
        }
    }

    pub(super) fn spawn_player(&self, seat: Option<Seat>, health: f32) -> EntityId {
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
        match seat {
            Some(seat) => registry.bind_pawn_seat(pawn, seat),
            None => registry.mark_local_player_pawn(pawn).unwrap(),
        }
        pawn
    }

    pub(super) fn set_health(&self, pawn: EntityId, health: f32) {
        let mut registry = self.script_ctx.registry.borrow_mut();
        let mut component = registry
            .get_component::<HealthComponent>(pawn)
            .expect("pawn has health")
            .clone();
        component.current = health;
        registry.set_component(pawn, component).unwrap();
    }

    pub(super) fn health(&self, pawn: EntityId) -> f32 {
        self.script_ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .expect("pawn has health")
            .current
    }

    /// Commit `reactions` and level `events` as a level install does, then
    /// bind a fresh table.
    pub(super) fn install(
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

    pub(super) fn tick(&mut self) {
        self.table.run_tick(&self.script_ctx, &mut self.residuals);
    }
}

pub(super) fn input(name: &str) -> IrNode {
    IrNode::Input {
        name: name.to_string(),
        owner: None,
    }
}

pub(super) fn number(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

pub(super) fn lt(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Lt {
        a: Box::new(a),
        b: Box::new(b),
    }
}

pub(super) fn player_event(
    edge: PlayerEventEdge,
    condition: IrNode,
    fire: &[&str],
) -> PlayerEventDescriptor {
    PlayerEventDescriptor {
        edge,
        condition,
        fire: fire.iter().map(|name| name.to_string()).collect(),
        levels: Vec::new(),
    }
}

pub(super) fn on_player(name: &str, primitive: &str, args: serde_json::Value) -> NamedReaction {
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
    assert_eq!(world.health(first), 80.0, "only the crossing player is the target");
    assert_eq!(world.health(second), 35.0, "the fire damages the event's player in-tick");

    world.tick();
    assert_eq!(
        world.health(second),
        35.0,
        "a held condition does not fire again"
    );
}
