// Committing a mod's attack tokens and budget profiles: the registry copy the
// AI's attack budget reads and the data-registry copy every other consumer reads.
// See: context/lib/scripting.md §2 (Attack token declarations)

use std::sync::Arc;

use postretro_foundation::attack_budget::AttackBudgetVocabulary;

use crate::ctx::ScriptCtx;

/// Install `vocabulary` as the committed one, replacing both copies whole.
/// The AI re-resolves draws and budgets when it sees the new pointer, so a
/// hot reload applies from the next tick.
pub fn commit_attack_budgets(ctx: &ScriptCtx, vocabulary: AttackBudgetVocabulary) {
    let vocabulary = Arc::new(vocabulary);
    ctx.registry
        .borrow_mut()
        .set_attack_budgets(Arc::clone(&vocabulary));
    ctx.data_registry.borrow_mut().attack_budgets = vocabulary;
}
