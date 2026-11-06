// `grantLayer` is a resource grant: a non-finite amount rejects the whole
// level manifest in both runtimes, as a bad `grantHealth` does. A bad layer
// reference parses, and install drops only the reaction holding it.
// See: context/lib/scripting.md §11 (Defense layers)

use super::super::*;
use super::common::{eval_js, eval_lua};

const SHIELD_JS: &str = "{kind:'defenseLayer',name:'shield'}";
const SHIELD_LUA: &str = "{kind='defenseLayer',name='shield'}";

fn manifests(
    js_args: &str,
    lua_args: &str,
) -> (Result<LevelManifest, String>, Result<LevelManifest, String>) {
    let js = eval_js(
        &format!(
            "({{reactions:[{{name:'ok',primitive:'playSound',args:{{sound:'x'}}}},{{name:'grant',primitive:'grantLayer',kind:'player',args:{js_args}}}]}})"
        ),
        |ctx, value| LevelManifest::from_js_value(ctx, value).map_err(|error| error.to_string()),
    );
    let lua = eval_lua(
        &format!(
            "return {{reactions={{{{name='ok',primitive='playSound',args={{sound='x'}}}},{{name='grant',primitive='grantLayer',kind='player',args={lua_args}}}}}}}"
        ),
        |value| LevelManifest::from_lua_value(value).map_err(|error| error.to_string()),
    );
    (js, lua)
}

#[test]
fn grant_layer_non_finite_amount_rejects_manifest() {
    for (js_args, lua_args, expected) in [
        (
            format!("{{layer:{SHIELD_JS},amount:NaN}}"),
            format!("{{layer={SHIELD_LUA},amount=0/0}}"),
            "amount",
        ),
        (
            format!("{{layer:{SHIELD_JS},amount:Infinity}}"),
            format!("{{layer={SHIELD_LUA},amount=math.huge}}"),
            "amount",
        ),
    ] {
        let (js, lua) = manifests(&js_args, &lua_args);
        let js = js.expect_err("a bad grant rejects the JS manifest");
        let lua = lua.expect_err("a bad grant rejects the Luau manifest");
        assert!(js.contains(expected), "{js}");
        assert!(lua.contains(expected), "{lua}");
    }
}

#[test]
fn grant_layer_with_a_layer_and_an_optional_amount_parses_in_both_runtimes() {
    // A bad reference parses too: install drops only its reaction.
    for (js_args, lua_args) in [
        (
            "{layer:'shield'}".to_string(),
            "{layer='shield'}".to_string(),
        ),
        (
            "{layer:{kind:'damageType',name:'shock'}}".to_string(),
            "{layer={kind='damageType',name='shock'}}".to_string(),
        ),
        (
            format!("{{layer:{SHIELD_JS}}}"),
            format!("{{layer={SHIELD_LUA}}}"),
        ),
        (
            format!("{{layer:{SHIELD_JS},amount:25}}"),
            format!("{{layer={SHIELD_LUA},amount=25}}"),
        ),
    ] {
        let (js, lua) = manifests(&js_args, &lua_args);
        let (js, lua) = (js.unwrap(), lua.unwrap());
        assert_eq!(js.reactions, lua.reactions);
        assert_eq!(js.reactions.len(), 2);
    }
}
