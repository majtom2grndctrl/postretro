// Emissive primitives at the Rust dispatch boundary: write-clears-animation,
// clamps, validation and tag dispatch (`research.md` ordering pins).
// See: context/lib/scripting.md §10.2a

use super::set_emissive_animation::SetEmissiveAnimationArgs;
use super::set_emissive_brightness::SetEmissiveBrightnessArgs;
use super::set_emissive_color::SetEmissiveColorArgs;
use super::*;
use crate::scripting::reactions::log_capture::capture;
use postretro_entities::{EmissiveAnimation, Transform};
use serde_json::{Value, json};

fn spawn_volume(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            EmissiveVolumeComponent::authored(0.5, 4.0, [1.0, 0.5, 0.25]),
        )
        .unwrap();
    id
}

fn volume(registry: &EntityRegistry, id: EntityId) -> EmissiveVolumeComponent {
    registry
        .get_component::<EmissiveVolumeComponent>(id)
        .unwrap()
        .clone()
}

fn tag_registry() -> ReactionPrimitiveRegistry {
    let mut registry = ReactionPrimitiveRegistry::new();
    register_emissive_reaction_primitives(&mut registry);
    registry
}

/// Dispatch `primitive` over `targets` through the registered tag handler,
/// returning the warnings it logged.
fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    primitive: &str,
    args: Value,
) -> Vec<String> {
    let reactions = tag_registry();
    capture(|| {
        reactions
            .dispatch(primitive, registry, targets, &args)
            .unwrap();
    })
    .into_iter()
    .filter(|(level, _)| *level == log::Level::Warn)
    .map(|(_, message)| message)
    .collect()
}

/// Dispatch with typed args, bypassing JSON, for the non-finite values the
/// script converters reject before they reach here.
fn brightness(registry: &mut EntityRegistry, id: EntityId, value: f32) -> Vec<String> {
    capture(|| {
        set_emissive_brightness::dispatch(
            registry,
            &[id],
            &SetEmissiveBrightnessArgs { brightness: value },
        )
    })
    .into_iter()
    .filter(|(level, _)| *level == log::Level::Warn)
    .map(|(_, message)| message)
    .collect()
}

fn animation(
    registry: &mut EntityRegistry,
    id: EntityId,
    args: Option<EmissiveAnimation>,
) -> Vec<String> {
    capture(|| set_emissive_animation::dispatch(registry, &[id], &SetEmissiveAnimationArgs(args)))
        .into_iter()
        .filter(|(level, _)| *level == log::Level::Warn)
        .map(|(_, message)| message)
        .collect()
}

fn flicker() -> EmissiveAnimation {
    EmissiveAnimation {
        period_ms: 1000.0 / 6.0,
        play_count: None,
        brightness: vec![0.965, 0.58, 1.0, 0.804, 0.405, 0.916, 0.51, 0.755],
    }
}

fn fade() -> EmissiveAnimation {
    EmissiveAnimation {
        period_ms: 2000.0,
        play_count: Some(1),
        brightness: vec![1.0, 0.5, 0.0],
    }
}

#[test]
fn registers_the_three_emissive_primitives_in_both_registries() {
    let tag = tag_registry();
    let mut sequenced = SequencedPrimitiveRegistry::new();
    register_sequenced_emissive_primitives(&mut sequenced, ScriptCtx::new());
    for primitive in EMISSIVE_PRIMITIVES {
        assert!(tag.contains(primitive), "tag registry lacks {primitive}");
        assert!(
            sequenced.contains(primitive),
            "sequenced registry lacks {primitive}"
        );
    }
    assert!(!tag.contains("setComponent"));
}

// A12 (local half): no write path carries the authored range. Each arg
// type's script keys, as serde names them, exclude `min` and `max`.
#[test]
fn no_emissive_primitive_arg_carries_min_or_max() {
    let keys =
        |value: Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    for (primitive, value) in [
        (
            "setEmissiveBrightness",
            serde_json::to_value(SetEmissiveBrightnessArgs { brightness: 1.0 }).unwrap(),
        ),
        (
            "setEmissiveColor",
            serde_json::to_value(SetEmissiveColorArgs { color: [1.0; 3] }).unwrap(),
        ),
        (
            "setEmissiveAnimation",
            serde_json::to_value(SetEmissiveAnimationArgs(Some(EmissiveAnimation {
                play_count: Some(1),
                ..flicker()
            })))
            .unwrap(),
        ),
    ] {
        let keys = keys(value);
        assert!(
            !keys.iter().any(|key| key == "min" || key == "max"),
            "{primitive} args carry {keys:?}"
        );
    }
}

// P1: a looping flicker, then a brightness write: the write clears the curve
// and its value holds.
#[test]
fn a_brightness_write_clears_a_running_animation_and_holds() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    dispatch(
        &mut registry,
        &[id],
        "setEmissiveAnimation",
        serde_json::to_value(flicker()).unwrap(),
    );
    assert!(volume(&registry, id).animation.is_some());

    let warnings = dispatch(
        &mut registry,
        &[id],
        "setEmissiveBrightness",
        json!({ "brightness": 0 }),
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    let after = volume(&registry, id);
    assert_eq!((after.brightness, after.animation), (0.0, None));
}

// P4: a finite fade mid-run, then a brightness write: the fade is gone, so
// it can never settle over the written value.
#[test]
fn a_brightness_write_mid_fade_removes_the_fade() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    animation(&mut registry, id, Some(fade()));
    brightness(&mut registry, id, 0.5);
    let after = volume(&registry, id);
    assert_eq!((after.brightness, after.animation), (0.5, None));
}

/// Fire one `levelLoad` sequence of `steps` against `id` through the
/// sequenced dispatcher, as a body's steps run.
fn run_body(steps: Vec<(&str, Value)>) -> EmissiveVolumeComponent {
    use postretro_entities::{DataRegistry, NamedReaction, ReactionDescriptor, SequenceStep};
    use postretro_scripting_core::reaction_dispatch::{
        fire_named_event_with_sequences, validate_sequence_primitives,
    };
    let ctx = ScriptCtx::new();
    let id = spawn_volume(&mut ctx.registry.borrow_mut());
    let mut sequenced = SequencedPrimitiveRegistry::new();
    register_sequenced_emissive_primitives(&mut sequenced, ctx.clone());
    let reactions = vec![NamedReaction {
        name: "levelLoad".to_string(),
        descriptor: ReactionDescriptor::Sequence(
            steps
                .into_iter()
                .map(|(primitive, args)| SequenceStep {
                    id: id.into(),
                    primitive: primitive.to_string(),
                    args,
                })
                .collect(),
        ),
    }];
    let validated = validate_sequence_primitives(reactions, &sequenced);
    assert_eq!(validated.len(), 1, "emissive steps survive validation");
    let mut data = DataRegistry::new();
    data.populate_level(validated, Vec::new(), &[]);
    fire_named_event_with_sequences(
        "levelLoad",
        &data,
        &sequenced,
        &ReactionPrimitiveRegistry::new(),
        &postretro_scripting_core::reaction_registry::SystemReactionRegistry::new(),
        &ctx,
        None,
    );
    volume(&ctx.registry.borrow(), id)
}

// P2 and P3: within one body, writes land in order, so the later wins.
#[test]
fn in_one_body_the_later_of_a_brightness_write_and_an_install_wins() {
    let curve = serde_json::to_value(flicker()).unwrap();
    let brightness_last = run_body(vec![
        ("setEmissiveAnimation", curve.clone()),
        ("setEmissiveBrightness", json!({ "brightness": 0.25 })),
    ]);
    assert_eq!(
        (brightness_last.brightness, brightness_last.animation),
        (0.25, None)
    );

    let curve_last = run_body(vec![
        ("setEmissiveBrightness", json!({ "brightness": 0.25 })),
        ("setEmissiveAnimation", curve),
    ]);
    assert_eq!(curve_last.animation, Some(flicker()));
}

// P5: clearing with `null` stops the curve and leaves brightness where the
// last sample put it.
#[test]
fn clearing_with_null_keeps_the_last_brightness() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    animation(&mut registry, id, Some(flicker()));
    // The frame bridge wrote this sample last frame.
    let mut sampled = volume(&registry, id);
    sampled.brightness = 0.7;
    registry.set_component(id, sampled).unwrap();

    let warnings = dispatch(&mut registry, &[id], "setEmissiveAnimation", Value::Null);
    assert!(warnings.is_empty(), "{warnings:?}");
    let after = volume(&registry, id);
    assert_eq!((after.brightness, after.animation), (0.7, None));
}

// P6 and P30 at the primitive: an identical re-install continues the run; the
// same curve after a brightness write starts a new one.
#[test]
fn reinstalling_the_running_curve_continues_its_run_unless_a_write_cleared_it() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    animation(&mut registry, id, Some(flicker()));
    let run = volume(&registry, id).animation_serial;
    animation(&mut registry, id, Some(flicker()));
    assert_eq!(volume(&registry, id).animation_serial, run);

    brightness(&mut registry, id, 0.0);
    animation(&mut registry, id, Some(flicker()));
    let after = volume(&registry, id);
    assert_eq!(after.animation, Some(flicker()));
    assert_ne!(after.animation_serial, run, "the clear ended the old run");
}

// P7: a zero, negative or non-finite period (a `flicker` at rate 0 or NaN
// lowers to one) and an empty curve each warn and install nothing; the prior
// animation and brightness stand.
#[test]
fn an_invalid_period_or_empty_curve_warns_and_leaves_the_prior_state() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    animation(&mut registry, id, Some(flicker()));
    let mut prior = volume(&registry, id);
    prior.brightness = 0.6;
    registry.set_component(id, prior.clone()).unwrap();

    for period_ms in [0.0, -5.0, f32::INFINITY, f32::NAN] {
        let warnings = animation(
            &mut registry,
            id,
            Some(EmissiveAnimation {
                period_ms,
                ..fade()
            }),
        );
        assert_eq!(warnings.len(), 1, "periodMs {period_ms}: {warnings:?}");
        assert!(warnings[0].contains("periodMs must be > 0"));
        assert_eq!(volume(&registry, id), prior, "periodMs {period_ms}");
    }
    let warnings = animation(
        &mut registry,
        id,
        Some(EmissiveAnimation {
            brightness: Vec::new(),
            ..fade()
        }),
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("curve is empty"));
    assert_eq!(volume(&registry, id), prior);

    // The same refusals as a script sends them: JSON args through the
    // registered handler (a `fade` with `periodMs: 0`, a raw empty curve).
    for args in [
        json!({ "periodMs": 0, "playCount": 1, "brightness": [1, 0] }),
        json!({ "periodMs": 500, "brightness": [] }),
    ] {
        let warnings = dispatch(&mut registry, &[id], "setEmissiveAnimation", args.clone());
        assert_eq!(warnings.len(), 1, "{args}: {warnings:?}");
        assert_eq!(volume(&registry, id), prior, "{args}");
    }
}

// P8: playCount 0 reads 1, with a warning.
#[test]
fn play_count_zero_installs_as_one() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    let warnings = animation(
        &mut registry,
        id,
        Some(EmissiveAnimation {
            play_count: Some(0),
            ..fade()
        }),
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("playCount of 0"));
    assert_eq!(volume(&registry, id).animation.unwrap().play_count, Some(1));
}

// P10: brightness NaN reads 0; -0.5 and 1.5 clamp; each with one warning. A
// curve sample past 1 clamps too, one warning for the install.
#[test]
fn brightness_and_curve_samples_clamp_to_the_unit_range_with_one_warning() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    for (written, held) in [(f32::NAN, 0.0), (-0.5, 0.0), (1.5, 1.0)] {
        let warnings = brightness(&mut registry, id, written);
        assert_eq!(warnings.len(), 1, "brightness {written}: {warnings:?}");
        assert_eq!(
            volume(&registry, id).brightness,
            held,
            "brightness {written}"
        );
    }
    for valid in [0.0, 0.5, 1.0] {
        assert!(brightness(&mut registry, id, valid).is_empty());
    }

    let warnings = animation(
        &mut registry,
        id,
        Some(EmissiveAnimation {
            brightness: vec![0.2, 1.5, -1.0, f32::NAN],
            ..flicker()
        }),
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        volume(&registry, id).animation.unwrap().brightness,
        vec![0.2, 1.0, 0.0, 0.0]
    );
}

// P49: script colour channels above 1 clamp to 1; negative and non-finite
// read 0; one warning per write. A valid colour warns nothing.
#[test]
fn colour_channels_clamp_to_the_unit_range_with_one_warning() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    let warnings = dispatch(
        &mut registry,
        &[id],
        "setEmissiveColor",
        json!({ "color": [100, 100, 100] }),
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(volume(&registry, id).color, [1.0; 3]);

    let warnings = capture(|| {
        set_emissive_color::dispatch(
            &mut registry,
            &[id],
            &SetEmissiveColorArgs {
                color: [-0.5, f32::NAN, f32::INFINITY],
            },
        )
    });
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(volume(&registry, id).color, [0.0; 3]);

    let warnings = dispatch(
        &mut registry,
        &[id],
        "setEmissiveColor",
        json!({ "color": [1, 0.05, 0.02] }),
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(volume(&registry, id).color, [1.0, 0.05, 0.02]);
}

// P44: a colour write leaves brightness, the running curve and its run alone.
#[test]
fn a_colour_write_leaves_brightness_and_the_running_animation_alone() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    animation(&mut registry, id, Some(flicker()));
    let mut sampled = volume(&registry, id);
    sampled.brightness = 0.58;
    registry.set_component(id, sampled.clone()).unwrap();

    dispatch(
        &mut registry,
        &[id],
        "setEmissiveColor",
        json!({ "color": [1, 0.05, 0.02] }),
    );
    let after = volume(&registry, id);
    assert_eq!(after.color, [1.0, 0.05, 0.02]);
    assert_eq!(after.brightness, sampled.brightness);
    assert_eq!(after.animation, sampled.animation);
    assert_eq!(after.animation_serial, sampled.animation_serial);
}

// P11: a tag matching nothing is a quiet no-op; a tag matching a non-emissive
// entity warns for it, skips it, and still writes every volume.
#[test]
fn tag_dispatch_skips_non_volumes_with_a_warning_and_writes_the_rest() {
    let mut registry = EntityRegistry::new();
    let volume_a = spawn_volume(&mut registry);
    let bare = registry.spawn(Transform::default());
    let volume_b = spawn_volume(&mut registry);

    for (primitive, args) in [
        ("setEmissiveBrightness", json!({ "brightness": 0.5 })),
        ("setEmissiveColor", json!({ "color": [0, 1, 0] })),
        (
            "setEmissiveAnimation",
            serde_json::to_value(fade()).unwrap(),
        ),
        ("setEmissiveAnimation", Value::Null),
    ] {
        let none = dispatch(&mut registry, &[], primitive, args.clone());
        assert!(none.is_empty(), "{primitive}: an empty set warns nothing");

        let warnings = dispatch(&mut registry, &[volume_a, bare, volume_b], primitive, args);
        assert_eq!(warnings.len(), 1, "{primitive}: {warnings:?}");
        assert!(warnings[0].contains("no EmissiveVolumeComponent"));
    }
    for id in [volume_a, volume_b] {
        let after = volume(&registry, id);
        assert_eq!(
            (after.brightness, after.color, after.animation),
            (0.5, [0.0, 1.0, 0.0], None)
        );
    }
}

// A script value that is not the arg shape fails the dispatch, naming the
// primitive, and writes nothing.
#[test]
fn malformed_args_fail_naming_the_primitive() {
    let mut registry = EntityRegistry::new();
    let id = spawn_volume(&mut registry);
    let before = volume(&registry, id);
    let reactions = tag_registry();
    for (primitive, args) in [
        ("setEmissiveBrightness", json!({ "level": 1 })),
        ("setEmissiveColor", json!({ "color": [1, 0] })),
        ("setEmissiveAnimation", json!({ "periodMs": 100 })),
    ] {
        let error = reactions
            .dispatch(primitive, &mut registry, &[id], &args)
            .unwrap_err();
        let ReactionError::InvalidArgument { reason } = error;
        assert!(reason.starts_with(primitive), "{reason}");
    }
    assert_eq!(volume(&registry, id), before);
}
