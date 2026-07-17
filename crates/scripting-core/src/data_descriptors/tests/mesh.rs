// Tests: mesh component parsing.

use super::super::*;
use super::common::*;

// --- components.mesh -----------------------------------------------------

#[test]
fn js_mesh_stateless_parses_model_only() {
    let src = r#"({ components: { mesh: { model: "decraniated" } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert_eq!(mesh.model, "decraniated");
    assert!(
        mesh.animations.is_empty() && mesh.default_state.is_none(),
        "no animations block ⇒ stateless"
    );
}

#[test]
fn js_mesh_shadow_bias_scale_defaults_and_validates() {
    let default = eval_js(r#"({ components: { mesh: { model: "m" } } })"#, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(
        (default.mesh.unwrap().shadow_bias_scale - 1.0).abs() < f32::EPSILON,
        "omitted shadowBiasScale must preserve the default"
    );

    let authored = eval_js(
        r#"({ components: { mesh: { model: "m", shadowBiasScale: 2.5 } } })"#,
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    assert!((authored.mesh.unwrap().shadow_bias_scale - 2.5).abs() < f32::EPSILON);

    for (source, expected) in [
        (
            r#"({ components: { mesh: { model: "m", shadowBiasScale: 0.0 } } })"#,
            0.0,
        ),
        (
            r#"({ components: { mesh: { model: "m", shadowBiasScale: 4.0 } } })"#,
            4.0,
        ),
    ] {
        let descriptor = eval_js(source, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
        assert!((descriptor.mesh.unwrap().shadow_bias_scale - expected).abs() < f32::EPSILON);
    }

    for source in [
        r#"({ components: { mesh: { model: "m", shadowBiasScale: -0.1 } } })"#,
        r#"({ components: { mesh: { model: "m", shadowBiasScale: 4.1 } } })"#,
        r#"({ components: { mesh: { model: "m", shadowBiasScale: 1 / 0 } } })"#,
        r#"({ components: { mesh: { model: "m", shadowBiasScale: 0 / 0 } } })"#,
    ] {
        let err = eval_js(source, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        let DescriptorError::InvalidShape { reason } = err else {
            panic!("out-of-range shadowBiasScale must be a validation error");
        };
        assert!(
            reason.contains("shadowBiasScale") && reason.contains("0.0..=4.0"),
            "error must identify the authored field and valid range: {reason}"
        );
    }
}

#[test]
fn js_mesh_animated_parses_states_and_default() {
    let src = r#"({ components: { mesh: {
        model: "decraniated",
        defaultState: "idle",
        animations: {
            idle:   { clip: "idle_clip", loop: true, crossfadeMs: 120, interrupt: "smooth" },
            attack: { clip: "attack_clip", loop: false }
        }
    } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert_eq!(mesh.default_state.as_deref(), Some("idle"));
    assert_eq!(mesh.animations.len(), 2);
    let idle = &mesh.animations["idle"];
    assert_eq!(idle.clip, "idle_clip");
    assert!(idle.looping);
    assert_eq!(idle.crossfade_ms, 120.0);
    assert_eq!(idle.interrupt, InterruptPolicy::Smooth);
    assert!(idle.clip_index.is_none(), "clip_index unresolved at parse");
    // Absent `crossfadeMs`/`interrupt` default; absent `loop` ⇒ false.
    let attack = &mesh.animations["attack"];
    assert!(!attack.looping);
    assert_eq!(
        attack.crossfade_ms,
        crate::components::mesh::DEFAULT_CROSSFADE_MS
    );
    assert_eq!(attack.interrupt, InterruptPolicy::Smooth);
}

#[test]
fn js_mesh_interrupt_snap_parses() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "die",
        animations: { die: { clip: "death", interrupt: "snap" } }
    } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(
        d.mesh.unwrap().animations["die"].interrupt,
        InterruptPolicy::Snap
    );
}

#[test]
fn js_mesh_empty_model_is_rejected() {
    let src = r#"({ components: { mesh: { model: "" } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_empty_clip_is_rejected() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "" } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_negative_crossfade_is_rejected() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "c", crossfadeMs: -1 } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_travel_speed_rejects_non_positive() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "c", travelSpeed: 0 } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_travel_speed_rejects_negative() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "c", travelSpeed: -3 } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_travel_speed_rejects_non_finite() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "c", travelSpeed: 1/0 } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_locomotion_speed_scale_false_parses() {
    let src = r#"({ components: { mesh: {
        model: "m",
        locomotion: { speedScale: false }
    } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    let locomotion = mesh.locomotion.expect("locomotion block parsed");
    assert!(!locomotion.speed_scale);
}

#[test]
fn js_mesh_locomotion_absent_defaults_speed_scale_true() {
    let src = r#"({ components: { mesh: { model: "m" } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert!(
        mesh.locomotion.is_none(),
        "absent `locomotion` block parses to None"
    );
    assert!(
        mesh.speed_scale(),
        "shared descriptor default is rate-scaled"
    );
}

#[test]
fn js_mesh_locomotion_present_without_field_defaults_speed_scale_true() {
    let src = r#"({ components: { mesh: { model: "m", locomotion: {} } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert!(d.mesh.unwrap().speed_scale());
}

#[test]
fn js_mesh_unknown_interrupt_is_rejected() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "idle",
        animations: { idle: { clip: "c", interrupt: "instant" } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_animations_without_default_state_is_rejected() {
    let src = r#"({ components: { mesh: {
        model: "m",
        animations: { idle: { clip: "c" } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "defaultState"
        }
    );
}

#[test]
fn js_mesh_default_state_not_declared_is_rejected() {
    let src = r#"({ components: { mesh: {
        model: "m", defaultState: "nope",
        animations: { idle: { clip: "c" } }
    } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_present_empty_animations_is_rejected() {
    let src = r#"({ components: { mesh: { model: "m", animations: {} } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_mesh_default_state_without_animations_is_rejected() {
    let src = r#"({ components: { mesh: { model: "m", defaultState: "idle" } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_stateless_parses_model_only() {
    let src = r#"return { components = { mesh = { model = "decraniated" } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert_eq!(mesh.model, "decraniated");
    assert!(mesh.animations.is_empty() && mesh.default_state.is_none());
}

#[test]
fn lua_mesh_shadow_bias_scale_defaults_and_validates() {
    let default = eval_lua(
        r#"return { components = { mesh = { model = "m" } } }"#,
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    assert!(
        (default.mesh.unwrap().shadow_bias_scale - 1.0).abs() < f32::EPSILON,
        "omitted shadowBiasScale must preserve the default"
    );

    let authored = eval_lua(
        r#"return { components = { mesh = { model = "m", shadowBiasScale = 2.5 } } }"#,
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    assert!((authored.mesh.unwrap().shadow_bias_scale - 2.5).abs() < f32::EPSILON);

    for (source, expected) in [
        (
            r#"return { components = { mesh = { model = "m", shadowBiasScale = 0.0 } } }"#,
            0.0,
        ),
        (
            r#"return { components = { mesh = { model = "m", shadowBiasScale = 4.0 } } }"#,
            4.0,
        ),
    ] {
        let descriptor = eval_lua(source, |v| entity_descriptor_from_lua(v).unwrap());
        assert!((descriptor.mesh.unwrap().shadow_bias_scale - expected).abs() < f32::EPSILON);
    }

    for source in [
        r#"return { components = { mesh = { model = "m", shadowBiasScale = -0.1 } } }"#,
        r#"return { components = { mesh = { model = "m", shadowBiasScale = 4.1 } } }"#,
        r#"return { components = { mesh = { model = "m", shadowBiasScale = 1 / 0 } } }"#,
        r#"return { components = { mesh = { model = "m", shadowBiasScale = 0 / 0 } } }"#,
    ] {
        let err = eval_lua(source, |v| entity_descriptor_from_lua(v).unwrap_err());
        let DescriptorError::InvalidShape { reason } = err else {
            panic!("out-of-range shadowBiasScale must be a validation error");
        };
        assert!(
            reason.contains("shadowBiasScale") && reason.contains("0.0..=4.0"),
            "error must identify the authored field and valid range: {reason}"
        );
    }
}

#[test]
fn lua_mesh_animated_parses_states_and_default() {
    let src = r#"return { components = { mesh = {
        model = "decraniated",
        defaultState = "idle",
        animations = {
            idle = { clip = "idle_clip", loop = true, crossfadeMs = 120, interrupt = "snap" },
            attack = { clip = "attack_clip" }
        }
    } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert_eq!(mesh.default_state.as_deref(), Some("idle"));
    assert_eq!(mesh.animations.len(), 2);
    assert_eq!(mesh.animations["idle"].interrupt, InterruptPolicy::Snap);
    assert!(mesh.animations["idle"].looping);
    // Absent `loop` ⇒ false; absent `crossfadeMs`/`interrupt` ⇒ defaults.
    let attack = &mesh.animations["attack"];
    assert!(!attack.looping);
    assert_eq!(attack.interrupt, InterruptPolicy::Smooth);
    assert_eq!(
        attack.crossfade_ms,
        crate::components::mesh::DEFAULT_CROSSFADE_MS
    );
}

#[test]
fn lua_mesh_empty_model_is_rejected() {
    let src = r#"return { components = { mesh = { model = "" } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_animations_without_default_state_is_rejected() {
    let src = r#"return { components = { mesh = {
        model = "m",
        animations = { idle = { clip = "c" } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "defaultState"
        }
    );
}

#[test]
fn lua_mesh_default_state_without_animations_is_rejected() {
    let src = r#"return { components = { mesh = { model = "m", defaultState = "idle" } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_default_state_not_declared_is_rejected() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "nope",
        animations = { idle = { clip = "c" } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_present_empty_animations_is_rejected() {
    // A present-but-empty `animations` table is rejected: the table value
    // IS present, so `animations_present` is true and the empty map is
    // rejected.
    let src = r#"return { components = { mesh = { model = "m", animations = {} } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_unknown_interrupt_is_rejected() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "idle",
        animations = { idle = { clip = "c", interrupt = "instant" } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_negative_crossfade_is_rejected() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "idle",
        animations = { idle = { clip = "c", crossfadeMs = -2.0 } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_travel_speed_rejects_non_positive() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "idle",
        animations = { idle = { clip = "c", travelSpeed = 0 } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_travel_speed_rejects_negative() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "idle",
        animations = { idle = { clip = "c", travelSpeed = -3.0 } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_travel_speed_rejects_non_finite() {
    let src = r#"return { components = { mesh = {
        model = "m", defaultState = "idle",
        animations = { idle = { clip = "c", travelSpeed = 1/0 } }
    } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_mesh_locomotion_speed_scale_false_parses() {
    let src = r#"return { components = { mesh = {
        model = "m",
        locomotion = { speedScale = false }
    } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    let locomotion = mesh.locomotion.expect("locomotion block parsed");
    assert!(!locomotion.speed_scale);
}

#[test]
fn lua_mesh_locomotion_absent_defaults_speed_scale_true() {
    let src = r#"return { components = { mesh = { model = "m" } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let mesh = d.mesh.expect("mesh descriptor parsed");
    assert!(
        mesh.locomotion.is_none(),
        "absent `locomotion` block parses to None"
    );
    assert!(
        mesh.speed_scale(),
        "shared descriptor default is rate-scaled"
    );
}

#[test]
fn lua_mesh_locomotion_present_without_field_defaults_speed_scale_true() {
    let src = r#"return { components = { mesh = { model = "m", locomotion = {} } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert!(d.mesh.unwrap().speed_scale());
}
