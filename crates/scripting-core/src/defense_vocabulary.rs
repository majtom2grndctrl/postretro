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
    let vocabulary = Arc::new(vocabulary);
    ctx.registry
        .borrow_mut()
        .set_defense_vocabulary(Arc::clone(&vocabulary));
    ctx.data_registry.borrow_mut().defense = vocabulary;
}

/// Warn once per slot name, when the trees register, for each UI binding that
/// reads `player.defense.<name>` for a layer no `defineMod` registers. The
/// binding still builds and reads absent, as any other slot without a value.
/// Returns the slot names warned about.
pub fn warn_unknown_layer_bindings<'a, T: serde::Serialize + 'a>(
    trees: impl IntoIterator<Item = &'a T>,
    vocabulary: &DefenseVocabulary,
) -> Vec<String> {
    fn walk(value: &serde_json::Value, vocabulary: &DefenseVocabulary, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(slot) = object.get("slot").and_then(serde_json::Value::as_str)
                    && let Some((layer, _)) = crate::player_slots::defense_slot(slot)
                    && vocabulary.layer_index(layer).is_none()
                    && !out.iter().any(|seen| seen == slot)
                {
                    log::warn!(
                        "[UI] a binding reads `{slot}`, which names no defense layer `defineMod` registers; it reads absent"
                    );
                    out.push(slot.to_string());
                }
                for child in object.values() {
                    walk(child, vocabulary, out);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    walk(child, vocabulary, out);
                }
            }
            _ => {}
        }
    }
    let mut warned = Vec::new();
    for tree in trees {
        if let Ok(value) = serde_json::to_value(tree) {
            walk(&value, vocabulary, &mut warned);
        }
    }
    warned
}
