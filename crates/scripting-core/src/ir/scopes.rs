// Runtime `BindingScope` adapters for store-backed state and ephemeral dispatch inputs.
// Governing contract: `context/lib/scripting.md` §§11–12.

// `StoreScope` bridges the IR evaluator to the live `SlotTable` through a
// captured `ScriptCtx`; it projects `Number`/`Boolean` slots into the IR value
// model and gates writes by a capability `mode` mirroring the engine-bypass vs
// script-gated split in `primitives::store`.

use std::cell::RefCell;

use crate::components::entity_state::EntityStateComponent;
use crate::components::health::{IMPACT_DISPATCH_INPUTS, IMPACT_SOURCE_TOKEN, ImpactDispatch};
use crate::ctx::ScriptCtx;
use crate::ir::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use crate::ir::{IrType, IrValue};
use crate::registry::{EntityId, EntityRegistry};
use crate::slot_table::{SlotType, SlotValue};
use crate::store_bridge::write_store_slot;
use postretro_foundation::Seat;

/// Write-capability mode for a [`StoreScope`]. Mirrors the two write paths in
/// `primitives::store`: an engine-policy program bypasses the readonly flag
/// (engine systems own those slots), while a script-authored program is gated
/// by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreCapability {
    /// Engine-policy IR (e.g. shield recharge). `resolve_output` grants a write
    /// handle for *any* projectable slot, readonly included; `write` delegates
    /// to the validated engine-bypass path.
    Engine,
    /// Script-authored IR (the deferred UI `setState`). `resolve_output` grants
    /// a handle only for non-readonly projectable slots; a readonly slot is
    /// denied at bind.
    Script,
}

/// A resolved store handle: the slot's stable dotted name plus its projected IR
/// type. Owning the name keeps the handle valid for the program's lifetime
/// without borrowing the table; the type is cached so `read` need not re-derive
/// it from the live slot.
#[derive(Clone, Debug)]
pub struct StoreHandle {
    name: String,
    ir_type: IrType,
}

/// Binds and evaluates IR against the engine-global [`SlotTable`] via a captured
/// [`ScriptCtx`]. Cloning the ctx is cheap (it bumps `Rc`s); the scope owns its
/// clone so it can read and write the live table without an external borrow.
pub struct StoreScope {
    ctx: ScriptCtx,
    mode: StoreCapability,
}

impl StoreScope {
    /// An engine-policy scope: writes bypass the readonly flag through the
    /// validated engine path.
    pub fn engine(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            mode: StoreCapability::Engine,
        }
    }

    /// A script-capability scope: readonly slots are denied a write handle at
    /// bind; granted writes flow through the same validated engine write path.
    pub fn script(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            mode: StoreCapability::Script,
        }
    }

    /// Project a slot's declared type into the IR value model, or `None` for the
    /// non-projectable kinds (`String`/`Enum`/`Array`).
    fn project(slot_type: &SlotType) -> Option<IrType> {
        match slot_type {
            SlotType::Number => Some(IrType::Number),
            SlotType::Boolean => Some(IrType::Bool),
            SlotType::String | SlotType::Enum { .. } | SlotType::Array => None,
        }
    }

    fn project_value(ir_type: IrType, value: Option<&SlotValue>) -> IrValue {
        match (ir_type, value) {
            (IrType::Number, Some(SlotValue::Number(value))) => IrValue::Number(*value),
            (IrType::Bool, Some(SlotValue::Boolean(value))) => IrValue::Bool(*value),
            // An absent value or a slot whose declaration changed after bind
            // is still total under the evaluator contract.
            (IrType::Number, _) => IrValue::Number(0.0),
            (IrType::Bool, _) => IrValue::Bool(false),
        }
    }

    /// Resolve the addressable half of an owner store. Only the impact scope
    /// exposes this handle after it validates the owner token.
    fn resolve_owner_input(&self, name: &str) -> Option<ResolvedInput<StoreHandle>> {
        let table = self.ctx.slot_table.borrow();
        let record = table.get(name)?;
        if !record.schema.per_owner {
            return None;
        }
        let ir_type = Self::project(&record.schema.slot_type)?;
        Some(ResolvedInput {
            handle: StoreHandle {
                name: name.to_string(),
                ir_type,
            },
            ir_type,
        })
    }

    fn per_owner_slot(&self, name: &str) -> Option<bool> {
        self.ctx
            .slot_table
            .borrow()
            .get(name)
            .map(|record| record.schema.per_owner)
    }
}

/// A resolved input in a [`DispatchScope`]: either an index into its ephemeral
/// value snapshot or a handle delegated to its ambient [`StoreScope`].
#[derive(Clone, Debug)]
pub enum DispatchInputHandle {
    Dispatch(usize),
    Store(StoreHandle),
}

/// Why an ephemeral dispatch input could not be seeded for an evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchSeedError {
    /// The name is not part of this dispatch source's fixed vocabulary.
    UnknownInput { name: String },
    /// The runtime value disagrees with the type projected when the program was
    /// bound. Callers should warn and skip evaluation rather than coercing it.
    TypeMismatch {
        name: &'static str,
        expected: IrType,
        actual: IrType,
    },
}

/// Layers per-dispatch ephemeral inputs over the ambient store namespace.
///
/// The vocabulary is fixed for the scope's lifetime, so bound dispatch handles
/// remain stable. Values are seeded in place before each evaluation; programs
/// bind once and observe the refreshed snapshot on every fire.
pub struct DispatchScope {
    store: StoreScope,
    inputs: &'static [(&'static str, IrType)],
    values: Box<[IrValue]>,
}

impl DispatchScope {
    /// An engine-policy dispatch scope whose delegated store writes bypass
    /// readonly through the validated engine path.
    pub fn engine(ctx: ScriptCtx, inputs: &'static [(&'static str, IrType)]) -> Self {
        Self::new(StoreScope::engine(ctx), inputs)
    }

    /// A script-capability dispatch scope whose delegated store writes are
    /// readonly-gated at bind.
    pub fn script(ctx: ScriptCtx, inputs: &'static [(&'static str, IrType)]) -> Self {
        Self::new(StoreScope::script(ctx), inputs)
    }

    fn new(store: StoreScope, inputs: &'static [(&'static str, IrType)]) -> Self {
        let values = inputs
            .iter()
            .map(|(_, ir_type)| match ir_type {
                IrType::Number => IrValue::Number(0.0),
                IrType::Bool => IrValue::Bool(false),
            })
            .collect();
        Self {
            store,
            inputs,
            values,
        }
    }

    /// Seed one ephemeral input for the next evaluation.
    ///
    /// A mismatched type or a name outside the fixed vocabulary is refused and
    /// leaves the snapshot unchanged.
    pub fn seed(&mut self, name: &str, value: IrValue) -> Result<(), DispatchSeedError> {
        let Some(index) = self
            .inputs
            .iter()
            .position(|(input_name, _)| *input_name == name)
        else {
            return Err(DispatchSeedError::UnknownInput {
                name: name.to_string(),
            });
        };
        let (input_name, expected) = self.inputs[index];
        let actual = value.ir_type();
        if actual != expected {
            return Err(DispatchSeedError::TypeMismatch {
                name: input_name,
                expected,
                actual,
            });
        }
        self.values[index] = value;
        Ok(())
    }
}

impl BindingScope for DispatchScope {
    type InputHandle = DispatchInputHandle;
    type OutputHandle = StoreHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>> {
        if name.starts_with('@') {
            let handle = self
                .inputs
                .iter()
                .position(|(input_name, _)| *input_name == name)?;
            return Some(ResolvedInput {
                handle: DispatchInputHandle::Dispatch(handle),
                ir_type: self.inputs[handle].1,
            });
        }

        self.store
            .resolve_input(name)
            .map(|resolved| ResolvedInput {
                handle: DispatchInputHandle::Store(resolved.handle),
                ir_type: resolved.ir_type,
            })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<Self::OutputHandle>> {
        self.store.resolve_output(name)
    }

    fn read(&self, handle: &Self::InputHandle) -> IrValue {
        match handle {
            DispatchInputHandle::Dispatch(index) => self.values[*index],
            DispatchInputHandle::Store(handle) => self.store.read(handle),
        }
    }

    fn write(&mut self, handle: &Self::OutputHandle, value: IrValue) {
        self.store.write(handle, value);
    }
}

/// Reserved prefix for per-instance numeric state leaves. Owned by the binding
/// seam in `postretro-foundation` because behavior-graph guards route the same
/// prefix from a scope that cannot depend on this crate.
pub use crate::ir::ENTITY_STATE_INPUT_PREFIX;

/// A resolved input in an [`EntityScope`]. State handles keep their name while
/// impact facts and ambient store reads retain the handles from the dispatch
/// layer below them.
#[derive(Clone, Debug)]
pub enum EntityInputHandle {
    State(usize),
    Dispatch(usize),
    Store(usize),
    /// A per-owner store handle reads its addressed seat's live value rather
    /// than the fire-time scalar store snapshot.
    OwnedStore(StoreHandle),
}

/// A resolved output in an [`EntityScope`].
#[derive(Clone, Debug)]
pub enum EntityOutputHandle {
    State(String),
    Store(StoreHandle),
}

/// Composite scope for an impact policy firing against one target entity.
///
/// `@impact.*` facts resolve through the embedded [`DispatchScope`],
/// `@state.*` resolves to the current target's per-instance state, and bare
/// names delegate through the dispatch layer to the global store. The target
/// id is deliberately an ambient per-fire channel rather than an `IrValue`:
/// entity ids are command-target tokens, not numeric IR inputs.
pub struct EntityScope {
    dispatch: DispatchScope,
    registry: std::rc::Rc<RefCell<EntityRegistry>>,
    /// The names bound as state leaves. Binding records them once; each impact
    /// snapshots precisely those fields before its effects can write live state.
    state_names: RefCell<Vec<String>>,
    target: Option<EntityId>,
    /// Parallel to `state_names`: handles bind to this stable index, and each
    /// fire replaces the values in place without changing those handles.
    snapshot: RefCell<Vec<f32>>,
    /// Bound bare-store leaves share the same fire-time snapshot as entity
    /// state. Policy effects must not make a later group observe an earlier
    /// group's write during the same impact dispatch.
    store_handles: RefCell<Vec<StoreHandle>>,
    store_snapshot: RefCell<Vec<IrValue>>,
    /// The seat resolved from the current impact source. It is refreshed once
    /// per fire from the caller-owned registry so evaluation can read without
    /// re-borrowing that registry while fixed-tick damage holds it mutably.
    owner_seat: Option<Seat>,
}

impl EntityScope {
    /// Construct the host-authoritative impact-policy composite scope.
    ///
    /// Entity state itself is host-only. Bare global-store outputs still use
    /// script capability because an impact policy is mod-authored data and
    /// must not acquire a write handle for readonly engine slots.
    pub fn impact(ctx: ScriptCtx) -> Self {
        Self {
            dispatch: DispatchScope::script(ctx.clone(), &IMPACT_DISPATCH_INPUTS),
            registry: ctx.registry,
            state_names: RefCell::new(Vec::new()),
            target: None,
            snapshot: RefCell::new(Vec::new()),
            store_handles: RefCell::new(Vec::new()),
            store_snapshot: RefCell::new(Vec::new()),
            owner_seat: None,
        }
    }

    /// Refresh the numeric impact facts and the target command token for one
    /// impact fire. Bound programs remain valid and observe this new snapshot.
    pub fn seed_impact(&mut self, dispatch: &ImpactDispatch) -> Result<(), DispatchSeedError> {
        for (name, value) in dispatch.ir_values() {
            self.dispatch.seed(name, value)?;
        }
        let registry = self.registry.clone();
        let registry = registry.borrow();
        self.seed_owner_seat_from_registry(&registry, dispatch.source);
        self.seed_target_from_registry(&registry, dispatch.target);
        Ok(())
    }

    /// Refresh one impact snapshot from a registry the caller already owns.
    ///
    /// The fixed-tick damage seam holds a mutable registry borrow while it
    /// evaluates the just-published impact. Reading through the captured
    /// `ScriptCtx` there would re-borrow the same `RefCell`; this explicit path
    /// preserves the same snapshot contract without aliasing that borrow.
    pub fn seed_impact_from_registry(
        &mut self,
        registry: &EntityRegistry,
        dispatch: &ImpactDispatch,
    ) -> Result<(), DispatchSeedError> {
        for (name, value) in dispatch.ir_values() {
            self.dispatch.seed(name, value)?;
        }
        self.seed_owner_seat_from_registry(registry, dispatch.source);
        self.seed_target_from_registry(registry, dispatch.target);
        Ok(())
    }

    /// Set the current target through the command-target ambient channel and
    /// freeze every state field already bound by this scope. This intentionally
    /// does not use [`DispatchScope::seed`], whose values are only numbers and
    /// booleans.
    pub fn seed_target(&mut self, target: EntityId) {
        let registry = self.registry.clone();
        let registry = registry.borrow();
        self.seed_target_from_registry(&registry, target);
    }

    fn seed_target_from_registry(&mut self, registry: &EntityRegistry, target: EntityId) {
        self.target = Some(target);

        let names = self.state_names.borrow();
        let mut snapshot = self.snapshot.borrow_mut();
        let state = registry.get_component::<EntityStateComponent>(target).ok();
        for (index, name) in names.iter().enumerate() {
            snapshot[index] = state.map_or(0.0, |state| state.get(name));
        }
        drop(snapshot);
        drop(names);

        let handles = self.store_handles.borrow();
        let mut store_snapshot = self.store_snapshot.borrow_mut();
        for (index, handle) in handles.iter().enumerate() {
            store_snapshot[index] = self.dispatch.store.read(handle);
        }
    }

    fn seed_owner_seat_from_registry(
        &mut self,
        registry: &EntityRegistry,
        source: Option<EntityId>,
    ) {
        self.owner_seat = source.and_then(|source| registry.seat_for_pawn(source));
    }

    fn bind_state_name(&self, name: &str) -> usize {
        let mut names = self.state_names.borrow_mut();
        if let Some(index) = names.iter().position(|bound| bound == name) {
            return index;
        }
        let index = names.len();
        names.push(name.to_string());
        self.snapshot.borrow_mut().push(0.0);
        index
    }

    fn bind_store_handle(&self, handle: StoreHandle) -> usize {
        let mut handles = self.store_handles.borrow_mut();
        if let Some(index) = handles
            .iter()
            .position(|bound| bound.name == handle.name && bound.ir_type == handle.ir_type)
        {
            return index;
        }
        let index = handles.len();
        let initial = self.dispatch.store.read(&handle);
        handles.push(handle);
        self.store_snapshot.borrow_mut().push(initial);
        index
    }

    fn read_owned_store(&self, handle: &StoreHandle) -> IrValue {
        let table = self.dispatch.store.ctx.slot_table.borrow();
        let Some(record) = table.get(&handle.name) else {
            return handle.ir_type.zero();
        };
        let Some(seat) = self.owner_seat else {
            log::warn!(
                "[Impact] owner read for slot `{}` resolved no seat; using declared default",
                handle.name
            );
            return StoreScope::project_value(handle.ir_type, record.schema.default.as_ref());
        };
        StoreScope::project_value(handle.ir_type, record.per_seat_value(seat))
    }

    /// Inspect a store declaration while binding an owner-addressed impact
    /// command. The owner target is meaningful only for per-owner slots;
    /// ordinary outputs continue through the StoreScope write path.
    pub fn per_owner_store_slot(&self, name: &str) -> Option<bool> {
        self.dispatch.store.per_owner_slot(name)
    }

    /// Owner-addressed impact writes bypass the ordinary output handle, which
    /// is where script capability normally denies readonly slots. Expose the
    /// schema gate explicitly so that alternate write path preserves it.
    pub fn store_slot_is_readonly(&self, name: &str) -> Option<bool> {
        self.dispatch
            .store
            .ctx
            .slot_table
            .borrow()
            .get(name)
            .map(|record| record.schema.readonly)
    }

    fn write_state(
        registry: &mut EntityRegistry,
        target: Option<EntityId>,
        name: &str,
        value: IrValue,
    ) {
        let IrValue::Number(value) = value else {
            return;
        };
        let Some(target) = target else {
            return;
        };

        let Ok(state) = registry.entity_state_mut(target) else {
            return;
        };
        state.set(name, value);
    }

    /// Apply a bound impact output while the caller owns the live registry.
    pub fn write_with_registry(
        &mut self,
        registry: &mut EntityRegistry,
        handle: &EntityOutputHandle,
        value: IrValue,
    ) {
        match handle {
            EntityOutputHandle::State(name) => {
                Self::write_state(registry, self.target, name, value)
            }
            EntityOutputHandle::Store(handle) => self.dispatch.write(handle, value),
        }
    }
}

impl BindingScope for EntityScope {
    type InputHandle = EntityInputHandle;
    type OutputHandle = EntityOutputHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>> {
        if let Some(state_name) = name.strip_prefix(ENTITY_STATE_INPUT_PREFIX) {
            let handle = self.bind_state_name(state_name);
            return Some(ResolvedInput {
                handle: EntityInputHandle::State(handle),
                ir_type: IrType::Number,
            });
        }

        self.dispatch.resolve_input(name).map(|resolved| {
            let handle = match resolved.handle {
                DispatchInputHandle::Dispatch(index) => EntityInputHandle::Dispatch(index),
                DispatchInputHandle::Store(handle) => {
                    EntityInputHandle::Store(self.bind_store_handle(handle))
                }
            };
            ResolvedInput {
                handle,
                ir_type: resolved.ir_type,
            }
        })
    }

    fn resolve_owned_input(
        &self,
        name: &str,
        owner: &str,
    ) -> Option<ResolvedInput<Self::InputHandle>> {
        if owner != IMPACT_SOURCE_TOKEN {
            return None;
        }
        self.dispatch
            .store
            .resolve_owner_input(name)
            .map(|resolved| ResolvedInput {
                handle: EntityInputHandle::OwnedStore(resolved.handle),
                ir_type: resolved.ir_type,
            })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<Self::OutputHandle>> {
        if let Some(state_name) = name.strip_prefix(ENTITY_STATE_INPUT_PREFIX) {
            return Some(ResolvedOutput {
                handle: EntityOutputHandle::State(state_name.to_string()),
                ir_type: IrType::Number,
            });
        }
        // The remaining reserved names are inputs. Impact facts and
        // command-target tokens can never fall through to an oddly named store
        // slot as writable outputs.
        if name.starts_with('@') {
            return None;
        }

        self.dispatch
            .resolve_output(name)
            .map(|resolved| ResolvedOutput {
                handle: EntityOutputHandle::Store(resolved.handle),
                ir_type: resolved.ir_type,
            })
    }

    fn read(&self, handle: &Self::InputHandle) -> IrValue {
        match handle {
            EntityInputHandle::State(index) => {
                IrValue::Number(self.snapshot.borrow().get(*index).copied().unwrap_or(0.0))
            }
            EntityInputHandle::Dispatch(index) => self.dispatch.values[*index],
            EntityInputHandle::Store(index) => self
                .store_snapshot
                .borrow()
                .get(*index)
                .copied()
                .unwrap_or(IrValue::Number(0.0)),
            EntityInputHandle::OwnedStore(handle) => self.read_owned_store(handle),
        }
    }

    fn write(&mut self, handle: &Self::OutputHandle, value: IrValue) {
        match handle {
            EntityOutputHandle::State(name) => {
                let registry = self.registry.clone();
                Self::write_state(&mut registry.borrow_mut(), self.target, name, value);
            }
            EntityOutputHandle::Store(handle) => self.dispatch.write(handle, value),
        }
    }
}

impl BindingScope for StoreScope {
    type InputHandle = StoreHandle;
    type OutputHandle = StoreHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<StoreHandle>> {
        let table = self.ctx.slot_table.borrow();
        let record = table.get(name)?;
        // Bare reads are scalar projections. A per-owner slot has no
        // meaningful scalar owner here, so only the impact scope's explicit
        // owner resolver may bind it.
        if record.schema.per_owner {
            return None;
        }
        let ir_type = Self::project(&record.schema.slot_type)?;
        Some(ResolvedInput {
            handle: StoreHandle {
                name: name.to_string(),
                ir_type,
            },
            ir_type,
        })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<StoreHandle>> {
        let table = self.ctx.slot_table.borrow();
        let record = table.get(name)?;
        // A per-owner value must travel through an owner-addressed command.
        // Returning no ordinary output handle rejects a bare `slot.set` at
        // bind, before it could silently overwrite the scalar projection.
        if record.schema.per_owner {
            return None;
        }
        let ir_type = Self::project(&record.schema.slot_type)?;
        // Script-capability scopes cannot write readonly slots — deny the handle
        // at bind so the write path is never reached for them. Engine scopes
        // bypass readonly, matching `write_store_slot`'s engine-bypass policy.
        if self.mode == StoreCapability::Script && record.schema.readonly {
            return None;
        }
        Some(ResolvedOutput {
            handle: StoreHandle {
                name: name.to_string(),
                ir_type,
            },
            ir_type,
        })
    }

    fn read(&self, handle: &StoreHandle) -> IrValue {
        // Alloc-free re-hash through the existing `get(&str)`; no new store API.
        let table = self.ctx.slot_table.borrow();
        let value = table
            .get(&handle.name)
            .and_then(|record| record.value.as_ref());
        Self::project_value(handle.ir_type, value)
    }

    fn write(&mut self, handle: &StoreHandle, value: IrValue) {
        // Both modes funnel the engine-validated `write_store_slot` (type/range
        // validation, clamp-with-warning). The capability difference is enforced
        // at bind: Script mode never resolves a readonly output, so reaching
        // here means the write is permitted. We deliberately do not duplicate
        // the script-gated readonly *re-check* — bind already denied it, and the
        // typed (Number/Bool) values eval produces are never the non-projectable
        // kinds the script path additionally guards.
        let slot_value = match value {
            IrValue::Number(n) => SlotValue::Number(n),
            IrValue::Bool(b) => SlotValue::Boolean(b),
        };
        // A failed write (unknown slot / type mismatch) cannot arise for a
        // bound handle against a stable table; if it somehow does, the engine
        // path logs and we drop the error rather than panicking per-tick.
        let _ = write_store_slot(&self.ctx, &handle.name, slot_value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eval::{eval_and_write, eval_value};
    use crate::ir::test_scope::{StubScope, StubWrite};
    use crate::ir::{BakedIr, BindError, CURRENT_IR_VERSION, IrNode, bind};
    use crate::slot_table::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use log::Level;
    use postretro_test_log_capture::LogCapture;

    const EPSILON: f32 = 1e-6;
    const TEST_DISPATCH_INPUTS: [(&str, IrType); 2] =
        [("@rising", IrType::Bool), ("@dt", IrType::Number)];

    fn num(v: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(v),
        })
    }

    fn input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
            owner: None,
        })
    }

    fn owned_input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
            owner: Some(IMPACT_SOURCE_TOKEN.to_string()),
        })
    }

    fn read_only(root: IrNode) -> BakedIr {
        BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root,
        }
    }

    fn assert_number(value: IrValue, expected: f32) {
        match value {
            IrValue::Number(actual) => assert!(
                (actual - expected).abs() <= EPSILON,
                "expected {expected}, got {actual}"
            ),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    fn number_slot(value: f32, readonly: bool) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(value)),
            range: Some(NumericRange {
                min: 0.0,
                max: 100.0,
            }),
            persist: false,
            readonly,
            ownership: if readonly {
                SlotOwnership::Engine
            } else {
                SlotOwnership::Mod
            },
            network: crate::slot_table::ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn per_owner_number_slot(value: f32) -> SlotRecord {
        let mut record = number_slot(value, false);
        record.schema.per_owner = true;
        record
    }

    fn bool_slot(value: bool) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Boolean,
            default: Some(SlotValue::Boolean(value)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: crate::slot_table::ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn string_slot() -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::String,
            default: Some(SlotValue::String("x".to_string())),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: crate::slot_table::ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    /// A ctx seeded with: `test.number` (writable, value 25), `test.flag`
    /// (bool true), `test.label` (string — non-projectable), and the built-in
    /// readonly `player.health`.
    fn seeded_ctx() -> ScriptCtx {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert("test.number".to_string(), number_slot(25.0, false))
                .unwrap();
            table
                .insert("test.flag".to_string(), bool_slot(true))
                .unwrap();
            table
                .insert("test.label".to_string(), string_slot())
                .unwrap();
        }
        // Give the readonly engine slot a current value so reads/writes are
        // observable.
        write_store_slot(&ctx, "player.health", SlotValue::Number(50.0)).unwrap();
        ctx
    }

    #[test]
    fn store_scope_projects_number_and_bool_inputs_and_reads_them() {
        let ctx = seeded_ctx();
        let scope = StoreScope::engine(ctx);

        let program = bind(&read_only(*input("test.number")), &scope).expect("number projects");
        assert_eq!(program.root_type, IrType::Number);
        assert_number(eval_value(&program, &scope), 25.0);

        let program = bind(&read_only(*input("test.flag")), &scope).expect("bool projects");
        assert_eq!(eval_value(&program, &scope), IrValue::Bool(true));
    }

    #[test]
    fn store_scope_denies_non_projectable_and_unknown_inputs() {
        let ctx = seeded_ctx();
        let scope = StoreScope::engine(ctx);
        for name in ["test.label", "test.missing"] {
            assert_eq!(
                bind(&read_only(*input(name)), &scope).unwrap_err(),
                BindError::UnknownInput {
                    name: name.to_string()
                }
            );
        }
    }

    #[test]
    fn store_scope_reads_absent_value_as_type_zero() {
        let ctx = ScriptCtx::new();
        let scope = StoreScope::engine(ctx);
        // `player.health` is readonly with no default/value → reads 0.0.
        let program = bind(&read_only(*input("player.health")), &scope).expect("projects");
        assert_number(eval_value(&program, &scope), 0.0);
    }

    #[test]
    fn engine_mode_writes_readonly_slot_through_validated_path() {
        // Engine policy bypasses readonly: it resolves a write handle for a
        // readonly engine-owned slot and writes through the validated path,
        // which range-clamps. The slot carries a known [0, 100] range so the
        // clamp is asserted against a pinned bound.
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("engine.shield".to_string(), number_slot(50.0, true))
            .unwrap();
        let mut scope = StoreScope::engine(ctx.clone());
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("engine.shield".to_string()),
            // 200 exceeds the slot range [0, 100]; the validated path clamps.
            root: *num(200.0),
        };
        let program = bind(&baked, &scope).expect("engine grants readonly write handle");
        eval_and_write(&program, &mut scope);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("engine.shield")
                .and_then(|r| r.value.clone()),
            Some(SlotValue::Number(100.0)),
            "engine write is validated and range-clamped"
        );
    }

    #[test]
    fn script_mode_denies_readonly_output_at_bind() {
        let ctx = seeded_ctx();
        let scope = StoreScope::script(ctx);
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("player.health".to_string()),
            root: *num(10.0),
        };
        assert_eq!(
            bind(&baked, &scope).unwrap_err(),
            BindError::UnknownOutput {
                name: "player.health".to_string()
            },
            "script capability must not grant a readonly write handle"
        );
    }

    #[test]
    fn script_mode_writes_writable_slot() {
        let ctx = seeded_ctx();
        let mut scope = StoreScope::script(ctx.clone());
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("test.number".to_string()),
            root: *num(42.0),
        };
        let program = bind(&baked, &scope).expect("writable slot binds in script mode");
        eval_and_write(&program, &mut scope);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("test.number")
                .and_then(|r| r.value.clone()),
            Some(SlotValue::Number(42.0))
        );
    }

    #[test]
    fn stub_scope_grants_writes_only_for_declared_outputs() {
        // An envelope targeting a granted output binds and writes; one targeting
        // an ungranted output fails to bind (write capability is a bind-time grant).
        let mut scope = StubScope::with_writes(&[("out_number", StubWrite::Number)]);
        let granted = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("out_number".to_string()),
            root: IrNode::Add {
                a: num(1.0),
                b: input("speed"),
            },
        };
        let program = bind(&granted, &scope).expect("granted output binds");
        eval_and_write(&program, &mut scope);
        assert_number(scope.written("out_number").expect("written"), 5.0);

        let denied = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("not_declared".to_string()),
            root: *num(1.0),
        };
        assert_eq!(
            bind(&denied, &scope).unwrap_err(),
            BindError::UnknownOutput {
                name: "not_declared".to_string()
            }
        );
    }

    #[test]
    fn same_tree_binds_against_store_and_stub_scopes() {
        // One IR tree, two scopes with distinct handle types (owned-name vs
        // index), each reading its own `speed` value. The slot table imposes no
        // dotted-name requirement, so inserting under the plain name "speed"
        // makes both scopes resolvable from the identical tree.
        let tree = read_only(IrNode::Add {
            a: input("speed"),
            b: num(1.0),
        });

        // Store scope: declare a `speed` number slot at value 10.
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("speed".to_string(), number_slot(10.0, false))
            .unwrap();
        let store_scope = StoreScope::engine(ctx);
        let store_program = bind(&tree, &store_scope).expect("store binds");
        assert_number(eval_value(&store_program, &store_scope), 11.0);

        // Stub scope: `speed` is 4.0 by construction — same tree, different scope.
        let stub_scope = StubScope::new();
        let stub_program = bind(&tree, &stub_scope).expect("stub binds");
        assert_number(eval_value(&stub_program, &stub_scope), 5.0);
    }

    #[test]
    fn stub_set_input_drives_reads() {
        let mut scope = StubScope::new();
        scope.set_input("speed", IrValue::Number(9.0));
        let program = bind(&read_only(*input("speed")), &scope).expect("binds");
        assert_number(eval_value(&program, &scope), 9.0);
    }

    #[test]
    fn dispatch_scope_binds_reserved_inputs_with_projected_types() {
        let scope = DispatchScope::script(ScriptCtx::new(), &TEST_DISPATCH_INPUTS);

        let rising = bind(&read_only(*input("@rising")), &scope).expect("bool input binds");
        assert_eq!(rising.root_type, IrType::Bool);

        let dt = bind(&read_only(*input("@dt")), &scope).expect("number input binds");
        assert_eq!(dt.root_type, IrType::Number);
    }

    #[test]
    fn dispatch_scope_rejects_unknown_reserved_input_at_bind() {
        let scope = DispatchScope::script(ScriptCtx::new(), &TEST_DISPATCH_INPUTS);

        assert_eq!(
            bind(&read_only(*input("@missing")), &scope).unwrap_err(),
            BindError::UnknownInput {
                name: "@missing".to_string()
            }
        );
    }

    #[test]
    fn dispatch_scope_delegates_ambient_store_reads_and_writes() {
        let ctx = seeded_ctx();
        let mut scope = DispatchScope::script(ctx.clone(), &TEST_DISPATCH_INPUTS);
        scope.seed("@dt", IrValue::Number(2.0)).unwrap();
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("test.number".to_string()),
            root: IrNode::Add {
                a: input("test.number"),
                b: input("@dt"),
            },
        };

        let program = bind(&baked, &scope).expect("dispatch and ambient inputs bind");
        eval_and_write(&program, &mut scope);

        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("test.number")
                .and_then(|record| record.value.clone()),
            Some(SlotValue::Number(27.0))
        );
    }

    #[test]
    fn dispatch_scope_seed_updates_are_observed_without_rebind() {
        let mut scope = DispatchScope::script(ScriptCtx::new(), &TEST_DISPATCH_INPUTS);
        let program = bind(&read_only(*input("@dt")), &scope).expect("binds once");

        scope.seed("@dt", IrValue::Number(0.25)).unwrap();
        assert_number(eval_value(&program, &scope), 0.25);

        scope.seed("@dt", IrValue::Number(0.5)).unwrap();
        assert_number(eval_value(&program, &scope), 0.5);
    }

    #[test]
    fn dispatch_scope_refuses_mismatched_seed_type() {
        let mut scope = DispatchScope::script(ScriptCtx::new(), &TEST_DISPATCH_INPUTS);
        let program = bind(&read_only(*input("@dt")), &scope).expect("binds");
        scope.seed("@dt", IrValue::Number(0.25)).unwrap();

        assert_eq!(
            scope.seed("@dt", IrValue::Bool(true)),
            Err(DispatchSeedError::TypeMismatch {
                name: "@dt",
                expected: IrType::Number,
                actual: IrType::Bool,
            })
        );
        assert_number(eval_value(&program, &scope), 0.25);
    }

    fn set_entity_state(ctx: &ScriptCtx, entity: EntityId, name: &str, value: f32) {
        let mut registry = ctx.registry.borrow_mut();
        let mut state = registry
            .get_component::<EntityStateComponent>(entity)
            .expect("spawned entity carries state")
            .clone();
        state.set(name, value);
        registry
            .set_component(entity, state)
            .expect("entity remains live during test setup");
    }

    fn impact(target: EntityId, amount: f32) -> ImpactDispatch {
        ImpactDispatch {
            amount,
            health_before: 10.0,
            health_after: 5.0,
            max_health: 10.0,
            target,
            source: None,
            producer: crate::components::health::DamageProducer::InTick,
        }
    }

    fn impact_from(target: EntityId, source: EntityId) -> ImpactDispatch {
        ImpactDispatch {
            source: Some(source),
            ..impact(target, 1.0)
        }
    }

    #[test]
    fn entity_scope_routes_exact_prefixes_without_store_collision() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("hits".to_string(), number_slot(40.0, false))
            .expect("test store slot is new");
        let target = ctx
            .registry
            .borrow_mut()
            .spawn(crate::registry::Transform::default());
        set_entity_state(&ctx, target, "hits", 3.0);

        let mut scope = EntityScope::impact(ctx);
        let state = bind(&read_only(*input("@state.hits")), &scope).expect("state binds");
        let store = bind(&read_only(*input("hits")), &scope).expect("store binds");
        let amount = bind(&read_only(*input("@impact.amount")), &scope).expect("fact binds");

        assert_eq!(
            bind(&read_only(*input("@stateful.hits")), &scope).unwrap_err(),
            BindError::UnknownInput {
                name: "@stateful.hits".to_string()
            },
            "only the exact @state. prefix belongs to entity state"
        );
        for name in ["@impact.target", "@impact.source"] {
            assert_eq!(
                bind(&read_only(*input(name)), &scope).unwrap_err(),
                BindError::UnknownInput {
                    name: name.to_string()
                },
                "command-target tokens never become numeric leaves"
            );
        }

        scope
            .seed_impact(&impact(target, 7.0))
            .expect("fixed facts seed");
        assert_number(eval_value(&state, &scope), 3.0);
        assert_number(eval_value(&store, &scope), 40.0);
        assert_number(eval_value(&amount, &scope), 7.0);
    }

    #[test]
    fn entity_scope_refreshes_bound_state_handle_for_each_impact_target() {
        let ctx = ScriptCtx::new();
        let (first, second) = {
            let mut registry = ctx.registry.borrow_mut();
            (
                registry.spawn(crate::registry::Transform::default()),
                registry.spawn(crate::registry::Transform::default()),
            )
        };
        set_entity_state(&ctx, first, "hits", 2.0);

        let mut scope = EntityScope::impact(ctx);
        let program = bind(&read_only(*input("@state.hits")), &scope).expect("state binds once");

        scope
            .seed_impact(&impact(first, 1.0))
            .expect("first fire seeds");
        assert_number(eval_value(&program, &scope), 2.0);

        scope
            .seed_impact(&impact(second, 1.0))
            .expect("second fire seeds");
        assert_number(eval_value(&program, &scope), 0.0);
    }

    #[test]
    fn entity_scope_reads_the_pre_write_state_snapshot_until_the_next_fire() {
        let ctx = ScriptCtx::new();
        let target = ctx
            .registry
            .borrow_mut()
            .spawn(crate::registry::Transform::default());
        set_entity_state(&ctx, target, "hits", 2.0);

        let mut scope = EntityScope::impact(ctx.clone());
        let increment = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some("@state.hits".to_string()),
            root: IrNode::Add {
                a: input("@state.hits"),
                b: num(1.0),
            },
        };
        let writer = bind(&increment, &scope).expect("state output binds");
        let reader = bind(&read_only(*input("@state.hits")), &scope).expect("state read binds");

        scope.seed_impact(&impact(target, 1.0)).expect("fire seeds");
        assert_number(eval_and_write(&writer, &mut scope), 3.0);
        assert_number(eval_value(&reader, &scope), 2.0);
        assert_number(
            IrValue::Number(
                ctx.registry
                    .borrow()
                    .get_component::<EntityStateComponent>(target)
                    .expect("target remains live")
                    .get("hits"),
            ),
            3.0,
        );

        scope
            .seed_impact(&impact(target, 1.0))
            .expect("next fire seeds");
        assert_number(eval_value(&reader, &scope), 3.0);
    }

    #[test]
    fn entity_scope_reads_store_snapshot_until_the_next_fire() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("impact.counter".to_string(), number_slot(2.0, false))
            .expect("test store slot is new");
        let target = ctx
            .registry
            .borrow_mut()
            .spawn(crate::registry::Transform::default());

        let mut scope = EntityScope::impact(ctx.clone());
        let reader = bind(&read_only(*input("impact.counter")), &scope).expect("store binds");

        scope.seed_impact(&impact(target, 1.0)).expect("fire seeds");
        ctx.slot_table
            .borrow_mut()
            .get_mut("impact.counter")
            .expect("test store slot remains present")
            .value = Some(SlotValue::Number(3.0));
        assert_number(eval_value(&reader, &scope), 2.0);

        scope
            .seed_impact(&impact(target, 1.0))
            .expect("next fire seeds");
        assert_number(eval_value(&reader, &scope), 3.0);
    }

    #[test]
    fn entity_scope_reads_owned_store_from_the_source_seat_live_value() {
        let ctx = ScriptCtx::new();
        let mut record = per_owner_number_slot(5.0);
        record.value = Some(SlotValue::Number(91.0));
        record.set_per_seat_value(Seat(1), SlotValue::Number(17.0));
        record.set_per_seat_value(Seat(2), SlotValue::Number(31.0));
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".to_string(), record)
            .expect("test store slot is new");

        let (target, source) = {
            let mut registry = ctx.registry.borrow_mut();
            let target = registry.spawn(crate::registry::Transform::default());
            let source = registry.spawn(crate::registry::Transform::default());
            registry.bind_pawn_seat(source, Seat(1));
            (target, source)
        };

        let mut scope = EntityScope::impact(ctx.clone());
        let reader = bind(&read_only(*owned_input("currency.xp")), &scope)
            .expect("impact source owns a per-owner input");
        scope
            .seed_impact(&impact_from(target, source))
            .expect("impact fire seeds");

        assert_number(eval_value(&reader, &scope), 17.0);
        ctx.slot_table
            .borrow_mut()
            .get_mut("currency.xp")
            .expect("slot remains present")
            .set_per_seat_value(Seat(1), SlotValue::Number(23.0));
        assert_number(eval_value(&reader, &scope), 23.0);
    }

    #[test]
    fn entity_scope_owner_read_without_a_source_seat_uses_default_and_warns() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".to_string(), per_owner_number_slot(5.0))
            .expect("test store slot is new");
        let (target, source) = {
            let mut registry = ctx.registry.borrow_mut();
            (
                registry.spawn(crate::registry::Transform::default()),
                registry.spawn(crate::registry::Transform::default()),
            )
        };
        let mut scope = EntityScope::impact(ctx);
        let reader = bind(&read_only(*owned_input("currency.xp")), &scope)
            .expect("per-owner input binds in impact scope");
        scope
            .seed_impact(&impact_from(target, source))
            .expect("impact fire seeds");

        let capture = LogCapture::start();
        assert_number(eval_value(&reader, &scope), 5.0);
        capture.assert_logged_once(
            Level::Warn,
            "owner read for slot `currency.xp` resolved no seat; using declared default",
        );
    }

    #[test]
    fn store_read_binders_reject_unaddressed_per_owner_and_addressed_global_slots() {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert("currency.xp".to_string(), per_owner_number_slot(0.0))
                .expect("per-owner slot is new");
            table
                .insert("currency.team".to_string(), number_slot(0.0, false))
                .expect("global slot is new");
        }
        let impact_scope = EntityScope::impact(ctx.clone());

        assert_eq!(
            bind(&read_only(*input("currency.xp")), &impact_scope).unwrap_err(),
            BindError::UnknownInput {
                name: "currency.xp".to_string(),
            },
            "a bare read must name no implicit owner"
        );
        assert_eq!(
            bind(&read_only(*owned_input("currency.team")), &impact_scope).unwrap_err(),
            BindError::UnknownInput {
                name: "currency.team".to_string(),
            },
            "an owner token cannot address a global slot"
        );
    }

    #[test]
    fn sourceless_dispatch_scope_refuses_owner_addressed_store_reads() {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert("currency.xp".to_string(), per_owner_number_slot(0.0))
            .expect("per-owner slot is new");
        let scope = DispatchScope::script(ctx, &TEST_DISPATCH_INPUTS);

        assert_eq!(
            bind(&read_only(*owned_input("currency.xp")), &scope).unwrap_err(),
            BindError::UnknownInput {
                name: "currency.xp".to_string(),
            }
        );
    }
}
