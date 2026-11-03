//! The registered declarations follow the foundation profile's closed vocabulary.

use std::collections::BTreeSet;

use postretro_foundation::{
    Assistable, LifeStateMovementProfile, LifeStatePosture, LifeStateProfile,
};
use postretro_scripting_core::primitives_registry::{FieldInfo, RegisteredType, TypeShape};

use super::*;

fn registered<'a>(registry: &'a PrimitiveRegistry, name: &str) -> &'a RegisteredType {
    registry
        .iter_types()
        .find(|ty| ty.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn fields<'a>(registry: &'a PrimitiveRegistry, name: &str) -> &'a [FieldInfo] {
    let TypeShape::Struct { fields } = &registered(registry, name).shape else {
        panic!("{name} must be a struct");
    };
    fields
}

#[test]
fn life_state_typedef_profiles_match_foundation_fields_and_postures() {
    let registry = mini_registry();
    for (name, value) in [
        (
            "LifeStateProfile",
            // `assistable` serializes only when declared.
            serde_json::to_value(LifeStateProfile {
                assistable: Some(Assistable::default()),
                ..LifeStateProfile::default()
            })
            .unwrap(),
        ),
        (
            "LifeStateAssistable",
            serde_json::to_value(Assistable::default()).unwrap(),
        ),
        (
            "LifeStateMovementProfile",
            serde_json::to_value(LifeStateMovementProfile::default()).unwrap(),
        ),
    ] {
        let authored = value.as_object().unwrap();
        let actual: BTreeSet<&str> = fields(&registry, name)
            .iter()
            .map(|field| {
                assert!(
                    field.name.ends_with('?'),
                    "every {name} key permits omission"
                );
                field.name.trim_end_matches('?')
            })
            .collect();
        let expected: BTreeSet<&str> = authored.keys().map(String::as_str).collect();
        assert_eq!(
            actual, expected,
            "{name} must cover every serialized foundation field"
        );
        for field in fields(&registry, name) {
            let value = &authored[field.name.trim_end_matches('?')];
            let expected_type = match value {
                serde_json::Value::Bool(_) => "bool",
                serde_json::Value::Number(_) if name == "LifeStateAssistable" => "NumberValue",
                serde_json::Value::Number(_) => "f32",
                serde_json::Value::String(_) => "LifeStatePosture",
                serde_json::Value::Object(_) if field.name == "assistable?" => {
                    "LifeStateAssistable"
                }
                serde_json::Value::Object(_) => "LifeStateMovementProfile",
                other => panic!("new {name} field needs an SDK type: {other:?}"),
            };
            assert_eq!(field.ty_name, expected_type);
        }
    }
    let TypeShape::StringEnum { variants } = &registered(&registry, "LifeStatePosture").shape
    else {
        panic!("life-state posture must be a closed string enum");
    };
    let expected: Vec<&str> = [LifeStatePosture::Stand, LifeStatePosture::Crouch]
        .map(|posture| {
            let name = match posture {
                LifeStatePosture::Stand => "stand",
                LifeStatePosture::Crouch => "crouch",
            };
            assert_eq!(serde_json::to_value(posture).unwrap(), name);
            name
        })
        .into();
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant.name)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn life_state_typedefs_expose_named_profiles_and_rejoin_without_lowered_ids() {
    let registry = mini_registry();
    let descriptor: BTreeSet<&str> = fields(&registry, "LifeStatesDescriptor")
        .iter()
        .map(|field| field.name)
        .collect();
    assert_eq!(descriptor, BTreeSet::from(["initial", "states", "rejoin?"]));
    let component = fields(&registry, "EntityTypeComponents")
        .iter()
        .find(|field| field.name == "lifeStates?")
        .expect("player descriptor exposes life states");
    assert_eq!(component.ty_name, "Option<LifeStatesDescriptor>");
    let ts = generate_typescript(&registry);
    let luau = generate_luau(&registry);
    for text in [
        "lifeStates?: LifeStatesDescriptor | null;",
        "states: { readonly [state: string]: LifeStateProfile };",
        "rejoin?: { readonly [state: string]: string };",
        "movement?: LifeStateMovementProfile;",
        "posture?: LifeStatePosture;",
        "use?: boolean;",
    ] {
        assert!(ts.contains(text), "TypeScript missing {text}");
    }
    for text in [
        "lifeStates: LifeStatesDescriptor?,",
        "states: { [string]: LifeStateProfile },",
        "rejoin: { [string]: string }?,",
        "movement: LifeStateMovementProfile?,",
        "posture: LifeStatePosture?,",
        "use: boolean?,",
    ] {
        assert!(luau.contains(text), "Luau missing {text}");
    }
}
