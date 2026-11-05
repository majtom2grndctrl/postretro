// Impact-policy composite scope: impact facts, the target's per-entity state and
// the impact source's per-owner reads, layered over a `DispatchScope`.
// Governing contract: `context/lib/scripting.md` §11 (Impact policies).

use super::*;

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
    /// An engine per-player slot of the impact source, from the fire's
    /// snapshot of that pawn.
    OwnedEngine(PlayerSlot),
    /// `source.inLifeState(name)`: the resolved id against the source pawn's
    /// life state from the fire's snapshot.
    OwnedLifeState(u8),
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
    /// The impact source's engine per-player values, refreshed with
    /// `owner_seat`. A source that is not a player pawn (an NPC damager, a
    /// pawnless hazard) has none.
    owner_engine: [Option<IrValue>; PlayerSlot::ALL.len()],
    /// The impact source pawn's life-state id, refreshed with `owner_seat`;
    /// none for a source that is not a player pawn.
    owner_life_state: Option<u8>,
    /// Engine owner reads already warned about, one bit per
    /// `PlayerSlot::index()`: an absent value warns once per scope, not once
    /// per hit.
    warned_owned_engine: std::cell::Cell<u32>,
    /// Per-owner store reads already warned about for a seatless source.
    warned_owned_store: RefCell<Vec<String>>,
}

// `warned_owned_engine` holds one bit per engine per-player slot.
const _: () = assert!(PlayerSlot::ALL.len() <= u32::BITS as usize);

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
            owner_engine: [None; PlayerSlot::ALL.len()],
            owner_life_state: None,
            warned_owned_engine: std::cell::Cell::new(0),
            warned_owned_store: RefCell::new(Vec::new()),
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
        // `byPlayer(impact.source)` names a player: an NPC damager's own
        // components are never "the source player's" value.
        let player_source = source.filter(|source| is_player_pawn(registry, *source));
        self.owner_life_state =
            player_source.and_then(|source| player_life_state(registry, source));
        for slot in PlayerSlot::ALL {
            self.owner_engine[slot.index()] = player_source.and_then(|source| {
                StoreScope::project_slot_value(
                    slot.ir_type(),
                    player_slot_value(registry, *slot, source).as_ref(),
                )
            });
        }
    }

    fn read_owned_engine(&self, slot: PlayerSlot) -> IrValue {
        self.owner_engine[slot.index()].unwrap_or_else(|| {
            let bit = 1u32 << slot.index();
            let warned = self.warned_owned_engine.get();
            if warned & bit == 0 {
                self.warned_owned_engine.set(warned | bit);
                log::warn!(
                    "[Impact] owner read for slot `{}`: the impact source is not a player or has no value; using zero",
                    slot.name()
                );
            }
            slot.ir_type().zero()
        })
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
            let mut warned = self.warned_owned_store.borrow_mut();
            if !warned.contains(&handle.name) {
                warned.push(handle.name.clone());
                log::warn!(
                    "[Impact] owner read for slot `{}` resolved no seat; using declared default",
                    handle.name
                );
            }
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

    /// Whether the mod-wide life-state name table holds `name`. Unlogged, so
    /// the binding caller's error is the one report of an unknown name.
    pub fn declares_life_state(&self, name: &str) -> bool {
        life_state_name_declared(&self.dispatch.store.ctx, name)
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

        self.dispatch.resolve_input(name).and_then(|resolved| {
            let handle = match resolved.handle {
                DispatchInputHandle::Dispatch(index) => EntityInputHandle::Dispatch(index),
                DispatchInputHandle::Store(handle) => {
                    EntityInputHandle::Store(self.bind_store_handle(handle))
                }
                // The impact scope's dispatch layer is never a player-event
                // scope, so it resolves no event-player handle.
                DispatchInputHandle::EventPlayer(_) => return None,
            };
            Some(ResolvedInput {
                handle,
                ir_type: resolved.ir_type,
            })
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
        if let Some(state) = name.strip_prefix(LIFE_STATE_INPUT_PREFIX) {
            let id = resolve_life_state_id(&self.dispatch.store.ctx, state)?;
            return Some(ResolvedInput {
                handle: EntityInputHandle::OwnedLifeState(id),
                ir_type: IrType::Bool,
            });
        }
        if let Some(slot) = PlayerSlot::from_name(name) {
            return Some(ResolvedInput {
                handle: EntityInputHandle::OwnedEngine(slot),
                ir_type: slot.ir_type(),
            });
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
            EntityInputHandle::OwnedEngine(slot) => self.read_owned_engine(*slot),
            // A non-player source holds no state: false, and unwarned.
            EntityInputHandle::OwnedLifeState(id) => {
                IrValue::Bool(self.owner_life_state == Some(*id))
            }
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
