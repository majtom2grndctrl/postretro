// `attackTokens` and `attackBudgets` manifest drain: both runtimes convert the
// two values to JSON and parse them through one foundation parser, so they
// judge every token and profile alike.
// See: context/lib/scripting.md §2 (Attack token declarations)

use super::*;
use postretro_foundation::attack_budget::AttackBudgetVocabulary;

const KEYS: [&str; 2] = ["attackTokens", "attackBudgets"];

/// Drain the manifest's attack tokens and budget profiles. Strict, like the
/// damage vocabulary: attacks resolve their draws against this one set, so a
/// malformed entry rejects the manifest naming its key path.
pub fn drain_attack_budgets_js<'js>(
    ctx: &rquickjs::Ctx<'js>,
    obj: &Object<'js>,
) -> Result<AttackBudgetVocabulary, DescriptorError> {
    let mut values: [Option<serde_json::Value>; 2] = [None, None];
    for (slot, key) in values.iter_mut().zip(KEYS) {
        if !obj.contains_key(key).map_err(js_err)? {
            continue;
        }
        let raw: JsValue = obj.get(key).map_err(js_err)?;
        if raw.is_undefined() {
            continue;
        }
        *slot = Some(conv::js_to_json(ctx, raw).map_err(js_err)?);
    }
    AttackBudgetVocabulary::from_manifest(values[0].as_ref(), values[1].as_ref())
        .map_err(|reason| DescriptorError::InvalidShape { reason })
}

/// Luau twin of [`drain_attack_budgets_js`].
pub fn drain_attack_budgets_lua(table: &Table) -> Result<AttackBudgetVocabulary, DescriptorError> {
    let mut values: [Option<serde_json::Value>; 2] = [None, None];
    for (slot, key) in values.iter_mut().zip(KEYS) {
        let raw: LuaValue = table.get(key).map_err(lua_err)?;
        if matches!(raw, LuaValue::Nil) {
            continue;
        }
        *slot = Some(conv::lua_to_json(raw).map_err(lua_err)?);
    }
    AttackBudgetVocabulary::from_manifest(values[0].as_ref(), values[1].as_ref())
        .map_err(|reason| DescriptorError::InvalidShape { reason })
}
