// Install-time `setState` IR bindings the app drain evaluates against.
// See: context/lib/scripting.md §10.4

use postretro_entities::ScriptCtx;
use postretro_foundation::{
    BakedIr, BoundProgram, CURRENT_IR_VERSION, IrType, IrValue, eval_and_write, ir_node_from_json,
};
use postretro_scripting_core::data_descriptors::ReactionDescriptor;
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::ir::bind;
use postretro_scripting_core::ir_scopes::DispatchScope;

use super::SetStateArgs;

/// Install-time bindings for `setState`. Entity-owned reaction descriptors and
/// queued commands deliberately retain only raw JSON; this binary-side table
/// owns app-drain `DispatchScope` programs plus known rejected command identities.
#[derive(Debug, Default)]
pub struct SystemReactionIrBindings {
    bindings: Vec<SystemSetStateBinding>,
    rejected_literals: Vec<SystemSetStateLiteralRejection>,
    pub(super) warned_missing_inputs:
        std::cell::RefCell<std::collections::HashSet<(usize, String)>>,
}

const APP_DRAIN_DISPATCH_INPUTS: [(&str, IrType); 1] = [("@rising", IrType::Bool)];

/// A system-command `setState` IR is identified by the fields the command
/// carries across the entities boundary. Keep the raw value here so the app
/// drain can compare it directly instead of allocating a slot key and
/// serializing its JSON on every fire.
#[derive(Debug)]
struct SystemSetStateBinding {
    /// The reaction name this binding was composed from. Carried so install-time
    /// validation (E18 V4b) can include system-setState IR in its broader
    /// descriptor-scope analysis without re-walking `BoundProgram.root` against
    /// a name list that can go stale.
    name: String,
    slot: String,
    value: serde_json::Value,
    program: Option<BoundProgram<DispatchScope>>,
    required_dispatch_inputs: Vec<String>,
    /// The value reads `byPlayer(on.player)`. Only a player event publishes
    /// the event player, and it binds the value in-tick; an app-drain
    /// dispatch came from a source no install check covers (a UI action, a
    /// progress target, an `onComplete` chain), so it warns once and skips.
    reads_event_player: bool,
    event_player_warned: std::cell::Cell<bool>,
}

#[derive(Debug)]
struct SystemSetStateLiteralRejection {
    slot: String,
    value: serde_json::Value,
}

impl SystemSetStateBinding {
    fn matches(&self, slot: &str, value: &serde_json::Value) -> bool {
        self.slot == slot && self.value.eq(value)
    }
}

/// Outcome of looking up an object-shaped `setState` value at the app drain.
/// Rejected entries deliberately remain in the table after their install-time
/// diagnostic, so a validly queued command does not warn again on every fire.
/// A `byPlayer(on.player)` value has no install-time diagnostic for the
/// sources that reach it here; its first dispatch warns instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemReactionIrDispatch {
    Evaluated,
    Rejected,
    Unknown,
}

impl SystemReactionIrBindings {
    /// Rebuild bindings after the active reaction set is composed. Inline args
    /// carry only an IR node, so stamp the current epoch while wrapping them in
    /// the normal `BakedIr` envelope; no wire envelope/version is introduced.
    pub fn rebuild(&mut self, data_registry: &DataRegistry, script_ctx: &ScriptCtx) {
        self.bindings.clear();
        self.rejected_literals.clear();
        let scope = DispatchScope::script(script_ctx.clone(), &APP_DRAIN_DISPATCH_INPUTS);
        self.warned_missing_inputs.borrow_mut().clear();

        for reaction in &data_registry.reactions {
            let ReactionDescriptor::Primitive(primitive) = &reaction.descriptor else {
                continue;
            };
            if primitive.primitive != "setState" || primitive.tag.is_some() {
                continue;
            }

            let args: SetStateArgs = match serde_json::from_value(primitive.args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!(
                        "[Scripting] setState reaction `{}` has invalid args; not binding: {error}",
                        reaction.name
                    );
                    continue;
                }
            };
            let per_owner = script_ctx
                .slot_table
                .borrow()
                .get(&args.slot)
                .is_some_and(|record| record.schema.per_owner);
            if per_owner {
                log::warn!(
                    "[Scripting] setState reaction `{}` rejects per-owner slot `{}`; not binding",
                    reaction.name,
                    args.slot
                );
                if is_ir_node(&args.value) {
                    self.bindings.push(SystemSetStateBinding {
                        name: reaction.name.clone(),
                        slot: args.slot,
                        value: args.value,
                        program: None,
                        required_dispatch_inputs: Vec::new(),
                        reads_event_player: false,
                        event_player_warned: std::cell::Cell::new(false),
                    });
                } else {
                    self.rejected_literals.push(SystemSetStateLiteralRejection {
                        slot: args.slot,
                        value: args.value,
                    });
                }
                continue;
            }
            if !is_ir_node(&args.value) {
                continue;
            }

            // Retain even failed bindings. A queued command with the same raw
            // identity is known-invalid, while a missing entry is genuinely
            // unknown or stale and remains worth diagnosing at the drain.
            let mut binding = SystemSetStateBinding {
                name: reaction.name.clone(),
                slot: args.slot,
                value: args.value,
                program: None,
                required_dispatch_inputs: Vec::new(),
                reads_event_player: false,
                event_player_warned: std::cell::Cell::new(false),
            };

            let root = match ir_node_from_json(binding.value.clone(), "setState.value") {
                Ok(root) => root,
                Err(error) => {
                    log::warn!(
                        "[Scripting] setState reaction `{}` has invalid runtime value; not binding: {error}",
                        reaction.name
                    );
                    self.bindings.push(binding);
                    continue;
                }
            };
            let baked = BakedIr {
                version: CURRENT_IR_VERSION,
                output: Some(binding.slot.clone()),
                root,
            };
            binding.required_dispatch_inputs = baked.root.dispatch_input_names();
            // A `byPlayer(on.player)` value binds in-tick under its player
            // event; no app-drain source publishes the event player. Install
            // rejects the subscriptions it can see (triggers, crossings,
            // `levelLoad`), but a UI action, a progress target or an
            // `onComplete` chain can still queue it here, so `dispatch` warns
            // once for this binding and skips.
            if postretro_scripting_core::player_event_scope::reads_event_player(&binding.value) {
                binding.reads_event_player = true;
                self.bindings.push(binding);
                continue;
            }
            match bind(&baked, &scope) {
                Ok(program) => {
                    binding.program = Some(program);
                }
                Err(error) => {
                    // `StoreScope::script` makes readonly, unknown, and
                    // non-projectable targets bind failures before any fire.
                    log::warn!(
                        "[Scripting] setState reaction `{}` cannot bind runtime value for `{}`: {error}",
                        reaction.name,
                        binding.slot
                    );
                }
            }
            self.bindings.push(binding);
        }
    }

    /// Evaluate an already-bound IR program at the app-drain write point.
    /// `setState` queues the active descriptor's `slot` and raw `value`
    /// unchanged, and binding is pure over that pair. New producers need a
    /// stable key instead of relying on this JSON equality lookup.
    pub fn dispatch(
        &self,
        slot: &str,
        value: &serde_json::Value,
        dispatch_source: &str,
        dispatch_values: &[(String, IrValue)],
        script_ctx: &ScriptCtx,
    ) -> SystemReactionIrDispatch {
        let Some((binding_index, binding)) = self
            .bindings
            .iter()
            .enumerate()
            .find(|(_, binding)| binding.matches(slot, value))
        else {
            return SystemReactionIrDispatch::Unknown;
        };
        if binding.reads_event_player {
            if !binding.event_player_warned.replace(true) {
                log::warn!(
                    "[Scripting] setState reaction `{}` reads `byPlayer(on.player)`, which only a player event publishes; skipped",
                    binding.name
                );
            }
            return SystemReactionIrDispatch::Rejected;
        }
        let Some(program) = &binding.program else {
            return SystemReactionIrDispatch::Rejected;
        };
        let mut carried_names: Vec<String> = dispatch_values
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        carried_names.sort_unstable();
        carried_names.dedup();
        if !binding
            .required_dispatch_inputs
            .iter()
            .all(|required| carried_names.contains(required))
        {
            let warning_key = (binding_index, dispatch_source.to_string());
            if self.warned_missing_inputs.borrow_mut().insert(warning_key) {
                log::warn!(
                    "[Scripting] setState runtime value for `{slot}` requires dispatch inputs {:?}, but source `{dispatch_source}` carries {:?}; skipping",
                    binding.required_dispatch_inputs,
                    carried_names,
                );
            }
            return SystemReactionIrDispatch::Rejected;
        }
        let mut scope = DispatchScope::script(script_ctx.clone(), &APP_DRAIN_DISPATCH_INPUTS);
        for (name, value) in dispatch_values {
            if let Err(error) = scope.seed(name, *value) {
                log::warn!(
                    "[Scripting] setState runtime value for `{slot}` received invalid dispatch input `{name}` ({error:?}); skipping"
                );
                return SystemReactionIrDispatch::Rejected;
            }
        }
        eval_and_write(program, &mut scope);
        SystemReactionIrDispatch::Evaluated
    }

    /// Whether install-time binding rejected this literal legacy `setState`.
    /// The app drain uses this to keep repeated fires silent after the one
    /// reaction-naming diagnostic emitted by [`Self::rebuild`].
    pub fn rejects_literal(&self, slot: &str, value: &serde_json::Value) -> bool {
        self.rejected_literals
            .iter()
            .any(|binding| binding.slot == slot && binding.value.eq(value))
    }

    /// Each `setState` binding's reaction name paired with the dispatch inputs its
    /// bound program reads. E18 install validation (V4b) combines this with
    /// descriptor-level sentinel requirements when deciding whether a `fire`
    /// target needs unavailable trigger-fire scope. Reads the precomputed
    /// `required_dispatch_inputs` rather than re-walking the IR.
    pub fn reaction_dispatch_inputs(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.bindings.iter().map(|binding| {
            (
                binding.name.as_str(),
                binding.required_dispatch_inputs.as_slice(),
            )
        })
    }
}

/// Runtime values are object-shaped IR nodes; all shipped literal store values
/// are scalar JSON or arrays. An object-valued literal slot type must replace
/// this discriminator, or its writes would be treated as malformed IR.
pub fn is_ir_node(value: &serde_json::Value) -> bool {
    value.is_object()
}
