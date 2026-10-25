//! Ghost-link member primitives: argument validation, the ordered host-only
//! queue, and its apply at the host's next fixed tick.
//! See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7d

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, GhostLinkComponent, ScriptCtx,
};
use postretro_foundation::{
    ARM_GHOST_LINK, DISARM_GHOST_LINK, GHOST_LINK_PRIMITIVES, GhostColorFilterTargets,
    GhostLinkCommand, GhostLook, SET_GHOST_LINK_COLOR_FILTER, SET_GHOST_LINK_MEMBERS,
};
use postretro_level_format::ghost_links::GhostMembers;
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};
use postretro_scripting_core::sequence::{SequenceError, SequencedPrimitiveRegistry};
use serde_json::Value;

/// `setMembers` fields, each a category bit in the id-56 order.
const MEMBER_FIELDS: [(&str, GhostMembers); 4] = [
    ("players", GhostMembers::PLAYERS),
    ("entities", GhostMembers::ENTITIES),
    ("projectiles", GhostMembers::PROJECTILES),
    ("movers", GhostMembers::MOVERS),
];

/// Validate one ghost-link step's `args` into the command it queues. The `Err`
/// reason is the author-facing warning; the caller drops the step with it.
/// Every path a step takes (named dispatch, a trigger binding) judges it here,
/// so both runtimes and every source reject alike.
pub fn parse_ghost_link_command(primitive: &str, args: &Value) -> Result<GhostLinkCommand, String> {
    match primitive {
        ARM_GHOST_LINK => Ok(GhostLinkCommand::Armed(true)),
        DISARM_GHOST_LINK => Ok(GhostLinkCommand::Armed(false)),
        SET_GHOST_LINK_MEMBERS => parse_members(args),
        SET_GHOST_LINK_COLOR_FILTER => parse_color_filter(args).map(GhostLinkCommand::ColorFilter),
        other => Err(format!("`{other}` is not a ghost-link primitive")),
    }
}

/// The authored fields of `call`'s args, `null` (an omitted Luau or JS value)
/// read as absent. An empty Luau table may lower to `[]`.
fn authored_fields<'a>(call: &str, args: &'a Value) -> Result<Vec<(&'a str, &'a Value)>, String> {
    match args {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) if items.is_empty() => Ok(Vec::new()),
        Value::Object(fields) => Ok(fields
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.as_str(), value))
            .collect()),
        other => Err(format!("{call} takes a table of fields, got {other}")),
    }
}

fn parse_members(args: &Value) -> Result<GhostLinkCommand, String> {
    let mut mask = GhostMembers::empty();
    let mut shown = GhostMembers::empty();
    for (key, value) in authored_fields("setMembers", args)? {
        let Some((_, bit)) = MEMBER_FIELDS.iter().find(|(name, _)| *name == key) else {
            return Err(format!(
                "setMembers has no field `{key}`; expected players, entities, projectiles or movers"
            ));
        };
        let Some(on) = value.as_bool() else {
            return Err(format!("setMembers `{key}` must be a boolean, got {value}"));
        };
        mask |= *bit;
        shown.set(*bit, on);
    }
    if mask.is_empty() {
        return Err(
            "setMembers names no category; expected one or more of players, entities, projectiles or movers"
                .to_string(),
        );
    }
    Ok(GhostLinkCommand::Members {
        mask: mask.bits(),
        shown: shown.bits(),
    })
}

/// A finite number in `[0, 1]`.
fn unit(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && (0.0..=1.0).contains(number))
        .map(|number| number as f32)
}

fn parse_color_filter(args: &Value) -> Result<GhostColorFilterTargets, String> {
    let mut targets = GhostColorFilterTargets {
        tint: None,
        opacity: None,
        saturation: None,
        period_ms: 0.0,
    };
    for (key, value) in authored_fields("setColorFilter", args)? {
        match key {
            "tint" => {
                let tint = value
                    .as_array()
                    .filter(|components| components.len() == 3)
                    .and_then(|components| {
                        let parsed: Option<Vec<f32>> = components.iter().map(unit).collect();
                        parsed.map(|rgb| [rgb[0], rgb[1], rgb[2]])
                    });
                targets.tint = Some(tint.ok_or_else(|| {
                    format!(
                        "setColorFilter `tint` must be [r, g, b] with each component a finite number in 0..=1, got {value}"
                    )
                })?);
            }
            "opacity" | "saturation" => {
                let field = unit(value).ok_or_else(|| {
                    format!("setColorFilter `{key}` must be a finite number in 0..=1, got {value}")
                })?;
                if key == "opacity" {
                    targets.opacity = Some(field);
                } else {
                    targets.saturation = Some(field);
                }
            }
            "periodMs" => {
                targets.period_ms = value
                    .as_f64()
                    .filter(|ms| *ms >= 0.0 && (*ms as f32).is_finite())
                    .map(|ms| ms as f32)
                    .ok_or_else(|| {
                        format!(
                            "setColorFilter `periodMs` must be a finite number >= 0, got {value}"
                        )
                    })?;
            }
            other => {
                return Err(format!(
                    "setColorFilter has no field `{other}`; expected tint, opacity, saturation or periodMs"
                ));
            }
        }
    }
    if !targets.names_a_field() {
        return Err(
            "setColorFilter names no field; expected one or more of tint, opacity or saturation"
                .to_string(),
        );
    }
    Ok(targets)
}

/// Queue `command` for each target that is a live ghost link. On a connected
/// client nothing queues: link state is the host's and replicates.
pub(crate) fn enqueue_ghost_link_commands(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    targets: &[EntityId],
    command: GhostLinkCommand,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    let mut pending = ctx.pending_ghost_link_commands.borrow_mut();
    for &id in targets {
        if registry.get_component::<GhostLinkComponent>(id).is_ok() {
            pending.push((id, command));
        } else {
            log::warn!("[Ghost] command target {id:?} is not a live ghost link; skipping");
        }
    }
}

/// Apply every queued command at the start of an authoritative fixed tick, in
/// call order, on the host and in single player. `time` is that tick's script
/// time; `style_look` resolves a link's style look by map-order index.
///
/// Arm, disarm and member changes write the link component, so two in one
/// tick end in the last call's state. A colour-filter set ramps each field it
/// names from the look as it stood before this tick (style or ramp in flight,
/// unclamped), so the later of two calls in one tick wins per field and still
/// starts from the pre-tick value.
pub fn apply_pending_ghost_link_commands(
    ctx: &ScriptCtx,
    time: f64,
    style_look: impl Fn(u16) -> GhostLook,
) {
    let mut pending = ctx.pending_ghost_link_commands.borrow_mut();
    if !group_commands_apply_here(ctx) {
        pending.clear();
        return;
    }
    if pending.is_empty() {
        return;
    }
    let mut registry = ctx.registry.borrow_mut();
    // Pre-tick looks of the links a colour filter touched this tick. Commands
    // are script-rate events, so this small list is the only allocation.
    let mut before: Vec<(EntityId, GhostLook)> = Vec::new();
    for (entity, command) in pending.drain(..) {
        let Ok(ComponentValue::GhostLink(link)) =
            registry.get_component_value_mut(entity, ComponentKind::GhostLink)
        else {
            log::warn!("[Ghost] queued target {entity:?} no longer exists; skipping");
            continue;
        };
        match command {
            GhostLinkCommand::Armed(armed) => link.armed = armed,
            GhostLinkCommand::Members { mask, shown } => {
                link.members = (link.members & !mask) | (shown & mask);
            }
            GhostLinkCommand::ColorFilter(targets) => {
                let from = match before.iter().find(|(id, _)| *id == entity) {
                    Some((_, look)) => *look,
                    None => {
                        let look = link.filter.resolve(style_look(link.index), time);
                        before.push((entity, look));
                        look
                    }
                };
                link.filter.set(from, &targets, time);
            }
        }
    }
}

/// Register the member steps `l.arm()`, `l.disarm()`, `l.setMembers(...)` and
/// `l.setColorFilter(...)` build. A malformed step returns its warning and the
/// dispatcher drops it.
pub fn register_sequenced_ghost_link_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for name in GHOST_LINK_PRIMITIVES {
        let ctx = ctx.clone();
        registry.register(name, move |id, args| {
            let command = parse_ghost_link_command(name, args)
                .map_err(|reason| SequenceError::InvalidArgument { reason })?;
            enqueue_ghost_link_commands(&ctx, &ctx.registry.borrow(), &[id], command);
            Ok(())
        });
    }
}

/// The same primitives as reaction bodies, for raw tag-keyed wire data, so a
/// body fired by name behaves as one bound to a trigger.
pub fn register_ghost_link_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for name in GHOST_LINK_PRIMITIVES {
        let ctx = ctx.clone();
        registry.register(name, move |registry, targets, args| {
            let command = parse_ghost_link_command(name, args)
                .map_err(|reason| ReactionError::InvalidArgument { reason })?;
            enqueue_ghost_link_commands(&ctx, registry, targets, command);
            Ok(())
        });
    }
}

#[cfg(test)]
#[path = "ghost_link_commands_tests.rs"]
mod tests;
