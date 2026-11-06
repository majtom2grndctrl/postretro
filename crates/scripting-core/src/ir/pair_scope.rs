// Interaction-pair binding scope: a Number expression over two player sides,
// an actor and a subject, evaluated by the host for each pair.
// See: context/lib/scripting.md §11 (interaction-pair scope) · §12 (Assist)

// A pair expression reads either side through its role token
// (`ref.byPlayer(assist.helper)`, `assist.target.inLifeState(name)`; Luau
// spells the calls with `:`), and global
// store values with no owner. Every side value is snapshotted once per pass by
// `seed_sides`, so seeding is linear in pawns; a pair evaluation then selects
// two seeded sides. A side read with no value is a fault the caller detects
// through `observes` before evaluating, never a default: `read` is total and
// `eval_value_checked` flags only non-finite Numbers.
//
// The read-resolution table (`classify_pair_read`) is shared with the
// value-free twin, `PairValidationScope` (`pair_validation.rs`), so the load
// gate and the host bind refuse exactly the same reads.

use std::cell::{Cell, RefCell};

use postretro_foundation::owner_tokens::{ASSIST_HELPER_TOKEN, ASSIST_TARGET_TOKEN};

use crate::ctx::ScriptCtx;
use crate::ir::scope::{BindingScope, ENTITY_STATE_INPUT_PREFIX, ResolvedInput, ResolvedOutput};
use crate::ir::{IrType, IrValue};
use crate::ir_scopes::{StoreHandle, StoreScope};
use crate::life_state_inputs::LIFE_STATE_INPUT_PREFIX;
use crate::player_event_scope::is_local_player_slot;
use crate::player_slots::{PlayerSlot, player_slot_value};
use crate::registry::{EntityId, EntityRegistry};
use crate::slot_table::{SlotTable, SlotValue};

/// What a side of the pair is. A player side is a pawn: its engine
/// per-player slots, its per-owner store values at its seat and its life state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideKind {
    Player,
}

/// The consumer's role tokens and side kinds. Assist binds `@assist.helper`
/// as actor and `@assist.target` as subject; a later hold-to-interact supplies
/// its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairRoles {
    pub actor_token: &'static str,
    pub subject_token: &'static str,
    pub actor_kind: SideKind,
    pub subject_kind: SideKind,
}

/// The assist pair: helper as actor, target as subject.
pub const ASSIST_PAIR_ROLES: PairRoles = PairRoles {
    actor_token: ASSIST_HELPER_TOKEN,
    subject_token: ASSIST_TARGET_TOKEN,
    actor_kind: SideKind::Player,
    subject_kind: SideKind::Player,
};

/// Reserved for facts about the pair itself; holds no names.
pub const PAIR_FACT_PREFIX: &str = "@pair.";

/// Per-player slots whose values depend on the pair evaluation itself, so a
/// pair expression may not read them under any owner.
pub const PAIR_EXCLUDED_SLOTS: [&str; 2] = [
    PlayerSlot::AssistProgress.name(),
    PlayerSlot::AssistReceivedProgress.name(),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PairRole {
    Actor,
    Subject,
}

/// One side read an expression binds, deduplicated across expressions.
#[derive(Clone, Debug, PartialEq)]
pub enum SideInput {
    Engine(PlayerSlot),
    LifeState(u8),
    Owned { name: String, ir_type: IrType },
}

impl SideInput {
    pub fn ir_type(&self) -> IrType {
        match self {
            SideInput::Engine(slot) => slot.ir_type(),
            SideInput::LifeState(_) => IrType::Bool,
            SideInput::Owned { ir_type, .. } => *ir_type,
        }
    }
}

/// A side read one expression makes: which role, and which seeded input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairInput {
    pub role: PairRole,
    pub index: usize,
}

/// The schema facts the read table consults. The runtime scope answers from
/// the live tables; the validation twin from the incoming declarations.
pub(crate) trait PairSchema {
    /// A store record's projected IR type (`None` when it has no projection)
    /// and whether it is per-owner; `None` for an unknown name.
    fn store_record(&self, name: &str) -> Option<(Option<IrType>, bool)>;
    /// The id of a mod-wide life-state name, or `None` when no descriptor
    /// declares it.
    fn life_state_id(&self, name: &str) -> Option<u8>;
}

/// What one read resolves to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PairRead {
    /// A global store value, read live.
    Global(IrType),
    /// A side value, seeded per pass.
    Side(PairRole, SideInput),
}

fn handle_name(token: &str) -> &str {
    token.strip_prefix('@').unwrap_or(token)
}

/// How to read a per-player value through a side, in both runtimes'
/// spellings, so the message is the same whichever runtime authored it.
fn owner_hint(roles: &PairRoles) -> String {
    let (actor, subject) = (
        handle_name(roles.actor_token),
        handle_name(roles.subject_token),
    );
    format!(
        "use `ref.byPlayer({actor})` or `ref.byPlayer({subject})` \
         (Luau: `ref:byPlayer({actor})` or `ref:byPlayer({subject})`)"
    )
}

/// How to read a side's life state, in both runtimes' spellings.
fn life_state_hint(roles: &PairRoles) -> String {
    let (actor, subject) = (
        handle_name(roles.actor_token),
        handle_name(roles.subject_token),
    );
    format!(
        "use `{actor}.inLifeState(name)` or `{subject}.inLifeState(name)` \
         (Luau: `{actor}:inLifeState(name)` or `{subject}:inLifeState(name)`)"
    )
}

fn reserved_read(name: &str) -> String {
    if name.starts_with(ENTITY_STATE_INPUT_PREFIX) {
        format!("reads per-entity state `{name}`, which a pair expression cannot read")
    } else if name.starts_with(PAIR_FACT_PREFIX) {
        format!("reads `{name}`: the `@pair.` fact namespace is reserved and holds no names")
    } else {
        format!("reads dispatch input `{name}`, which a pair expression cannot read")
    }
}

/// The read-resolution table for a pair expression's input leaf.
pub(crate) fn classify_pair_read(
    roles: &PairRoles,
    schema: &impl PairSchema,
    name: &str,
    owner: Option<&str>,
) -> Result<PairRead, String> {
    let role = match owner {
        None => None,
        Some(owner) if owner == roles.actor_token => Some(PairRole::Actor),
        Some(owner) if owner == roles.subject_token => Some(PairRole::Subject),
        Some(owner) => {
            return Err(format!(
                "reads `{name}` with owner `{owner}`, which a pair expression does not know; {}",
                owner_hint(roles)
            ));
        }
    };
    if name.starts_with('@') {
        return Err(reserved_read(name));
    }
    if PAIR_EXCLUDED_SLOTS.contains(&name) {
        return Err(format!(
            "reads `{name}`, whose value depends on this pair evaluation"
        ));
    }
    // Pair-scope layer reads are a non-goal: by either side or plain, a
    // layer read fails load rather than meaning a player the pair never names.
    if crate::player_slots::defense_slot(name).is_some() {
        return Err(format!(
            "reads defense layer `{name}`; a pair expression cannot read a defense layer"
        ));
    }
    let Some(role) = role else {
        return classify_unowned(roles, schema, name);
    };
    if let Some(state) = name.strip_prefix(LIFE_STATE_INPUT_PREFIX) {
        let id = schema
            .life_state_id(state)
            .ok_or_else(|| format!("names life state `{state}`, which no descriptor declares"))?;
        return Ok(PairRead::Side(role, SideInput::LifeState(id)));
    }
    if let Some(slot) = PlayerSlot::from_name(name) {
        return Ok(PairRead::Side(role, SideInput::Engine(slot)));
    }
    match schema.store_record(name) {
        Some((Some(ir_type), true)) => Ok(PairRead::Side(
            role,
            SideInput::Owned {
                name: name.to_string(),
                ir_type,
            },
        )),
        Some((None, true)) => Err(format!(
            "reads `{name}`, whose type is not a number or boolean"
        )),
        Some((_, false)) => Err(format!(
            "reads unknown per-player input `{name}` with owner `{}`; it is not a per-player value",
            match role {
                PairRole::Actor => roles.actor_token,
                PairRole::Subject => roles.subject_token,
            }
        )),
        None => Err(format!("reads unknown input `{name}`")),
    }
}

fn classify_unowned(
    roles: &PairRoles,
    schema: &impl PairSchema,
    name: &str,
) -> Result<PairRead, String> {
    if name.starts_with(LIFE_STATE_INPUT_PREFIX) {
        return Err(format!(
            "reads `inLifeState` with no owner; {}",
            life_state_hint(roles)
        ));
    }
    // A local-only `player.*` slot is a global store record, not a
    // `PlayerSlot`: each machine publishes it for its own player, so the host
    // holds no value for either side. Refused before the global row.
    if is_local_player_slot(name) {
        return Err(format!(
            "reads `{name}`, which each machine publishes for its own player; a pair expression cannot read it"
        ));
    }
    if PlayerSlot::from_name(name).is_some() {
        return Err(format!(
            "reads per-player `{name}` with no owner; {}",
            owner_hint(roles)
        ));
    }
    match schema.store_record(name) {
        Some((_, true)) => Err(format!(
            "reads per-player `{name}` with no owner; {}",
            owner_hint(roles)
        )),
        Some((Some(ir_type), false)) => Ok(PairRead::Global(ir_type)),
        Some((None, false)) => Err(format!(
            "reads `{name}`, whose type is not a number or boolean"
        )),
        None => Err(format!("reads unknown input `{name}`")),
    }
}

/// A resolved read in [`InteractionPairScope`].
#[derive(Clone, Debug)]
pub enum PairInputHandle {
    /// A global store value, read live: no fire applies during a pass.
    Store(StoreHandle),
    /// A seeded side input of the selected actor or subject.
    Side(PairInput),
}

struct LiveSchema<'a> {
    store: &'a StoreScope,
}

impl PairSchema for LiveSchema<'_> {
    fn store_record(&self, name: &str) -> Option<(Option<IrType>, bool)> {
        self.store.store_record(name)
    }

    fn life_state_id(&self, name: &str) -> Option<u8> {
        self.store.life_state_id(name)
    }
}

/// The runtime pair scope. One instance serves every expression of one bind;
/// its bound programs hold indices into its input list, so they are only
/// valid against this instance.
pub struct InteractionPairScope {
    roles: PairRoles,
    store: StoreScope,
    /// Side inputs bound by any expression, deduplicated.
    inputs: RefCell<Vec<SideInput>>,
    /// The side inputs the expression being bound reads.
    recording: RefCell<Vec<PairInput>>,
    /// Why the last refused read was refused, for the caller's warning.
    last_error: RefCell<Option<String>>,
    /// Seeded pawns, in seeding order.
    side_pawns: RefCell<Vec<EntityId>>,
    /// `side_values[side * inputs.len() + input]`; `None` when absent.
    side_values: RefCell<Vec<Option<IrValue>>>,
    /// The selected (actor, subject) side indices.
    selected: Cell<Option<(usize, usize)>>,
}

impl InteractionPairScope {
    pub fn new(ctx: ScriptCtx, roles: PairRoles) -> Self {
        Self {
            roles,
            store: StoreScope::script(ctx),
            inputs: RefCell::new(Vec::new()),
            recording: RefCell::new(Vec::new()),
            last_error: RefCell::new(None),
            side_pawns: RefCell::new(Vec::new()),
            side_values: RefCell::new(Vec::new()),
            selected: Cell::new(None),
        }
    }

    pub fn roles(&self) -> &PairRoles {
        &self.roles
    }

    /// Start recording the side inputs of the next bind.
    pub fn begin_expression(&self) {
        self.recording.borrow_mut().clear();
        self.last_error.borrow_mut().take();
    }

    /// The side inputs the expression bound since [`Self::begin_expression`],
    /// deduplicated and sorted.
    pub fn finish_expression(&self) -> Vec<PairInput> {
        let mut recorded = std::mem::take(&mut *self.recording.borrow_mut());
        recorded.sort_unstable();
        recorded.dedup();
        recorded
    }

    /// Why the last refused read of this bind was refused.
    pub fn take_bind_error(&self) -> Option<String> {
        self.last_error.borrow_mut().take()
    }

    /// Snapshot every bound side input for each pawn, once per pass. A pawn's
    /// per-owner values read at its seat; a seatless pawn, or any pawn when
    /// there is no slot table, reads them missing. Clears the previous pass's
    /// sides and the selection.
    pub fn seed_sides(
        &self,
        registry: &EntityRegistry,
        slot_table: Option<&SlotTable>,
        pawns: &[EntityId],
    ) {
        let inputs = self.inputs.borrow();
        let mut side_pawns = self.side_pawns.borrow_mut();
        let mut values = self.side_values.borrow_mut();
        side_pawns.clear();
        values.clear();
        self.selected.set(None);
        for &pawn in pawns {
            side_pawns.push(pawn);
            let seat = registry.seat_for_pawn(pawn);
            values.extend(inputs.iter().map(|input| {
                match input {
                    SideInput::Engine(slot) => project(
                        slot.ir_type(),
                        player_slot_value(registry, *slot, pawn).as_ref(),
                    ),
                    SideInput::LifeState(state) => registry
                        .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
                        .ok()
                        .map(|movement| IrValue::Bool(movement.life_state == *state)),
                    SideInput::Owned { name, ir_type } => {
                        let seat = seat?;
                        let record = slot_table?.get(name)?;
                        project(*ir_type, record.per_seat_value(seat))
                    }
                }
            }));
        }
    }

    /// Select a seeded (actor, subject) pair for the next evaluations. False,
    /// with nothing selected, when either pawn was not seeded this pass.
    pub fn select(&self, actor: EntityId, subject: EntityId) -> bool {
        let pawns = self.side_pawns.borrow();
        let index = |pawn| pawns.iter().position(|seeded| *seeded == pawn);
        let selected = index(actor).zip(index(subject));
        self.selected.set(selected);
        selected.is_some()
    }

    /// Whether the selected pair has a value for every listed side input.
    /// False with no selection.
    pub fn observes(&self, inputs: &[PairInput]) -> bool {
        let Some(selected) = self.selected.get() else {
            return false;
        };
        let values = self.side_values.borrow();
        inputs
            .iter()
            .all(|input| self.side_value(&values, selected, *input).is_some())
    }

    fn side_value(
        &self,
        values: &[Option<IrValue>],
        (actor, subject): (usize, usize),
        input: PairInput,
    ) -> Option<IrValue> {
        let side = match input.role {
            PairRole::Actor => actor,
            PairRole::Subject => subject,
        };
        let width = self.inputs.borrow().len();
        values.get(side * width + input.index).copied().flatten()
    }

    fn bind_side_input(&self, role: PairRole, input: SideInput) -> PairInput {
        let mut inputs = self.inputs.borrow_mut();
        let index = match inputs.iter().position(|bound| *bound == input) {
            Some(index) => index,
            None => {
                // Widening the input list invalidates any seeded pass.
                self.side_pawns.borrow_mut().clear();
                self.side_values.borrow_mut().clear();
                self.selected.set(None);
                inputs.push(input);
                inputs.len() - 1
            }
        };
        let input = PairInput { role, index };
        self.recording.borrow_mut().push(input);
        input
    }

    fn resolve(&self, name: &str, owner: Option<&str>) -> Option<ResolvedInput<PairInputHandle>> {
        let schema = LiveSchema { store: &self.store };
        match classify_pair_read(&self.roles, &schema, name, owner) {
            Ok(PairRead::Global(_)) => {
                let resolved = self.store.resolve_input(name)?;
                Some(ResolvedInput {
                    handle: PairInputHandle::Store(resolved.handle),
                    ir_type: resolved.ir_type,
                })
            }
            Ok(PairRead::Side(role, input)) => {
                let ir_type = input.ir_type();
                Some(ResolvedInput {
                    handle: PairInputHandle::Side(self.bind_side_input(role, input)),
                    ir_type,
                })
            }
            Err(reason) => {
                self.last_error.borrow_mut().get_or_insert(reason);
                None
            }
        }
    }
}

fn project(ir_type: IrType, value: Option<&SlotValue>) -> Option<IrValue> {
    StoreScope::project_slot_value(ir_type, value)
}

impl BindingScope for InteractionPairScope {
    type InputHandle = PairInputHandle;
    type OutputHandle = ();

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>> {
        self.resolve(name, None)
    }

    fn resolve_owned_input(
        &self,
        name: &str,
        owner: &str,
    ) -> Option<ResolvedInput<Self::InputHandle>> {
        self.resolve(name, Some(owner))
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<Self::OutputHandle>> {
        None
    }

    fn read(&self, handle: &Self::InputHandle) -> IrValue {
        match handle {
            PairInputHandle::Store(handle) => self.store.read(handle),
            PairInputHandle::Side(input) => {
                let fallback = self.inputs.borrow()[input.index].ir_type().zero();
                let Some(selected) = self.selected.get() else {
                    return fallback;
                };
                self.side_value(&self.side_values.borrow(), selected, *input)
                    .unwrap_or(fallback)
            }
        }
    }

    fn write(&mut self, _handle: &Self::OutputHandle, _value: IrValue) {}
}

#[cfg(test)]
#[path = "pair_scope_tests.rs"]
mod tests;
