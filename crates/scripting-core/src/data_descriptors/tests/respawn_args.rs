// Respawn alone retains invalid numeric health through the authoring boundary.
use super::super::*;
use super::common::{eval_js, eval_lua};

fn sdk_pair(js: &str, lua: &str) -> (NamedReaction, NamedReaction) {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    let js = context.with(|ctx| {
        crate::quickjs::evaluate_prelude(&ctx).unwrap();
        let value: JsValue = ctx.eval(js).unwrap();
        let parsed = named_reaction_from_js(&ctx, value).unwrap();
        assert!(
            ctx.eval::<bool, _>("!Number.isFinite(options.health)")
                .unwrap()
        );
        parsed
    });
    let state = mlua::Lua::new();
    crate::luau_prelude::evaluate_prelude(&state, None).unwrap();
    let value: LuaValue = state.load(lua).eval().unwrap();
    let lua = named_reaction_from_lua(value).unwrap();
    assert!(
        state
            .load(
                "return options.health ~= options.health or math.abs(options.health) == math.huge"
            )
            .eval::<bool>()
            .unwrap()
    );
    (js, lua)
}

// Regression: true VM NaN/infinities rejected before the respawn drain could
// warn and clamp health; a finite f64-to-f32 overflow did not cover this path.
#[test]
fn respawn_sdk_twins_preserve_all_nonfinite_health_as_invalid_in_bodies_and_sequences() {
    for (js_number, lua_number) in [
        ("NaN", "0/0"),
        ("Infinity", "math.huge"),
        ("-Infinity", "-math.huge"),
    ] {
        for (js_target, lua_target, target) in [
            ("players()", "players()", None),
            ("on.player", "on.player", Some("@player")),
            ("on.activators", "on.activators", Some("@activators")),
        ] {
            for sequence in [false, true] {
                let js_command = format!("{js_target}.respawn(options)");
                let lua_command = format!("{lua_target}:respawn(options)");
                let js_body = if sequence {
                    format!("({{sequence:[{js_command}]}})")
                } else {
                    js_command
                };
                let lua_body = if sequence {
                    format!("{{sequence={{{lua_command}}}}}")
                } else {
                    lua_command
                };
                let js = format!(
                    "globalThis.options = Object.freeze({{health:{js_number},lifeState:'alive'}}); defineReaction('restore', on => {js_body})"
                );
                let lua = format!(
                    "options = table.freeze({{health={lua_number},lifeState='alive'}})\nreturn defineReaction('restore', function(on) return {lua_body} end)"
                );
                let (js, lua) = sdk_pair(&js, &lua);
                assert_eq!(js, lua);
                let args = match &js.descriptor {
                    ReactionDescriptor::Primitive(body) => {
                        assert_eq!(body.target.as_deref(), target);
                        &body.args
                    }
                    ReactionDescriptor::Sequence(steps) => {
                        assert_eq!(steps.len(), 1);
                        &steps[0].args
                    }
                    other => panic!("unexpected descriptor: {other:?}"),
                };
                assert_eq!(args, &serde_json::json!({"health":0,"state":"alive"}));
            }
        }
    }
}

#[test]
fn respawn_parser_twins_reject_wrong_health_types_and_nonfinite_other_fields() {
    for (js_args, lua_args, primitive, expected) in [
        ("{health:'full'}", "{health='full'}", "respawn", "health"),
        ("{health:true}", "{health=true}", "respawn", "health"),
        ("{health:{}}", "{health={}}", "respawn", "health"),
        ("{health:null}", "{health=false}", "respawn", "health"),
        (
            "{health:Infinity,state:NaN}",
            "{health=math.huge,state=0/0}",
            "respawn",
            "state",
        ),
        (
            "{health:Infinity,extra:{n:Infinity}}",
            "{health=math.huge,extra={n=math.huge}}",
            "respawn",
            "extra.n",
        ),
        (
            "{health:25,extra:Infinity}",
            "{health=25,extra=math.huge}",
            "respawn",
            "extra",
        ),
        (
            "{health:Infinity}",
            "{health=math.huge}",
            "grantHealth",
            "health",
        ),
        ("{amount:NaN}", "{amount=0/0}", "grantHealth", "amount"),
    ] {
        for sequence in [false, true] {
            let js_command = format!("{{primitive:'{primitive}',kind:'player',args:{js_args}}}");
            let lua_command = format!("{{primitive='{primitive}',kind='player',args={lua_args}}}");
            let js = if sequence {
                format!("({{name:'bad',sequence:[{js_command}]}})")
            } else {
                format!("({{name:'bad',...{js_command}}})")
            };
            let lua = if sequence {
                format!("return {{name='bad',sequence={{{lua_command}}}}}")
            } else {
                format!("local r = {lua_command}; r.name='bad'; return r")
            };
            let js = eval_js(&js, named_reaction_from_js)
                .unwrap_err()
                .to_string();
            let lua = eval_lua(&lua, named_reaction_from_lua)
                .unwrap_err()
                .to_string();
            assert!(js.contains(expected), "{js}");
            assert!(lua.contains(expected), "{lua}");
        }
    }
}

#[test]
fn respawn_parser_twins_do_not_mutate_frozen_authored_arguments() {
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        let value: JsValue = ctx.eval("globalThis.args = Object.freeze({health:Infinity}); ({name:'restore',primitive:'respawn',kind:'player',args})").unwrap();
        named_reaction_from_js(&ctx, value).unwrap();
        assert!(ctx.eval::<bool, _>("args.health === Infinity && Object.isFrozen(args)").unwrap());
    });
    let lua = mlua::Lua::new();
    let value: LuaValue = lua.load("args = table.freeze({health=math.huge}); return {name='restore',primitive='respawn',kind='player',args=args}").eval().unwrap();
    named_reaction_from_lua(value).unwrap();
    assert!(
        lua.load("return args.health == math.huge and table.isfrozen(args)")
            .eval::<bool>()
            .unwrap()
    );
}
