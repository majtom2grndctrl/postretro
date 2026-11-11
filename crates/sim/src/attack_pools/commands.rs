//! Attack pool member verbs as sequenced primitives (in-tick: `trigger_bindings`).
//! See: context/lib/scripting.md §12 (Entity addressing)

use postretro_entities::ScriptCtx;
use postretro_foundation::ATTACK_POOL_PRIMITIVES;
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::{AttackPoolVerb, issue_attack_pool_request};

/// Register the id-keyed `startAttackPool` and `stopAttackPool` steps an
/// attack pool member's handle emits. Both are host-only, before the first
/// `wait` too: a connected client issues nothing and latches nothing. Install
/// already rejected any `args`.
pub fn register_sequenced_attack_pool_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for name in ATTACK_POOL_PRIMITIVES {
        let ctx = ctx.clone();
        let verb = AttackPoolVerb::from_primitive(name).expect("every pool primitive is a verb");
        registry.register(name, move |id, _args| {
            if group_commands_apply_here(&ctx) {
                issue_attack_pool_request(&mut ctx.registry.borrow_mut(), id, verb);
            }
            Ok(())
        });
    }
}
