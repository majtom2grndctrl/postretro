// Binding scope for player-event conditions: reads bind to the evaluated player.
// See: context/lib/scripting.md §12 (Player events)

// A plain read of a per-player slot inside a condition means the player being
// evaluated: engine per-player slots read that player's pawn through the
// shared lookup, mod per-owner slots read that player's seat. Global slots
// read as everywhere. The binding is lexical — it belongs to the condition,
// never to a reaction the event fires.
//
// Each player's per-player inputs are snapshotted once by `seed_player`, then
// every condition evaluates against the snapshot. A condition reading an input
// the player has no value for is unobserved for that player, never evaluated
// against a default.

use std::cell::RefCell;

use postretro_foundation::Seat;

use crate::ctx::ScriptCtx;
use crate::ir::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use crate::ir::{IrType, IrValue};
use crate::ir_scopes::{StoreHandle, StoreScope};
use crate::player_slots::{PlayerSlot, player_slot_value};
use crate::registry::{EntityId, EntityRegistry};
use crate::slot_table::{SlotTable, SlotValue};

#[derive(Clone, Debug)]
pub enum PlayerConditionHandle {
    /// A global slot, read live: no fire applies during the evaluation pass.
    Store(StoreHandle),
    /// An index into the per-player snapshot.
    Player(usize),
}

#[derive(Clone, Debug, PartialEq)]
enum PlayerInput {
    Engine(PlayerSlot),
    Owned { name: String, ir_type: IrType },
}

impl PlayerInput {
    fn ir_type(&self) -> IrType {
        match self {
            PlayerInput::Engine(slot) => slot.ir_type(),
            PlayerInput::Owned { ir_type, .. } => *ir_type,
        }
    }
}

/// A read-only scope shared by every player-event condition of one install.
pub struct PlayerConditionScope {
    store: StoreScope,
    /// Per-player inputs bound by any condition, deduplicated.
    inputs: RefCell<Vec<PlayerInput>>,
    /// The per-player inputs the condition being bound reads.
    recording: RefCell<Vec<usize>>,
    /// The seeded player's value for each input; `None` when absent.
    values: RefCell<Vec<Option<IrValue>>>,
}

impl PlayerConditionScope {
    pub fn new(ctx: ScriptCtx) -> Self {
        Self {
            store: StoreScope::script(ctx),
            inputs: RefCell::new(Vec::new()),
            recording: RefCell::new(Vec::new()),
            values: RefCell::new(Vec::new()),
        }
    }

    /// Start recording the per-player inputs of the next bind.
    pub fn begin_condition(&self) {
        self.recording.borrow_mut().clear();
    }

    /// The per-player input indices the condition bound since
    /// [`Self::begin_condition`], deduplicated and sorted.
    pub fn finish_condition(&self) -> Vec<usize> {
        let mut recorded = std::mem::take(&mut *self.recording.borrow_mut());
        recorded.sort_unstable();
        recorded.dedup();
        recorded
    }

    /// Snapshot every bound per-player input for one player. The registry is
    /// the caller's borrow: the evaluation seam reads under a live tick.
    pub fn seed_player(
        &self,
        registry: &EntityRegistry,
        slot_table: &SlotTable,
        pawn: EntityId,
        seat: Option<Seat>,
    ) {
        let inputs = self.inputs.borrow();
        let mut values = self.values.borrow_mut();
        values.clear();
        values.extend(inputs.iter().map(|input| match input {
            PlayerInput::Engine(slot) => {
                project(slot.ir_type(), player_slot_value(registry, *slot, pawn).as_ref())
            }
            PlayerInput::Owned { name, ir_type } => {
                let seat = seat?;
                let record = slot_table.get(name)?;
                project(*ir_type, record.per_seat_value(seat))
            }
        }));
    }

    /// Whether the seeded player has a value for every listed input.
    pub fn observes(&self, inputs: &[usize]) -> bool {
        let values = self.values.borrow();
        inputs
            .iter()
            .all(|index| values.get(*index).is_some_and(Option::is_some))
    }

    fn bind_player_input(&self, input: PlayerInput) -> usize {
        let mut inputs = self.inputs.borrow_mut();
        let index = match inputs.iter().position(|bound| *bound == input) {
            Some(index) => index,
            None => {
                inputs.push(input);
                inputs.len() - 1
            }
        };
        self.recording.borrow_mut().push(index);
        index
    }
}

fn project(ir_type: IrType, value: Option<&SlotValue>) -> Option<IrValue> {
    match (ir_type, value?) {
        (IrType::Number, SlotValue::Number(value)) => Some(IrValue::Number(*value)),
        (IrType::Bool, SlotValue::Boolean(value)) => Some(IrValue::Bool(*value)),
        _ => None,
    }
}

impl BindingScope for PlayerConditionScope {
    type InputHandle = PlayerConditionHandle;
    type OutputHandle = ();

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>> {
        if let Some(slot) = PlayerSlot::from_name(name) {
            let index = self.bind_player_input(PlayerInput::Engine(slot));
            return Some(ResolvedInput {
                handle: PlayerConditionHandle::Player(index),
                ir_type: slot.ir_type(),
            });
        }
        if let Some(resolved) = self.store.resolve_owner_input(name) {
            let ir_type = resolved.ir_type;
            let index = self.bind_player_input(PlayerInput::Owned {
                name: resolved.handle.name().to_string(),
                ir_type,
            });
            return Some(ResolvedInput {
                handle: PlayerConditionHandle::Player(index),
                ir_type,
            });
        }
        // Reserved dispatch names (`@rising`, `@player`) have no meaning in a
        // condition.
        if name.starts_with('@') {
            return None;
        }
        self.store.resolve_input(name).map(|resolved| ResolvedInput {
            handle: PlayerConditionHandle::Store(resolved.handle),
            ir_type: resolved.ir_type,
        })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<Self::OutputHandle>> {
        None
    }

    fn read(&self, handle: &Self::InputHandle) -> IrValue {
        match handle {
            PlayerConditionHandle::Store(handle) => self.store.read(handle),
            PlayerConditionHandle::Player(index) => {
                let inputs = self.inputs.borrow();
                let fallback = inputs[*index].ir_type().zero();
                self.values
                    .borrow()
                    .get(*index)
                    .copied()
                    .flatten()
                    .unwrap_or(fallback)
            }
        }
    }

    fn write(&mut self, _handle: &Self::OutputHandle, _value: IrValue) {}
}
