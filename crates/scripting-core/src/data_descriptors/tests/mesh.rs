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
