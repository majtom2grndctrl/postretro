// Committing a mod's damage vocabulary: the registry copy every consumer reads
// and the owner-private `player.defense.<name>` slot pair per layer.
// See: context/lib/scripting.md §2, §5 (Defense layer slots)

use std::sync::Arc;

use postretro_foundation::defense::DefenseVocabulary;

use crate::ctx::ScriptCtx;

/// Install `vocabulary` as the committed one. Replaces the registry copy and
/// the layer slot family whole, so a removed layer's slots go with it.
pub fn commit_defense_vocabulary(ctx: &ScriptCtx, vocabulary: DefenseVocabulary) {
    ctx.slot_table
        .borrow_mut()
        .replace_player_defense_slots(&vocabulary.layers);
    ctx.data_registry.borrow_mut().defense = Arc::new(vocabulary);
}
