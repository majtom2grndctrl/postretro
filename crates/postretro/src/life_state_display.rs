//! Life-state HUD projection at the frozen UI boundary; gameplay keeps tick facts.
//! See: context/lib/scripting.md §5 · context/lib/networking.md §What gates, and what replicates instead
use crate::life_state_clock::{LifeStateClock, predicted_target_ms};
use postretro_entities::{EntityId, ScriptCtx, SlotValue};
use postretro_foundation::{PlayerMovementComponent, UNDECLARED_LIFE_STATE};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    pawn: EntityId,
    name: String,
    teleport_generation: u8,
}

/// One frame's life-state reading. Reset evidence differs by role: the host
/// reads the authoritative elapsed tick count (`Some`, witness 0) and detects a
/// rewind by comparing it; a client has no authoritative count (`None`) and
/// relies on the state-slot apply's elapsed-reset witness instead.
pub(crate) struct Sample {
    identity: Identity,
    target_ms: f64,
    authoritative_ticks: Option<u32>,
    reset_witness: u64,
}

#[derive(Default)]
pub(crate) struct LifeStateDisplay {
    clock: LifeStateClock,
    identity: Option<Identity>,
    authoritative_ticks: Option<u32>,
    reset_witness: u64,
    reset_serial: u64,
}

impl LifeStateDisplay {
    pub(crate) fn project(
        &mut self,
        values: &mut HashMap<String, SlotValue>,
        sample: Option<Sample>,
        frame_dt: Duration,
    ) {
        let Some(sample) = sample else {
            // No declared own pawn: show zero over whatever the store or a
            // client's timed target last held, so a gone pawn's clock never
            // freezes on screen.
            self.identity = None;
            self.authoritative_ticks = None;
            self.reset_witness = 0;
            self.clock.clear();
            write_elapsed(values, 0.0);
            return;
        };
        let reset = self.identity.as_ref() != Some(&sample.identity)
            || sample.reset_witness != self.reset_witness
            || sample
                .authoritative_ticks
                .zip(self.authoritative_ticks)
                .is_some_and(|(new, old)| new < old);
        if reset {
            self.reset_serial = self.reset_serial.wrapping_add(1);
        }
        self.identity = Some(sample.identity);
        self.authoritative_ticks = sample.authoritative_ticks;
        self.reset_witness = sample.reset_witness;
        let display = self
            .clock
            .advance(frame_dt, sample.target_ms, self.reset_serial);
        write_elapsed(values, display as f32);
    }
}

fn write_elapsed(values: &mut HashMap<String, SlotValue>, elapsed_ms: f32) {
    let value = SlotValue::Number(elapsed_ms);
    if let Some(slot) = values.get_mut("player.lifeStateMs") {
        *slot = value;
    } else {
        values.insert("player.lifeStateMs".into(), value);
    }
}

pub(crate) struct UiProjection<'a> {
    pub display: &'a mut LifeStateDisplay,
    pub sample: Option<Sample>,
    pub frame_dt: Duration,
}

pub(crate) fn sample(
    ctx: &ScriptCtx,
    endpoint: Option<&crate::netcode::NetEndpoint>,
    alpha: f32,
) -> Option<Sample> {
    let registry = ctx.registry.borrow();
    let pawn = if matches!(endpoint, Some(crate::netcode::NetEndpoint::Client { .. })) {
        registry.local_player_pawn()?
    } else {
        registry.local_player_movement_pawn()?
    };
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()?;
    if movement.life_state == UNDECLARED_LIFE_STATE {
        return None;
    }
    let (name, authoritative_ticks, reset_witness) = match endpoint {
        Some(crate::netcode::NetEndpoint::Client {
            life_state_tables,
            state_slots,
            ..
        }) => (
            life_state_tables.name(movement).to_owned(),
            None,
            state_slots.life_state_elapsed_witness().reset_serial,
        ),
        _ => (
            ctx.data_registry
                .borrow()
                .life_state_names
                .get(usize::from(movement.life_state))?
                .clone(),
            Some(movement.life_state_elapsed_ticks),
            0,
        ),
    };
    Some(Sample {
        identity: Identity {
            pawn,
            name,
            teleport_generation: movement.teleport.generation,
        },
        target_ms: predicted_target_ms(movement.life_state_elapsed_ticks, alpha),
        authoritative_ticks,
        reset_witness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use postretro_entities::{EntityRegistry, SlotTable, Transform};

    fn sample(
        pawn: EntityId,
        name: &str,
        target_ms: f64,
        authoritative_ticks: Option<u32>,
        reset_witness: u64,
        teleport_generation: u8,
    ) -> Sample {
        Sample {
            identity: Identity {
                pawn,
                name: name.into(),
                teleport_generation,
            },
            target_ms,
            authoritative_ticks,
            reset_witness,
        }
    }

    #[test]
    fn life_state_ui_projection_advances_frozen_value_without_mutating_gameplay_store() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut slots = SlotTable::new();
        slots
            .get_mut("player.lifeStateMs")
            .unwrap()
            .write_value(Some(SlotValue::Number(100.0)));
        slots
            .get_mut("player.lifeState")
            .unwrap()
            .write_value(Some(SlotValue::String("down".into())));
        let mut display = LifeStateDisplay::default();
        let dt = Duration::from_millis(8);
        let mut project = |target| {
            App::build_ui_display_slot_snapshot(
                &slots,
                None,
                Some(UiProjection {
                    display: &mut display,
                    sample: Some(sample(pawn, "down", target, None, 0, 0)),
                    frame_dt: dt,
                }),
            )
        };
        let first = project(104.0);
        let second = project(95.0); // replay/backlog correction, no authoritative reset
        let number = |values: &HashMap<String, SlotValue>| match values["player.lifeStateMs"] {
            SlotValue::Number(value) => value,
            _ => unreachable!(),
        };
        assert!(number(&second) > number(&first));
        assert_eq!(second["player.lifeState"], SlotValue::String("down".into()));
        assert_eq!(
            slots.get("player.lifeStateMs").unwrap().value,
            Some(SlotValue::Number(100.0))
        );
    }

    // Regression: a client's own pawn removed while downed (level change,
    // demotion, digest hold) froze the bleed-out clock on screen.
    #[test]
    fn life_state_ui_projection_shows_zero_once_the_own_pawn_is_gone() {
        let ctx = ScriptCtx::new();
        let pawn = ctx.registry.borrow_mut().spawn(Transform::default());
        let mut slots = SlotTable::new();
        slots
            .get_mut("player.lifeStateMs")
            .unwrap()
            .write_value(Some(SlotValue::Number(9_000.0)));
        let mut display = LifeStateDisplay::default();
        let dt = Duration::from_millis(8);
        let mut project = |sample| {
            App::build_ui_display_slot_snapshot(
                &slots,
                None,
                Some(UiProjection {
                    display: &mut display,
                    sample,
                    frame_dt: dt,
                }),
            )
        };
        let downed = project(Some(sample(pawn, "down", 9_000.0, None, 0, 0)));
        assert_eq!(downed["player.lifeStateMs"], SlotValue::Number(9_000.0));
        ctx.registry.borrow_mut().despawn(pawn).unwrap();
        let gone = project(super::sample(&ctx, None, 0.0));
        assert_eq!(gone["player.lifeStateMs"], SlotValue::Number(0.0));
    }

    #[test]
    fn life_state_identity_keeps_name_continuity_and_resets_on_real_witnesses() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let replacement = registry.spawn(Transform::default());
        let mut display = LifeStateDisplay::default();
        let mut values = HashMap::new();
        let dt = Duration::from_millis(8);
        let mut project = |pawn, name, target, ticks, witness, generation| {
            display.project(
                &mut values,
                Some(sample(pawn, name, target, ticks, witness, generation)),
                dt,
            );
            match values["player.lifeStateMs"] {
                SlotValue::Number(value) => value,
                _ => unreachable!(),
            }
        };
        assert_eq!(project(pawn, "down", 1000.0, None, 0, 0), 1000.0);
        assert!(
            project(pawn, "down", 900.0, None, 0, 0) > 1000.0,
            "kept name ignores numeric id/revision and replay corrections"
        );
        assert_eq!(
            project(pawn, "down", 10.0, None, 1, 0),
            10.0,
            "accepted authoritative elapsed decrease survives later samples"
        );
        assert_eq!(project(pawn, "alive", 2.0, None, 1, 0), 2.0);
        assert_eq!(project(pawn, "alive", 1.0, None, 1, 1), 1.0);
        assert_eq!(project(replacement, "alive", 0.5, None, 1, 1), 0.5);
        assert!(project(replacement, "alive", 17.0, Some(1), 1, 1) > 0.5);
        assert_eq!(
            project(replacement, "alive", 0.0, Some(0), 1, 1),
            0.0,
            "host authoritative decrease resets"
        );
    }
}
