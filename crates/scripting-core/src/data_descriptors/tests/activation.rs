//! Equivalent action ingestion through the two real VM boundaries.
use super::super::*;
use super::common::*;
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
