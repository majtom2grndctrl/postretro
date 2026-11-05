// Bound defense-layer parameters: each expression binds once per descriptor
// and field, at install and hot reload; a literal binds to a constant and is
// never evaluated. Spawn, grants, respawn and reload evaluate on demand, and
// the tick-start step evaluates every layered entity.
// See: context/lib/scripting.md §11 (Defense layers)

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use postretro_entities::ScriptCtx;
use postretro_entities::components::defense_layers::{
    DefenseLayersComponent, DefenseParameterSource, Resist,
};
use postretro_entities::components::entity_state::EntityStateComponent;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_entities::slot_table::{SlotTable, SlotValue};
use postretro_foundation::defense::{
    LAYER_CAPACITY_RANGE, LAYER_DELAY_MS_RANGE, LAYER_RATE_RANGE, MULTIPLIER_RANGE, clamp_parameter,
};
use postretro_foundation::ir::{
    BakedIr, BindingScope, BoundProgram, CURRENT_IR_VERSION, IrNode, IrValue, ResolvedInput,
    ResolvedOutput, bind, eval_value_checked,
};
use postretro_foundation::{HealthDescriptor, NumberOrIr, Seat};
use postretro_scripting_core::ir_layer_validation::{
    LayerRead, LayerScopeEntity, classify_layer_read,
};
use postretro_scripting_core::ir_scopes::StoreScope;

#[cfg(test)]
thread_local! {
    /// Expression evaluations, for the per-tick cost tests.
    pub(crate) static EVALUATIONS: Cell<usize> = const { Cell::new(0) };
}

/// One read a bound layer expression makes.
#[derive(Clone, Debug)]
pub enum LayerInput {
    /// Index into the scope's per-entity state values.
    State(usize),
    /// Index into the scope's store slot names, read globally.
    Global(usize),
    /// Index into the scope's store slot names, read for the entity's seat.
    PerOwner(usize),
}

/// The live layer-parameter scope: global store values, the evaluated
/// entity's own `@state.*` and, on the player, its seat's per-owner values.
/// Seeded once per entity before its evaluations; reads allocate nothing.
pub struct LayerScope {
    slot_table: Rc<RefCell<SlotTable>>,
    entity: Cell<LayerScopeEntity>,
    state_names: RefCell<Vec<String>>,
    state_values: RefCell<Vec<f32>>,
    slot_names: RefCell<Vec<String>>,
    seat: Cell<Option<Seat>>,
    /// A read found no value this evaluation: the result is a fault.
    missing: Cell<bool>,
    /// A per-owner read on a pawn no seat owns yet: not a fault. The value
    /// waits for the seat bind, which evaluates again.
    unseated: Cell<bool>,
    /// Set while binding when the expression reads the entity's own state or
    /// a player's per-owner value: it cannot share one result per descriptor.
    binding_per_entity: Cell<bool>,
}

/// One expression's outcome.
enum Evaluated {
    Value(f32),
    /// A missing read or non-finite arithmetic: keep the last good value.
    Fault,
    /// The pawn has no seat yet: keep the value in force, silently.
    Unseated,
}

impl LayerScope {
    fn new(slot_table: Rc<RefCell<SlotTable>>) -> Self {
        Self {
            slot_table,
            entity: Cell::new(LayerScopeEntity::Other),
            state_names: RefCell::new(Vec::new()),
            state_values: RefCell::new(Vec::new()),
            slot_names: RefCell::new(Vec::new()),
            seat: Cell::new(None),
            missing: Cell::new(false),
            unseated: Cell::new(false),
            binding_per_entity: Cell::new(false),
        }
    }

    fn intern(names: &RefCell<Vec<String>>, name: &str) -> usize {
        let mut names = names.borrow_mut();
        names
            .iter()
            .position(|bound| bound == name)
            .unwrap_or_else(|| {
                names.push(name.to_string());
                names.len() - 1
            })
    }

    fn seed(&self, registry: &EntityRegistry, entity: EntityId) {
        self.seat.set(registry.seat_for_pawn(entity));
        let state = registry.get_component::<EntityStateComponent>(entity).ok();
        let names = self.state_names.borrow();
        let mut values = self.state_values.borrow_mut();
        for (value, name) in values.iter_mut().zip(names.iter()) {
            *value = state.map_or(0.0, |state| state.get(name));
        }
    }

    fn slot_value(&self, value: Option<&SlotValue>) -> IrValue {
        match value {
            Some(SlotValue::Number(value)) => IrValue::Number(*value),
            Some(SlotValue::Boolean(value)) => IrValue::Bool(*value),
            _ => {
                self.missing.set(true);
                IrValue::Number(0.0)
            }
        }
    }

    /// One expression's outcome: its value, a fault (a missing read or
    /// non-finite arithmetic), or a per-owner read waiting on a seat.
    fn evaluate(&self, program: &BoundProgram<LayerScope>) -> Evaluated {
        #[cfg(test)]
        EVALUATIONS.with(|count| count.set(count.get() + 1));
        self.missing.set(false);
        self.unseated.set(false);
        let result = eval_value_checked(program, self);
        if self.unseated.get() {
            return Evaluated::Unseated;
        }
        match result {
            Ok(IrValue::Number(value)) if value.is_finite() && !self.missing.get() => {
                Evaluated::Value(value)
            }
            _ => Evaluated::Fault,
        }
    }
}

impl BindingScope for LayerScope {
    type InputHandle = LayerInput;
    type OutputHandle = ();

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<LayerInput>> {
        let table = self.slot_table.borrow();
        let (read, ir_type) = classify_layer_read(self.entity.get(), name, |slot| {
            table.get(slot).map(|record| {
                (
                    StoreScope::project(&record.schema.slot_type),
                    record.schema.per_owner,
                )
            })
        })
        .ok()?;
        if !matches!(read, LayerRead::Global) {
            self.binding_per_entity.set(true);
        }
        let handle = match read {
            LayerRead::State(state) => {
                let index = Self::intern(&self.state_names, &state);
                let mut values = self.state_values.borrow_mut();
                if values.len() <= index {
                    values.resize(index + 1, 0.0);
                }
                LayerInput::State(index)
            }
            LayerRead::Global => LayerInput::Global(Self::intern(&self.slot_names, name)),
            LayerRead::PerOwner => LayerInput::PerOwner(Self::intern(&self.slot_names, name)),
        };
        Some(ResolvedInput { handle, ir_type })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<()>> {
        None
    }

    fn read(&self, handle: &LayerInput) -> IrValue {
        match handle {
            LayerInput::State(index) => IrValue::Number(
                self.state_values
                    .borrow()
                    .get(*index)
                    .copied()
                    .unwrap_or(0.0),
            ),
            LayerInput::Global(index) => {
                let names = self.slot_names.borrow();
                let table = self.slot_table.borrow();
                self.slot_value(
                    table
                        .get(&names[*index])
                        .and_then(|record| record.value.as_ref()),
                )
            }
            LayerInput::PerOwner(index) => {
                let Some(seat) = self.seat.get() else {
                    self.unseated.set(true);
                    return IrValue::Number(0.0);
                };
                let names = self.slot_names.borrow();
                let table = self.slot_table.borrow();
                self.slot_value(
                    table
                        .get(&names[*index])
                        .and_then(|record| record.per_seat_value(seat)),
                )
            }
        }
    }

    fn write(&mut self, _handle: &(), _value: IrValue) {}
}

enum BoundValue {
    Literal,
    Expr {
        program: BoundProgram<LayerScope>,
        /// `descriptor `name` key.path`, for the one fault warning.
        label: String,
        warned: Cell<bool>,
        /// Reads only global slots and constants, so one tick-start result
        /// serves every entity of the descriptor.
        global: bool,
        /// That shared result: the host tick it was evaluated on, and the
        /// clamped value or `None` for a fault.
        shared: Cell<Option<(u64, Option<f32>)>>,
    },
}

struct BoundLayer {
    name: String,
    capacity: BoundValue,
    delay: BoundValue,
    rate: BoundValue,
    resist: Vec<(String, BoundValue)>,
}

struct BoundDescriptor {
    layers: Vec<BoundLayer>,
    health_resist: Vec<(String, BoundValue)>,
    /// Every parameter literal: nothing to evaluate.
    literal: bool,
}

/// The host's bound layer parameters, keyed by descriptor canonical name.
pub struct DefenseParameters {
    /// The host / single-player role gate (`owner_slot_writes_enabled`): a
    /// connected client evaluates no layer parameter.
    authoritative: Rc<Cell<bool>>,
    scope: LayerScope,
    entries: HashMap<String, BoundDescriptor>,
}

impl DefenseParameters {
    /// Bind every layer parameter of the active descriptors. Binding is
    /// role-free; evaluation reads the role gate on every call.
    pub fn bind(ctx: &ScriptCtx) -> Self {
        let scope = LayerScope::new(ctx.slot_table.clone());
        let mut entries = HashMap::new();
        let data_registry = ctx.data_registry.borrow();
        for descriptor in &data_registry.entities {
            let (Some(name), Some(health)) = (
                descriptor.canonical_name.as_deref(),
                descriptor.health.as_ref(),
            ) else {
                continue;
            };
            if health.layers.is_empty() && health.resist.is_empty() {
                continue;
            }
            scope.entity.set(if descriptor.movement.is_some() {
                LayerScopeEntity::Player
            } else {
                LayerScopeEntity::Other
            });
            entries.insert(name.to_string(), bind_descriptor(&scope, name, health));
        }
        Self {
            authoritative: ctx.owner_slot_writes_enabled.clone(),
            scope,
            entries,
        }
    }

    /// Bind and install the evaluator on the registry, replacing any earlier
    /// one and re-arming its fault warnings.
    pub fn install(ctx: &ScriptCtx) {
        let source: Rc<dyn DefenseParameterSource> = Rc::new(Self::bind(ctx));
        ctx.registry
            .borrow_mut()
            .set_defense_parameters(Some(source));
    }

    /// Evaluate one parameter into `target`: a good result, clamped, replaces
    /// it; a fault keeps it and warns once for the descriptor and field.
    /// `shared_tick` is the host tick when the tick-start step calls: a
    /// global-only expression then evaluates once that tick and every later
    /// entity of the descriptor reads the same result.
    fn evaluate_into(
        &self,
        value: &BoundValue,
        range: (f32, f32),
        target: &mut f32,
        shared_tick: Option<u64>,
    ) {
        let BoundValue::Expr {
            program,
            label,
            warned,
            global,
            shared,
        } = value
        else {
            return;
        };
        let tick = shared_tick.filter(|_| *global);
        let result = match (tick, shared.get()) {
            (Some(tick), Some((filled, result))) if filled == tick => result,
            _ => {
                let result = match self.scope.evaluate(program) {
                    Evaluated::Unseated => return,
                    Evaluated::Value(value) => clamp_parameter(value, range),
                    Evaluated::Fault => None,
                };
                if let Some(tick) = tick {
                    shared.set(Some((tick, result)));
                }
                result
            }
        };
        match result {
            Some(value) => *target = value,
            None => {
                if !warned.replace(true) {
                    log::warn!(
                        "[Defense] {label} faulted with a missing value or non-finite result; each entity keeps its last good value"
                    );
                }
            }
        }
    }

    fn evaluate_resists(
        &self,
        bound: &[(String, BoundValue)],
        resists: &mut [Resist],
        shared_tick: Option<u64>,
    ) {
        for (damage_type, value) in bound {
            if let Some(resist) = resists
                .iter_mut()
                .find(|resist| resist.damage_type == *damage_type)
            {
                self.evaluate_into(value, MULTIPLIER_RANGE, &mut resist.value, shared_tick);
            }
        }
    }

    /// Evaluate every expression parameter of `component` for `entity`.
    /// `shared_tick` is set only by the tick-start step.
    fn evaluate_with(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
        shared_tick: Option<u64>,
    ) {
        if !self.authoritative.get() {
            return;
        }
        let Some(bound) = component
            .descriptor
            .as_deref()
            .and_then(|name| self.entries.get(name))
        else {
            return;
        };
        if bound.literal {
            return;
        }
        self.scope.seed(registry, entity);
        for layer in &mut component.layers {
            let Some(bound_layer) = bound.layers.iter().find(|bound| bound.name == layer.name)
            else {
                continue;
            };
            self.evaluate_into(
                &bound_layer.capacity,
                LAYER_CAPACITY_RANGE,
                &mut layer.capacity,
                shared_tick,
            );
            self.evaluate_into(
                &bound_layer.delay,
                LAYER_DELAY_MS_RANGE,
                &mut layer.recharge_delay_ms,
                shared_tick,
            );
            self.evaluate_into(
                &bound_layer.rate,
                LAYER_RATE_RANGE,
                &mut layer.recharge_rate,
                shared_tick,
            );
            self.evaluate_resists(&bound_layer.resist, &mut layer.resist, shared_tick);
        }
        self.evaluate_resists(
            &bound.health_resist,
            &mut component.health_resist,
            shared_tick,
        );
    }
}

impl DefenseParameterSource for DefenseParameters {
    fn evaluate(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
    ) {
        self.evaluate_with(registry, entity, component, None);
    }

    fn evaluate_at_tick_start(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
    ) {
        self.evaluate_with(registry, entity, component, Some(registry.host_tick()));
    }

    fn evaluate_capacity(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
        component: &mut DefenseLayersComponent,
        index: usize,
    ) {
        if !self.authoritative.get() {
            return;
        }
        let Some(bound) = component
            .descriptor
            .as_deref()
            .and_then(|name| self.entries.get(name))
        else {
            return;
        };
        let Some(layer) = component.layers.get_mut(index) else {
            return;
        };
        let Some(bound_layer) = bound.layers.iter().find(|bound| bound.name == layer.name) else {
            return;
        };
        if matches!(bound_layer.capacity, BoundValue::Literal) {
            return;
        }
        self.scope.seed(registry, entity);
        self.evaluate_into(
            &bound_layer.capacity,
            LAYER_CAPACITY_RANGE,
            &mut layer.capacity,
            None,
        );
    }
}

fn bind_value(scope: &LayerScope, label: String, value: &NumberOrIr) -> BoundValue {
    match value {
        NumberOrIr::Literal(_) => BoundValue::Literal,
        NumberOrIr::Ir(node) => match {
            scope.binding_per_entity.set(false);
            bind_node(scope, node)
        } {
            Ok(program) => BoundValue::Expr {
                program,
                label,
                warned: Cell::new(false),
                global: !scope.binding_per_entity.get(),
                shared: Cell::new(None),
            },
            // Load validation refused such content, so this is a defect.
            Err(reason) => {
                log::warn!(
                    "[Defense] {label} did not bind: {reason}; it keeps its literal default"
                );
                BoundValue::Literal
            }
        },
    }
}

fn bind_node(scope: &LayerScope, node: &IrNode) -> Result<BoundProgram<LayerScope>, String> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    bind(&baked, scope).map_err(|error| error.to_string())
}

fn bind_descriptor(scope: &LayerScope, name: &str, health: &HealthDescriptor) -> BoundDescriptor {
    let label = |path: String| format!("descriptor `{name}` `{path}`");
    let mut literal = true;
    let mut track = |value: BoundValue| {
        if matches!(value, BoundValue::Expr { .. }) {
            literal = false;
        }
        value
    };
    let layers = health
        .layers
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let path = format!("components.health.layers[{index}]");
            let optional = |field: &str, value: Option<&NumberOrIr>| match value {
                Some(value) => bind_value(scope, label(format!("{path}.{field}")), value),
                None => BoundValue::Literal,
            };
            BoundLayer {
                name: entry
                    .layer
                    .handle("layer")
                    .map(|handle| handle.name().to_string())
                    .unwrap_or_default(),
                capacity: track(bind_value(
                    scope,
                    label(format!("{path}.capacity")),
                    &entry.capacity,
                )),
                delay: track(optional(
                    "rechargeDelayMs",
                    entry.recharge_delay_ms.as_ref(),
                )),
                rate: track(optional("rechargeRate", entry.recharge_rate.as_ref())),
                resist: entry
                    .resist
                    .iter()
                    .map(|(ty, value)| {
                        (
                            ty.clone(),
                            track(bind_value(
                                scope,
                                label(format!("{path}.resist.{ty}")),
                                value,
                            )),
                        )
                    })
                    .collect(),
            }
        })
        .collect();
    let health_resist = health
        .resist
        .iter()
        .map(|(ty, value)| {
            (
                ty.clone(),
                track(bind_value(
                    scope,
                    label(format!("components.health.resist.{ty}")),
                    value,
                )),
            )
        })
        .collect();
    BoundDescriptor {
        layers,
        health_resist,
        literal,
    }
}
