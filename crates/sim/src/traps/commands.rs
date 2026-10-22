//! The trap member verbs `fireTrap`, `startTrap` and `stopTrap`: their step
//! arguments and the sequenced primitives named dispatch and `wait` landings
//! run. In-tick binding lowers the same verbs in `trigger_bindings`.
//! See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::ScriptCtx;
use postretro_entities::components::projectile_trap::TrapRequest;
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use serde::Deserialize;

/// A `startTrap` step's per-run options. Install validation already rejected
/// non-finite, non-positive intervals and negative phases.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartTrapArgs {
    interval_ms: Option<f32>,
    phase_ms: Option<f32>,
}

/// The request one trap verb issues, or `None` for another primitive or
/// malformed `startTrap` options.
pub fn trap_request_for_step(primitive: &str, args: &serde_json::Value) -> Option<TrapRequest> {
    match primitive {
        "fireTrap" => Some(TrapRequest::Fire),
        "stopTrap" => Some(TrapRequest::Stop),
        "startTrap" => {
            let args: StartTrapArgs = if args.is_null() {
                StartTrapArgs::default()
            } else {
                serde_json::from_value(args.clone()).ok()?
            };
            Some(TrapRequest::Start {
                interval_ms: args.interval_ms,
                phase_ms: args.phase_ms,
            })
        }
        _ => None,
    }
}

/// Register the id-keyed trap verbs a trap member's handle emits. Every trap
/// step is host-only, before the first `wait` too: a connected client issues
/// nothing and latches nothing.
pub fn register_sequenced_trap_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for name in ["fireTrap", "startTrap", "stopTrap"] {
        let ctx = ctx.clone();
        registry.register(name, move |id, args| {
            if !group_commands_apply_here(&ctx) {
                return Ok(());
            }
            let Some(request) = trap_request_for_step(name, args) else {
                log::warn!("[Trap] `{name}` step has invalid args; skipping");
                return Ok(());
            };
            super::issue_trap_request(&mut ctx.registry.borrow_mut(), id, request);
            Ok(())
        });
    }
}
