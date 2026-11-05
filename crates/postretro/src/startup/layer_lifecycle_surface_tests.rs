// Defense layers across the production lifecycle: a level unload harvests
// each seat's layer values, both spawn paths restore them against the
// capacity the seat's per-owner values give once the seat binds, a hot reload
// keeps them by name, and post-tick hits stamp the tick they follow.
// See: context/lib/scripting.md §11 (Defense layers) · plans E16 §Pinned orderings

use std::collections::{BTreeSet, HashMap};

use glam::Vec3;
use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::{
    EntityId, ReplicationScope, ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    SlotValue,
};
use postretro_foundation::defense::{DefenseVocabulary, HandleRef};
use postretro_foundation::{
    AuthoredHandle, DamagePayload, DefenseLayerEntry, IrNode, IrValue, NumberOrIr, Seat,
};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts_with_carried_loadout;
use postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_sim::sim::defense_parameters::DefenseParameters;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting::primitives::register_all;
use crate::startup::lifecycle::tests::{script_ctx, test_app};

const CAPACITY_SLOT: &str = "e16.shieldCapacity";

struct World {
    app: crate::App,
    ctx: ScriptCtx,
    player: String,
}

fn placement(x: f32) -> MapEntity {
    MapEntity {
        map_ordinal: None,
        classname: "player_spawn".into(),
        origin: Vec3::X * x,
        angles: Vec3::ZERO,
        key_values: HashMap::new(),
        tags: Vec::new(),
    }
}

fn shield_entry(capacity: NumberOrIr) -> DefenseLayerEntry {
    DefenseLayerEntry {
        layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
            name: "shield".to_string(),
        }),
        capacity,
        recharge_delay_ms: Some(NumberOrIr::Literal(3_000.0)),
        recharge_rate: Some(NumberOrIr::Literal(60.0)),
        resist: Default::default(),
    }
}

/// `read(e16.shieldCapacity).plus(50)` over a per-owner slot.
fn per_owner_capacity() -> NumberOrIr {
    NumberOrIr::Ir(IrNode::Add {
        a: Box::new(IrNode::Input {
            name: CAPACITY_SLOT.to_string(),
            owner: None,
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(50.0),
        }),
    })
}

/// The dev mod with a `shield` vocabulary, a per-owner capacity slot at 100
/// for seats 0 and 1, and the dev player descriptor carrying the shield.
fn world() -> World {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);
    ctx.data_registry.borrow_mut().recompose(&[]);
    ctx.registry.borrow_mut().clear_for_level_unload();
    postretro_scripting_core::defense_vocabulary::commit_defense_vocabulary(
        &ctx,
        DefenseVocabulary {
            layers: vec!["shield".to_string()],
            ..Default::default()
        },
    );
    {
        let mut slots = ctx.slot_table.borrow_mut();
        let mut record = SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(0.0)),
            range: None,
            persist: true,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: true,
            accumulate: None,
        });
        for seat in [Seat(0), Seat(1)] {
            record.set_per_seat_value(seat, SlotValue::Number(100.0));
        }
        slots.insert(CAPACITY_SLOT.into(), record).unwrap();
    }
    let player = {
        let mut data = ctx.data_registry.borrow_mut();
        let mut player = data
            .entities
            .iter()
            .find(|descriptor| descriptor.movement.is_some() && descriptor.health.is_some())
            .cloned()
            .expect("the dev mod declares a player descriptor");
        let health = player.health.as_mut().unwrap();
        health.max = 100.0;
        health.layers = vec![shield_entry(per_owner_capacity())];
        let name = player.canonical_name.clone().unwrap();
        data.upsert_entity_type(player);
        name
    };
    DefenseParameters::install(&ctx);
    World { app, ctx, player }
}

impl World {
    /// The host pawn by player start, the joiner by net slot, each with its
    /// seat's carried loadout, then bound the way the seat table binds.
    fn materialize(&mut self) -> (EntityId, EntityId) {
        let descriptors = self.ctx.data_registry.borrow().entities.clone();
        let session = self.app.session.as_mut().unwrap();
        let seats = session
            .seat_table
            .get_or_insert_with(crate::netcode::SeatTable::local_only);
        if seats.seat_for_client(41).is_none() {
            assert_eq!(
                seats.admit_or_reclaim(41, None, false).unwrap().seat,
                Seat(1)
            );
        }
        let mut registry = self.ctx.registry.borrow_mut();
        let host = spawn_from_player_starts_with_carried_loadout(
            &[placement(0.0)],
            &descriptors,
            &mut registry,
            None,
            seats.carried_state(Seat(0)),
        )
        .unwrap()
        .pawn;
        let other = spawn_net_slot_pawn_with_carried_loadout(
            &placement(50.0),
            &descriptors,
            &mut registry,
            None,
            seats.carried_state(Seat(1)),
        )
        .unwrap();
        seats.bind_pawn(&mut registry, Seat(0), host);
        seats.bind_pawn(&mut registry, Seat(1), other);
        (host, other)
    }

    fn shield(&self, pawn: EntityId) -> (f32, f32) {
        let registry = self.ctx.registry.borrow();
        let layer = &registry
            .get_component::<DefenseLayersComponent>(pawn)
            .unwrap()
            .layers[0];
        (layer.value, layer.capacity)
    }

    fn health(&self, pawn: EntityId) -> f32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current
    }

    fn hit(&self, pawn: EntityId, amount: f32, producer: DamageProducer) {
        apply_damage_with_context(
            &mut self.ctx.registry.borrow_mut(),
            pawn,
            &DamagePayload {
                amount,
                ..Default::default()
            },
            DamageContext::new("e16.lifecycle", producer),
        );
    }

    /// One frame-loop tick start: `begin_tick`, then the layer step.
    fn tick(&self) {
        let _pending = postretro_sim::sim::begin_tick(&self.ctx.registry, Some(&self.ctx));
        postretro_sim::sim::defense_layers::recharge_defense_layers(
            &mut self.ctx.registry.borrow_mut(),
        );
    }

    fn set_capacity(&self, seat: Seat, value: f32) {
        self.ctx
            .slot_table
            .borrow_mut()
            .get_mut(CAPACITY_SLOT)
            .unwrap()
            .set_per_seat_value(seat, SlotValue::Number(value));
    }
}

fn approx(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.0e-3,
        "expected {expected}, got {actual}"
    );
}

// L5 / R-P6: carried layers clamp against the per-owner capacity in place
// once the seat binds; a seat carrying health 0 arrives full.
#[test]
fn carried_layer_clamps_against_per_owner_capacity() {
    let mut w = world();
    let (host, other) = w.materialize();
    assert_eq!(
        w.shield(host),
        (150.0, 150.0),
        "spawn fills at the seat's capacity"
    );
    w.hit(host, 30.0, DamageProducer::InTick);
    w.hit(other, 400.0, DamageProducer::InTick);
    assert_eq!(w.health(other), 0.0);

    w.app.unload_level();
    let (host, other) = w.materialize();
    assert_eq!(w.shield(host), (120.0, 150.0));
    assert_eq!(w.health(other), 100.0, "health 0 restores nothing");
    assert_eq!(w.shield(other), (150.0, 150.0));
}

// L1: across a level change, restart or rejoin the carry clamps; a hot
// reload keeps the value by name.
#[test]
fn layer_values_carry_and_hot_reload_by_name() {
    let mut w = world();
    let (host, _) = w.materialize();
    w.hit(host, 30.0, DamageProducer::InTick);
    w.app.unload_level();
    // Between levels the seat's capacity drops to 100: the carry clamps.
    w.set_capacity(Seat(0), 50.0);
    let (host, other) = w.materialize();
    assert_eq!(w.shield(host), (100.0, 100.0));
    assert_eq!(w.shield(other), (150.0, 150.0));

    // A hot reload reorders nothing here but rebuilds the layer list; the
    // value and stamp stay by name.
    w.hit(host, 40.0, DamageProducer::InTick);
    let stamp = w
        .ctx
        .registry
        .borrow()
        .get_component::<DefenseLayersComponent>(host)
        .unwrap()
        .last_hit_tick;
    let old = w.ctx.data_registry.borrow().entities.clone();
    let mut new = old.clone();
    let edited = new
        .iter_mut()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(w.player.as_str()))
        .unwrap();
    edited.health.as_mut().unwrap().layers = vec![shield_entry(per_owner_capacity())];
    edited.health.as_mut().unwrap().max = 120.0;
    let plan = postretro_scripting_core::refresh_plan::plan_descriptor_refresh(
        &old,
        &new,
        &w.ctx.registry.borrow(),
    );
    postretro_scripting_core::refresh_plan::apply_descriptor_refresh_plan(
        &plan,
        &mut w.ctx.registry.borrow_mut(),
    )
    .unwrap();
    for descriptor in new {
        w.ctx
            .data_registry
            .borrow_mut()
            .upsert_entity_type(descriptor);
    }
    DefenseParameters::install(&w.ctx);
    w.tick();
    assert_eq!(w.shield(host), (60.0, 100.0));
    assert_eq!(
        w.ctx
            .registry
            .borrow()
            .get_component::<DefenseLayersComponent>(host)
            .unwrap()
            .last_hit_tick,
        stamp
    );
}

// R10 / R-P13: the stamp is not carried, so the first tick after arrival
// recharges before that tick's hits; the next recharge waits a full delay.
#[test]
fn carried_layer_recharges_first_tick_after_arrival() {
    let mut w = world();
    let (host, _) = w.materialize();
    w.tick();
    w.hit(host, 30.0, DamageProducer::InTick);
    w.app.unload_level();
    let (host, _) = w.materialize();
    assert_eq!(w.shield(host), (120.0, 150.0));
    w.tick();
    let first = w.ctx.registry.borrow().host_tick();
    approx(w.shield(host).0, 121.0);
    w.hit(host, 20.0, DamageProducer::InTick);
    approx(w.shield(host).0, 101.0);
    while w.ctx.registry.borrow().host_tick() < first + 179 {
        w.tick();
    }
    approx(w.shield(host).0, 101.0);
    w.tick();
    approx(w.shield(host).0, 102.0);
}

// R11 / R-P2: a hit after a tick's simulation (frame-end script damage in
// single player, a host-flushed client hit in co-op) stamps the tick it
// follows, as an in-tick hit does.
#[test]
fn post_tick_hits_stamp_the_tick_they_follow() {
    let mut w = world();
    let (host, other) = w.materialize();
    w.tick();
    let tick = w.ctx.registry.borrow().host_tick();
    w.hit(host, 10.0, DamageProducer::AppDrain);
    w.hit(other, 10.0, DamageProducer::InTick);
    let stamps: BTreeSet<_> = [host, other]
        .into_iter()
        .map(|pawn| {
            w.ctx
                .registry
                .borrow()
                .get_component::<DefenseLayersComponent>(pawn)
                .unwrap()
                .last_hit_tick
        })
        .collect();
    assert_eq!(stamps, BTreeSet::from([Some(tick)]));
    while w.ctx.registry.borrow().host_tick() < tick + 179 {
        w.tick();
    }
    approx(w.shield(host).0, 140.0);
    w.tick();
    approx(w.shield(host).0, 141.0);
}

// L7 / R-P16, R-P24: a join seed landing through the host's seed path marks
// the seat's pawn; the next tick start redoes the spawn fill, before that
// tick's hits, only for a layer full at its pre-seed capacity.
#[test]
fn late_join_seed_refills_only_layers_full_at_pre_seed_capacity() {
    for pre_hit in [None, Some(20.0)] {
        let mut w = world();
        w.set_capacity(Seat(1), 0.0);
        let (_, joiner) = w.materialize();
        assert_eq!(w.shield(joiner), (50.0, 50.0));
        w.tick();
        if let Some(amount) = pre_hit {
            w.hit(joiner, amount, DamageProducer::InTick);
        }
        let mut ledger = postretro_scripting_core::store_identity::StoreIdentityLedger::empty();
        ledger
            .slots
            .insert(CAPACITY_SLOT.to_string(), "e16-cap".to_string());
        crate::apply_host_join_seed(
            &w.ctx,
            &mut w.ctx.registry.borrow_mut(),
            Some(&ledger),
            &BTreeSet::from([CAPACITY_SLOT.to_string()]),
            Seat(1),
            [(
                "e16-cap".to_string(),
                postretro_net::wire::JoinSeedValue::Number(100.0),
            )]
            .into(),
        );
        w.tick();
        if pre_hit.is_none() {
            assert_eq!(w.shield(joiner), (150.0, 150.0));
            w.hit(joiner, 20.0, DamageProducer::InTick);
            assert_eq!(w.shield(joiner), (130.0, 150.0), "the refill beat the hit");
        } else {
            assert_eq!(w.shield(joiner), (30.0, 150.0));
        }
    }
}

// L6: a fresh joiner whose seed lands before its pawn materializes arrives
// full at the seeded capacity: the seat bind evaluates with the seed's
// per-owner values in place.
#[test]
fn fresh_join_arrives_full_at_seeded_capacity() {
    let mut w = world();
    w.set_capacity(Seat(1), 0.0);
    let mut ledger = postretro_scripting_core::store_identity::StoreIdentityLedger::empty();
    ledger
        .slots
        .insert(CAPACITY_SLOT.to_string(), "e16-cap".to_string());
    // No pawn holds seat 1 yet, so the seed marks nothing.
    crate::apply_host_join_seed(
        &w.ctx,
        &mut w.ctx.registry.borrow_mut(),
        Some(&ledger),
        &BTreeSet::from([CAPACITY_SLOT.to_string()]),
        Seat(1),
        [(
            "e16-cap".to_string(),
            postretro_net::wire::JoinSeedValue::Number(100.0),
        )]
        .into(),
    );
    let (_, joiner) = w.materialize();
    assert_eq!(
        w.shield(joiner),
        (150.0, 150.0),
        "full at the seeded capacity"
    );
    w.tick();
    assert_eq!(w.shield(joiner), (150.0, 150.0));
}
