// Value-free scope for defense-layer parameter expressions, and the load gate
// that binds every one against it before a mod commits.
// See: context/lib/scripting.md §11 (Defense layers)

// A layer parameter reads global store values, the entity's own `@state.*`,
// and, on the player entity only, its seat's per-owner store values, all as
// plain reads. Engine per-player slots, layer reads (`@defense.*`, the
// player's `player.defense.*` pair) and every other `@` fact are refused, so a
// bad expression fails load at its authored key path.

use std::cell::RefCell;
use std::collections::BTreeSet;

use postretro_entities::EntityTypeDescriptor;
use postretro_foundation::NumberOrIr;

use crate::ir::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use crate::ir::{BakedIr, BindError, CURRENT_IR_VERSION, IrNode, IrType, IrValue, bind};
use crate::ir_pair_scope::PairSchema;
use crate::ir_pair_validation::IncomingSchema;
use crate::slot_table::{SlotTable, StoreDeclarationSet};

/// Prefix of a per-entity state read.
const STATE_PREFIX: &str = "@state.";

/// Which entity a layer parameter belongs to: per-owner reads need a seat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerScopeEntity {
    Player,
    Other,
}

/// What one name in a layer parameter reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerRead {
    /// The entity's own `@state.<name>`.
    State(String),
    /// A global store slot.
    Global,
    /// A per-owner store slot, read for the player's seat.
    PerOwner,
}

/// Classify one plain read in a layer parameter, or say why it is refused.
pub fn classify_layer_read(
    entity: LayerScopeEntity,
    name: &str,
    store_record: impl FnOnce(&str) -> Option<(Option<IrType>, bool)>,
) -> Result<(LayerRead, IrType), String> {
    if let Some(state) = name.strip_prefix(STATE_PREFIX) {
        return Ok((LayerRead::State(state.to_string()), IrType::Number));
    }
    if name.starts_with('@') {
        return Err(format!("`{name}` is not readable in a layer parameter"));
    }
    if crate::player_slots::PlayerSlot::from_name(name).is_some()
        || crate::player_slots::defense_slot(name).is_some()
        || name.starts_with(crate::life_state_inputs::LIFE_STATE_INPUT_PREFIX)
    {
        return Err(format!(
            "`{name}` is an engine per-player slot, which a layer parameter cannot read"
        ));
    }
    let Some((ir_type, per_owner)) = store_record(name) else {
        return Err(format!("`{name}` names no store slot"));
    };
    let Some(ir_type) = ir_type else {
        return Err(format!("`{name}` is neither a number nor a boolean slot"));
    };
    if per_owner {
        if entity != LayerScopeEntity::Player {
            return Err(format!(
                "per-owner slot `{name}` is readable only in a player entity's layer parameter"
            ));
        }
        return Ok((LayerRead::PerOwner, ir_type));
    }
    Ok((LayerRead::Global, ir_type))
}

/// The value-free, read-only layer-parameter scope. A refused read records
/// why, so the gate can name it at the authored key path.
pub struct LayerValidationScope<'a> {
    entity: LayerScopeEntity,
    schema: IncomingSchema<'a>,
    error: RefCell<Option<String>>,
}

impl<'a> LayerValidationScope<'a> {
    pub fn new(
        entity: LayerScopeEntity,
        declarations: &'a StoreDeclarationSet,
        live: &'a SlotTable,
        life_state_names: &'a BTreeSet<&'a str>,
    ) -> Self {
        Self {
            entity,
            schema: IncomingSchema {
                declarations,
                live,
                life_state_names,
            },
            error: RefCell::new(None),
        }
    }

    /// Bind one layer expression: every read through the layer read table and
    /// a Number root. The depth and node caps were applied at parse.
    pub fn validate_expression(&self, node: &IrNode) -> Result<(), String> {
        self.error.borrow_mut().take();
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: node.clone(),
        };
        match bind(&baked, self) {
            Ok(program) if program.root_type == IrType::Number => Ok(()),
            Ok(_) => Err("expression must produce a number, not a boolean".to_string()),
            Err(error @ BindError::UnknownInput { .. }) => Err(self
                .error
                .borrow_mut()
                .take()
                .unwrap_or_else(|| error.to_string())),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl BindingScope for LayerValidationScope<'_> {
    type InputHandle = IrType;
    type OutputHandle = IrType;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<IrType>> {
        match classify_layer_read(self.entity, name, |slot| self.schema.store_record(slot)) {
            Ok((_, ir_type)) => Some(ResolvedInput {
                handle: ir_type,
                ir_type,
            }),
            Err(reason) => {
                self.error.borrow_mut().get_or_insert(reason);
                None
            }
        }
    }

    fn resolve_owned_input(&self, name: &str, _owner: &str) -> Option<ResolvedInput<IrType>> {
        self.error.borrow_mut().get_or_insert(format!(
            "`{name}` is addressed with byPlayer; a layer parameter reads its own entity's values"
        ));
        None
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<IrType>> {
        None
    }

    fn read(&self, handle: &IrType) -> IrValue {
        handle.zero()
    }

    fn write(&mut self, _handle: &IrType, _value: IrValue) {
        unreachable!("LayerValidationScope is read-only")
    }
}

/// Every expression parameter of one `health` block, with its key path.
pub fn layer_expressions(
    health: &postretro_foundation::HealthDescriptor,
) -> impl Iterator<Item = (String, &IrNode)> {
    let mut out: Vec<(String, &IrNode)> = Vec::new();
    for (index, entry) in health.layers.iter().enumerate() {
        let path = format!("components.health.layers[{index}]");
        if let NumberOrIr::Ir(node) = &entry.capacity {
            out.push((format!("{path}.capacity"), node));
        }
        if let Some(NumberOrIr::Ir(node)) = &entry.recharge_delay_ms {
            out.push((format!("{path}.rechargeDelayMs"), node));
        }
        if let Some(NumberOrIr::Ir(node)) = &entry.recharge_rate {
            out.push((format!("{path}.rechargeRate"), node));
        }
        for (key, value) in &entry.resist {
            if let NumberOrIr::Ir(node) = value {
                out.push((format!("{path}.resist.{key}"), node));
            }
        }
    }
    for (key, value) in &health.resist {
        if let NumberOrIr::Ir(node) = value {
            out.push((format!("components.health.resist.{key}"), node));
        }
    }
    out.into_iter()
}

/// Bind every layer-parameter expression of an incoming descriptor list. A
/// descriptor with `movement` is the player entity, whose expressions may read
/// its seat's per-owner values. The first refusal names the descriptor and the
/// authored key path.
pub fn validate_layer_expressions(
    descriptors: &[EntityTypeDescriptor],
    declarations: &StoreDeclarationSet,
    live: &SlotTable,
) -> Result<(), String> {
    let names = crate::ir_pair_validation::mod_life_state_names(descriptors);
    for descriptor in descriptors {
        let Some(health) = descriptor.health.as_ref() else {
            continue;
        };
        let entity = if descriptor.movement.is_some() {
            LayerScopeEntity::Player
        } else {
            LayerScopeEntity::Other
        };
        let scope = LayerValidationScope::new(entity, declarations, live, &names);
        for (path, node) in layer_expressions(health) {
            scope.validate_expression(node).map_err(|reason| {
                let label = match descriptor.canonical_name.as_deref() {
                    Some(name) => format!("descriptor `{name}`"),
                    None => "an unnamed descriptor".to_string(),
                };
                format!("{label} `{path}`: {reason}")
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot_table::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_foundation::defense::HandleRef;
    use postretro_foundation::{AuthoredHandle, DefenseLayerEntry, HealthDescriptor, IrValue};

    fn table() -> SlotTable {
        let mut table = SlotTable::new();
        for (name, per_owner) in [("tune.cap", false), ("seat.cap", true)] {
            table
                .insert(
                    name.to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(1.0)),
                        range: Some(NumericRange {
                            min: 0.0,
                            max: 1.0e6,
                        }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner,
                        accumulate: None,
                    }),
                )
                .unwrap();
        }
        table.replace_player_defense_slots(&["shield".to_string()]);
        table
    }

    fn input(name: &str) -> IrNode {
        IrNode::Input {
            name: name.to_string(),
            owner: None,
        }
    }

    fn descriptor(player: bool, capacity: IrNode, resist: Option<IrNode>) -> EntityTypeDescriptor {
        let mut health = HealthDescriptor {
            max: 10.0,
            layers: vec![DefenseLayerEntry {
                layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
                    name: "shield".to_string(),
                }),
                capacity: NumberOrIr::Ir(capacity),
                recharge_delay_ms: None,
                recharge_rate: None,
                resist: Default::default(),
            }],
            ..Default::default()
        };
        if let Some(resist) = resist {
            health
                .resist
                .insert("shock".to_string(), NumberOrIr::Ir(resist));
        }
        let movement: Option<postretro_foundation::PlayerMovementDescriptor> = player.then(|| {
            serde_json::from_value(serde_json::json!({
                "capsule": { "radius": 0.4, "halfHeight": 0.8, "eyeHeight": 0.5 },
                "ground": { "speed": { "walk": 7.0, "run": 11.0, "crouch": 3.0 }, "accel": 10.0, "friction": 6.0, "maxSlope": 45.0, "stepHeight": 0.3 },
                "air": { "accel": 1.0, "maxSpeed": 1.0 },
                "fall": { "gravity": 20.0, "terminalVelocity": 50.0 }
            }))
            .ok()
        }).flatten();
        EntityTypeDescriptor {
            life_states: None,
            faction: None,
            ghost: false,
            tolerance: None,
            canonical_name: Some(if player { "player" } else { "enemy" }.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement,
            weapon: None,
            touchable: None,
            mesh: None,
            health: Some(health),
            behavior: None,
        }
    }

    fn validate(descriptor: EntityTypeDescriptor) -> Result<(), String> {
        validate_layer_expressions(&[descriptor], &StoreDeclarationSet::default(), &table())
    }

    #[test]
    fn layer_expression_per_owner_read_is_player_only() {
        // The scope keys the player on `movement`; build that descriptor
        // through the layer-scope entity directly when the movement JSON
        // shape drifts.
        let names = BTreeSet::new();
        let declarations = StoreDeclarationSet::default();
        let live = table();
        let player =
            LayerValidationScope::new(LayerScopeEntity::Player, &declarations, &live, &names);
        assert!(player.validate_expression(&input("seat.cap")).is_ok());
        let enemy =
            LayerValidationScope::new(LayerScopeEntity::Other, &declarations, &live, &names);
        let error = enemy.validate_expression(&input("seat.cap")).unwrap_err();
        assert!(error.contains("per-owner"), "{error}");
        // Through the gate, an enemy's health.resist per-owner read names its path.
        let error = validate(descriptor(
            false,
            input("tune.cap"),
            Some(input("seat.cap")),
        ))
        .unwrap_err();
        assert!(
            error.contains("descriptor `enemy` `components.health.resist.shock`"),
            "{error}"
        );
        assert!(validate(descriptor(false, input("tune.cap"), None)).is_ok());
    }

    #[test]
    fn layer_parameter_rejects_engine_and_defense_reads() {
        for (player, name) in [
            (true, "player.health"),
            (false, "player.health"),
            (true, "player.defense.shield"),
            (true, "player.defense.shield.capacity"),
            (false, "@defense.shield"),
            (true, "@defense.shield.capacity"),
            (false, "@impact.amount"),
            (false, "missing.slot"),
        ] {
            let names = BTreeSet::new();
            let declarations = StoreDeclarationSet::default();
            let live = table();
            let entity = if player {
                LayerScopeEntity::Player
            } else {
                LayerScopeEntity::Other
            };
            let scope = LayerValidationScope::new(entity, &declarations, &live, &names);
            assert!(
                scope.validate_expression(&input(name)).is_err(),
                "{name} must be refused"
            );
        }
        let error = validate(descriptor(false, input("player.health"), None)).unwrap_err();
        assert!(
            error.contains("components.health.layers[0].capacity"),
            "{error}"
        );
    }

    #[test]
    fn layer_expression_caps_at_depth_32_and_256_nodes() {
        fn chain(depth: usize) -> IrNode {
            let mut node = IrNode::Const {
                value: IrValue::Number(1.0),
            };
            for _ in 1..depth {
                node = IrNode::Add {
                    a: Box::new(node),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(1.0),
                    }),
                };
            }
            node
        }
        fn wide(nodes: usize) -> IrNode {
            // A balanced sum of constants with exactly `nodes` nodes (odd).
            let leaves = nodes.div_ceil(2);
            let mut level: Vec<IrNode> = (0..leaves)
                .map(|_| IrNode::Const {
                    value: IrValue::Number(1.0),
                })
                .collect();
            while level.len() > 1 {
                let mut next = Vec::new();
                let mut iter = level.into_iter();
                while let Some(a) = iter.next() {
                    match iter.next() {
                        Some(b) => next.push(IrNode::Add {
                            a: Box::new(a),
                            b: Box::new(b),
                        }),
                        None => next.push(a),
                    }
                }
                level = next;
            }
            level.pop().unwrap()
        }
        let health = |node: IrNode| HealthDescriptor {
            max: 10.0,
            layers: vec![DefenseLayerEntry {
                layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
                    name: "shield".to_string(),
                }),
                capacity: NumberOrIr::Ir(node),
                recharge_delay_ms: None,
                recharge_rate: None,
                resist: Default::default(),
            }],
            ..Default::default()
        };
        assert!(health(chain(32)).validate().is_ok());
        let error = health(chain(33)).validate().unwrap_err().to_string();
        assert!(
            error.contains("components.health.layers[0].capacity"),
            "{error}"
        );
        assert!(health(wide(255)).validate().is_ok());
        assert!(health(wide(257)).validate().is_err());
    }
}
