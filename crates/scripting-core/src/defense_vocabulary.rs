// Committing a mod's damage vocabulary: the registry copy every consumer reads
// the owner-private `player.defense.<name>` slot pair per layer and the
// local-only `player.status.<name>` slot per status.
// See: context/lib/scripting.md §2, §5 (Defense layer slots)

use std::sync::Arc;

use postretro_foundation::defense::DefenseVocabulary;

use crate::ctx::ScriptCtx;

/// Install `vocabulary` as the committed one. Replaces the registry copy and
/// the layer slot family whole, so a removed layer's slots go with it.
pub fn commit_defense_vocabulary(ctx: &ScriptCtx, vocabulary: DefenseVocabulary) {
    {
        let mut slots = ctx.slot_table.borrow_mut();
        slots.replace_player_defense_slots(&vocabulary.layers);
        let statuses: Vec<String> = vocabulary
            .statuses
            .iter()
            .map(|status| status.name.clone())
            .collect();
        slots.replace_player_status_slots(&statuses);
    }
    let vocabulary = Arc::new(vocabulary);
    ctx.registry
        .borrow_mut()
        .set_defense_vocabulary(Arc::clone(&vocabulary));
    ctx.data_registry.borrow_mut().defense = vocabulary;
}
