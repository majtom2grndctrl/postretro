// Value-free twin of the interaction-pair scope, and the load gate that binds
// every `assistable` expression against it before a mod commits.
// See: context/lib/scripting.md §11 (interaction-pair scope) · §12 (Assist)

// The twin answers the same read table as `InteractionPairScope`, with types
// only, from the incoming store schemas and the mod-wide life-state names. It
// binds where a mod's descriptors and store declarations meet before commit:
// `run_mod_init` on first load and `commit_staged_manifest_result` on reload.
// The descriptor parsers see no store schemas, so they cannot host it.

use std::cell::RefCell;
use std::collections::BTreeSet;

use postretro_entities::EntityTypeDescriptor;
use postretro_foundation::{NumberOrIr, check_activation_ir_caps};

use crate::ir::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use crate::ir::{BakedIr, BindError, CURRENT_IR_VERSION, IrNode, IrType, IrValue, bind};
use crate::ir_pair_scope::{
    ASSIST_PAIR_ROLES, PairRead, PairRoles, PairSchema, classify_pair_read,
};
use crate::ir_scopes::StoreScope;
use crate::slot_table::{SlotTable, StoreDeclarationSet};

/// The incoming schema: the attempt's declarations over the live table. The
/// live table only gains namespaces, so the union is what the host binds
/// against after commit.
pub(crate) struct IncomingSchema<'a> {
    pub(crate) declarations: &'a StoreDeclarationSet,
    pub(crate) live: &'a SlotTable,
    pub(crate) life_state_names: &'a BTreeSet<&'a str>,
}

impl PairSchema for IncomingSchema<'_> {
    fn store_record(&self, name: &str) -> Option<(Option<IrType>, bool)> {
        let declared = self.declarations.iter().find_map(|declaration| {
            let slot = name
                .strip_prefix(declaration.namespace.as_str())?
                .strip_prefix('.')?;
            declaration
                .records
                .iter()
                .find(|(record_name, _)| record_name == slot)
                .map(|(_, record)| record)
        });
        let schema = match declared {
            Some(record) => &record.schema,
            None => &self.live.get(name)?.schema,
        };
        Some((StoreScope::project(&schema.slot_type), schema.per_owner))
    }

    fn life_state_id(&self, name: &str) -> Option<u8> {
        // Validation needs membership only; the id is the name's rank.
        self.life_state_names
            .iter()
            .position(|known| *known == name)
            .and_then(|index| u8::try_from(index).ok())
    }
}

/// The value-free, read-only twin of [`InteractionPairScope`]: the same read
/// table, answered with types only. A refused read records why, so the gate
/// can name it at the authored key path.
///
/// [`InteractionPairScope`]: crate::ir_pair_scope::InteractionPairScope
pub struct PairValidationScope<'a> {
    roles: PairRoles,
    schema: IncomingSchema<'a>,
    error: RefCell<Option<String>>,
}

impl<'a> PairValidationScope<'a> {
    pub fn new(
        roles: PairRoles,
        declarations: &'a StoreDeclarationSet,
        live: &'a SlotTable,
        life_state_names: &'a BTreeSet<&'a str>,
    ) -> Self {
        Self {
            roles,
            schema: IncomingSchema {
                declarations,
                live,
                life_state_names,
            },
            error: RefCell::new(None),
        }
    }

    fn resolve(&self, name: &str, owner: Option<&str>) -> Option<ResolvedInput<IrType>> {
        match classify_pair_read(&self.roles, &self.schema, name, owner) {
            Ok(PairRead::Global(ir_type)) => Some(ResolvedInput {
                handle: ir_type,
                ir_type,
            }),
            Ok(PairRead::Side(_, input)) => {
                let ir_type = input.ir_type();
                Some(ResolvedInput {
                    handle: ir_type,
                    ir_type,
                })
            }
            Err(reason) => {
                self.error.borrow_mut().get_or_insert(reason);
                None
            }
        }
    }

    /// Bind one pair expression: the activation caps, every read through the
    /// pair read table, and a Number root. The error names the fault, not the
    /// key path; the caller prefixes it.
    pub fn validate_expression(&self, node: &IrNode) -> Result<(), String> {
        check_activation_ir_caps(node).map_err(str::to_string)?;
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

impl BindingScope for PairValidationScope<'_> {
    type InputHandle = IrType;
    type OutputHandle = IrType;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<IrType>> {
        self.resolve(name, None)
    }

    fn resolve_owned_input(&self, name: &str, owner: &str) -> Option<ResolvedInput<IrType>> {
        self.resolve(name, Some(owner))
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<IrType>> {
        None
    }

    fn read(&self, handle: &IrType) -> IrValue {
        handle.zero()
    }

    fn write(&mut self, _handle: &IrType, _value: IrValue) {
        unreachable!("PairValidationScope is read-only")
    }
}

/// The mod-wide life-state names, from the descriptor list as
/// `first_life_state_name_overflow` walks it.
pub fn mod_life_state_names(descriptors: &[EntityTypeDescriptor]) -> BTreeSet<&str> {
    descriptors
        .iter()
        .filter_map(|descriptor| descriptor.life_states.as_ref())
        .flat_map(|block| block.states.keys().map(String::as_str))
        .collect()
}

/// Bind every `assistable` expression of an incoming descriptor list against
/// [`PairValidationScope`]. The first refusal names the descriptor and the
/// authored key path (`components.lifeStates.states.<state>.assistable.radius`
/// or `.holdMs`). Literals were range-checked at parse.
pub fn validate_assistable_expressions(
    descriptors: &[EntityTypeDescriptor],
    declarations: &StoreDeclarationSet,
    live: &SlotTable,
) -> Result<(), String> {
    let names = mod_life_state_names(descriptors);
    let scope = PairValidationScope::new(ASSIST_PAIR_ROLES, declarations, live, &names);
    for descriptor in descriptors {
        let Some(block) = descriptor.life_states.as_ref() else {
            continue;
        };
        for (state, profile) in &block.states {
            let Some(assistable) = profile.assistable.as_ref() else {
                continue;
            };
            for (field, value) in [
                ("radius", &assistable.radius),
                ("holdMs", &assistable.hold_ms),
            ] {
                let NumberOrIr::Ir(node) = value else {
                    continue;
                };
                scope.validate_expression(node).map_err(|reason| {
                    let label = match descriptor.canonical_name.as_deref() {
                        Some(name) => format!("descriptor `{name}`"),
                        None => "an unnamed descriptor".to_string(),
                    };
                    format!(
                        "{label} `components.lifeStates.states.{state}.assistable.{field}`: {reason}"
                    )
                })?;
            }
        }
    }
    Ok(())
}
