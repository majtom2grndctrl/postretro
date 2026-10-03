//! Equivalent action ingestion through the two real VM boundaries.
use super::super::*;
use super::common::*;

fn sdk_pair(js_step: &str, lua_step: &str) -> [Result<EntityTypeDescriptor, String>; 2] {
    use crate::luau::{LuauConfig, LuauSubsystem, Which};
    use crate::primitives_registry::PrimitiveRegistry;
    use crate::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};

    let registry = PrimitiveRegistry::new();
    let quickjs = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
    let js = quickjs.definition_ctx().with(|ctx| {
        let value = run_script::<JsValue>(
            &ctx,
            &format!(
                "({{components:{{weapon:{{damage:10,range:96,resolution:'hitscan',primary:{{trigger:'press',recoveryMs:0,steps:[{js_step}]}}}}}}}})"
            ),
            "activation-helper.js",
        )
        .map_err(|error| error.to_string())?;
        entity_descriptor_from_js(&ctx, value).map_err(|error| error.to_string())
    });
    let luau = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();
    let lua = luau
        .run_source::<LuaValue>(
            Which::Definition,
            &format!(
                "return {{components={{weapon={{damage=10,range=96,resolution='hitscan',primary={{trigger='press',recoveryMs=0,steps={{{lua_step}}}}}}}}}}}"
            ),
            "activation-helper.luau",
        )
        .map_err(|error| error.to_string())
        .and_then(|value| entity_descriptor_from_lua(value).map_err(|error| error.to_string()));
    [js, lua]
}

// Regression: malformed helper options/scales disappeared into empty default shot data in JS.
#[test]
fn sdk_activation_shot_helpers_reject_malformed_options_and_scales() {
    for (js, lua) in [
        ("5", "5"),
        ("true", "true"),
        ("false", "false"),
        ("()=>({})", "function() return {} end"),
        ("'bad'", "'bad'"),
        ("[1]", "{1}"),
    ] {
        for (js_options, lua_options) in [
            (js.to_string(), lua.to_string()),
            (format!("{{scale:{js}}}"), format!("{{scale={lua}}}")),
        ] {
            for result in sdk_pair(
                &format!("activation.shot({js_options})"),
                &format!("activation.shot({lua_options})"),
            ) {
                let error = result.expect_err("malformed supplied helper data must reject");
                assert!(error.contains("activation.shot"), "{error}");
            }
        }
    }
    // Luau's empty table is a valid empty object; JS arrays/null remain distinct.
    for options in ["[]", "null", "{scale:[]}", "{scale:null}"] {
        let [js, _] = sdk_pair(&format!("activation.shot({options})"), "activation.shot()");
        let error = js.expect_err("JS array/null must not become omitted helper data");
        assert!(error.contains("activation.shot"), "{error}");
    }
    for result in sdk_pair("activation.shot({typo:5})", "activation.shot({typo=5})") {
        let error = result.expect_err("unknown shot option must not disappear");
        assert!(error.contains("activation.shot options"), "{error}");
    }
}

#[test]
fn sdk_activation_shot_helpers_preserve_defaults_and_numeric_expression_axes() {
    for (js, lua, damage, range) in [
        ("activation.shot()", "activation.shot()", 1.0, 1.0),
        ("activation.shot({})", "activation.shot({})", 1.0, 1.0),
        (
            "activation.shot({scale:{}})",
            "activation.shot({scale={}})",
            1.0,
            1.0,
        ),
        (
            "activation.shot({scale:{damage:2,range:3}})",
            "activation.shot({scale={damage=2,range=3}})",
            2.0,
            3.0,
        ),
        (
            "activation.shot({scale:{damage:activation.charge.times(5).plus(1)}})",
            "activation.shot({scale={damage=activation.charge:times(5):plus(1)}})",
            6.0,
            1.0,
        ),
        (
            "activation.shot({scale:{damage:{op:'mul',a:{op:'input',name:'charge'},b:{op:'const',value:3}}}})",
            "activation.shot({scale={damage={op='mul',a={op='input',name='charge'},b={op='const',value=3}}}})",
            3.0,
            1.0,
        ),
    ] {
        let [js, lua] = sdk_pair(js, lua).map(Result::unwrap);
        assert_eq!(
            js.weapon, lua.weapon,
            "helpers preserve the same descriptor shape"
        );
        let action = js.weapon.unwrap().primary;
        let compiled =
            postretro_foundation::CompiledActivation::compile(&action, "fixture").unwrap();
        let values = compiled.resolve_scales(0, 1.0).unwrap();
        assert!((values.damage - damage).abs() < 0.0001);
        assert!((values.range - range).abs() < 0.0001);
        assert!((values.resource_cost - 1.0).abs() < 0.0001);
    }
}

fn pair(js: &str, lua: &str) -> [Result<EntityTypeDescriptor, DescriptorError>; 2] {
    [
        eval_js(
            &format!(
                "({{components:{{weapon:{{damage:10,range:96,resolution:'hitscan',primary:{js}}}}}}})"
            ),
            entity_descriptor_from_js,
        ),
        eval_lua(
            &format!(
                "return {{components={{weapon={{damage=10,range=96,resolution='hitscan',primary={lua}}}}}}}"
            ),
            entity_descriptor_from_lua,
        ),
    ]
}
#[test]
fn activation_descriptors_preserve_independent_ir_through_both_vms() {
    let js = r#"{trigger:'press', recoveryMs:400,charge:{minMs:200,fullMs:1000},steps:[{kind:'shot',scale:{damage:{op:'add',a:{op:'mul',a:{op:'input',name:'charge'},b:{op:'const',value:5}},b:{op:'const',value:1}}}}]}"#;
    let lua = r#"{trigger='press',recoveryMs=400,charge={minMs=200,fullMs=1000},steps={{kind='shot',scale={damage={op='add',a={op='mul',a={op='input',name='charge'},b={op='const',value=5}},b={op='const',value=1}}}}}}"#;
    let [js, lua] = pair(js, lua).map(Result::unwrap);
    assert_eq!(js.weapon, lua.weapon);
    let action = js.weapon.unwrap().primary;
    let compiled = postretro_foundation::CompiledActivation::compile(&action, "fixture").unwrap();
    let values = compiled.resolve_scales(0, 1.0).unwrap();
    assert!((values.damage - 6.0).abs() < 0.0001);
    assert!((values.resource_cost - 1.0).abs() < 0.0001);
    assert!((values.projectile_size - 1.0).abs() < 0.0001);
}
#[test]
fn activation_parsers_reject_malformed_steps_scopes_and_domains() {
    for (js, lua) in [
        ("{kind:'wait',durationMs:10}", "{kind='wait',durationMs=10}"),
        ("{kind:'burst',count:3}", "{kind='burst',count=3}"),
        (
            "{kind:'shot',scale:{damage:-1}}",
            "{kind='shot',scale={damage=-1}}",
        ),
        (
            "{kind:'shot',scale:{range:0}}",
            "{kind='shot',scale={range=0}}",
        ),
        (
            "{kind:'shot',scale:{typo:2}}",
            "{kind='shot',scale={typo=2}}",
        ),
        (
            "{kind:'shot',scale:{damage:{op:'input',name:'player.health'}}}",
            "{kind='shot',scale={damage={op='input',name='player.health'}}}",
        ),
        (
            "{kind:'shot',scale:{damage:{op:'const',value:true}}}",
            "{kind='shot',scale={damage={op='const',value=true}}}",
        ),
        (
            "{kind:'shot',scale:()=>({})}",
            "{kind='shot',scale=function() return {} end}",
        ),
    ] {
        let js = format!("{{trigger:'press',recoveryMs:100,steps:[{js}]}}");
        let lua = format!("{{trigger='press',recoveryMs=100,steps={{{lua}}}}}");
        for result in pair(&js, &lua) {
            assert!(result.is_err(), "invalid action accepted: {js}");
        }
    }
}
#[test]
fn activation_optional_objects_do_not_degrade_functions_or_builtin_aliases() {
    for (js, lua) in [
        ("sounds:()=>({})", "sounds=function() return {} end"),
        ("emits:()=>({})", "emits=function() return {} end"),
        ("charge:()=>({})", "charge=function() return {} end"),
        ("emits:{activate:'activate'}", "emits={activate='activate'}"),
        ("emits:{impact:''}", "emits={impact=''}"),
        ("sounds:{fire:()=>{}}", "sounds={fire=function() end}"),
    ] {
        let js = format!("{{trigger:'press',recoveryMs:0,steps:[{{kind:'shot'}}],{js}}}");
        let lua = format!("{{trigger='press',recoveryMs=0,steps={{{{kind='shot'}}}},{lua}}}");
        for result in pair(&js, &lua) {
            assert!(result.is_err());
        }
    }
}
#[test]
fn activation_parsers_reject_nonfinite_values_at_the_authored_path() {
    for (js, lua) in [("Infinity", "math.huge"), ("NaN", "0/0")] {
        let js = format!(
            "{{trigger:'press',recoveryMs:0,steps:[{{kind:'shot',scale:{{damage:{js}}}}}]}}"
        );
        let lua = format!(
            "{{trigger='press',recoveryMs=0,steps={{{{kind='shot',scale={{damage={lua}}}}}}}}}"
        );
        for result in pair(&js, &lua) {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("primary.steps[0].scale.damage"), "{error}");
        }
    }
}
#[test]
fn activation_requires_primary_and_rejects_legacy_fields() {
    for result in [
        eval_js(
            "({components:{weapon:{damage:10,range:96,resolution:'hitscan',fireMode:'auto',fireRateMs:100}}})",
            entity_descriptor_from_js,
        ),
        eval_lua(
            "return {components={weapon={damage=10,range=96,resolution='hitscan',fireMode='auto',fireRateMs=100}}}",
            entity_descriptor_from_lua,
        ),
    ] {
        assert!(result.is_err());
    }
}

#[test]
fn activation_cue_name_byte_bounds_match_both_vm_authoring_boundaries() {
    for len in [256, 257] {
        let alias = "a".repeat(len);
        let js = format!(
            "{{trigger:'press',recoveryMs:0,steps:[{{kind:'shot'}}],sounds:{{fire:'{alias}'}},emits:{{activate:'{alias}'}}}}"
        );
        let lua = format!(
            "{{trigger='press',recoveryMs=0,steps={{{{kind='shot'}}}},sounds={{fire='{alias}'}},emits={{activate='{alias}'}}}}"
        );
        for result in pair(&js, &lua) {
            assert_eq!(result.is_ok(), len == 256);
        }
    }
}
