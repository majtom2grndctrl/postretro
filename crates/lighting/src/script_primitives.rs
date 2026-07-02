// Light-domain scripting primitives: `setLightAnimation` definition context.
// Also owns world-query shared typedef registrations to preserve SDK typedef order:
// `WorldQueryComponent`, `WorldQueryFilter`, `Entity`, and `EmitterEntity`.
// See: context/lib/scripting.md

use postretro_entities::components::light::{LightAnimation, LightComponent};
use postretro_entities::{
    Component, ComponentKind, EntityId, EntityRegistry, ScriptCtx, ScriptError,
};
use postretro_foundation::Vec3Lit;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::sequence::{SequenceError, SequencedPrimitiveRegistry};

/// A single entity-handle snapshot produced by `world.query`. Carries the
/// `EntityId` plus a read-only copy of the live component data at query time.
#[derive(Debug, Clone)]
pub struct LightQueryHandle {
    id: EntityId,
    component: LightComponent,
    tags: Vec<String>,
}

pub fn collect_light_handles(ctx: &ScriptCtx, tag: Option<&str>) -> Vec<LightQueryHandle> {
    let reg = ctx.registry.borrow();
    let mut out = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::Light, tag) {
        let Some(light) = LightComponent::from_value(value) else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        out.push(LightQueryHandle {
            id,
            component: light.clone(),
            tags,
        });
    }
    out
}

/// Serialize a handle slice into the JSON array returned to scripts. The SDK
/// wrappers (`sdk/lib/entities/lights.ts` and `lights.luau`) call
/// `wrapLightEntity` on each entry to attach script-facing methods.
pub fn handles_to_json(handles: Vec<LightQueryHandle>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let arr: Vec<Value> = handles
        .into_iter()
        .map(|h| {
            // `LightComponent` carries `#[serde(rename_all = "camelCase")]`,
            // so direct serialization yields the script-facing key shape.
            let comp =
                serde_json::to_value(&h.component).expect("LightComponent always serializes");
            let mut obj = Map::with_capacity(5);
            obj.insert("id".to_string(), Value::from(h.id.to_raw()));
            let [x, y, z] = h.component.origin;
            let mut position = Map::with_capacity(3);
            position.insert("x".to_string(), Value::from(x as f64));
            position.insert("y".to_string(), Value::from(y as f64));
            position.insert("z".to_string(), Value::from(z as f64));
            obj.insert("position".to_string(), Value::Object(position));
            obj.insert("isDynamic".to_string(), Value::from(h.component.is_dynamic));
            obj.insert(
                "tags".to_string(),
                Value::Array(h.tags.into_iter().map(Value::String).collect()),
            );
            obj.insert("component".to_string(), comp);
            Value::Object(obj)
        })
        .collect();
    Value::Array(arr)
}

fn validate_and_normalize(
    mut anim: LightAnimation,
    _target_is_dynamic: bool,
) -> Result<LightAnimation, ScriptError> {
    if !anim.period_ms.is_finite() || anim.period_ms <= 0.0 {
        return Err(ScriptError::InvalidArgument {
            reason: format!("periodMs must be > 0 (got {})", anim.period_ms),
        });
    }
    if let Some(ref b) = anim.brightness
        && b.is_empty()
    {
        return Err(ScriptError::InvalidArgument {
            reason: "brightness channel present but empty (use null to omit)".into(),
        });
    }
    if let Some(ref c) = anim.color {
        if c.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "color channel present but empty (use null to omit)".into(),
            });
        }
        // Task 1b: relax the previous `is_dynamic` gate. The geometry-axis
        // redefinition of `is_dynamic` (Task 1b spec) plus the
        // animated-baked compose path (Task 2c) means script-driven
        // intensity/color on a static light no longer drifts from the SH
        // bake — animated-baked lights route their per-frame radiance
        // through the compose pass, which fuses the per-frame dominant
        // direction and feeds the SDF shadow trace. The old "color
        // animation requires dynamic" rule is incompatible with that
        // path; remove it. Brightness-only animation on a now-static
        // light is admitted unchanged.
        //
        // Historical source: context/plans/done/sdf-static-occluder-shadows/.
    }
    if let Some(ref mut dirs) = anim.direction {
        if dirs.is_empty() {
            return Err(ScriptError::InvalidArgument {
                reason: "direction channel present but empty (use null to omit)".into(),
            });
        }
        for (i, sample) in dirs.iter_mut().enumerate() {
            let [x, y, z] = sample.as_f32_3();
            let len_sq = x * x + y * y + z * z;
            if !len_sq.is_finite() || len_sq <= 1.0e-12 {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "direction sample {i} has zero / non-finite length ({:?})",
                        sample
                    ),
                });
            }
            let len = len_sq.sqrt();
            // Unit-length invariant enforced here — GPU evaluator assumes normalized direction.
            *sample = Vec3Lit([x / len, y / len, z / len]);
        }
    }
    // rem_euclid matches the GPU evaluator's phase wrap behavior.
    if let Some(p) = anim.phase {
        let normalized = if p.is_finite() {
            p.rem_euclid(1.0)
        } else {
            0.0
        };
        anim.phase = Some(normalized);
    }
    Ok(anim)
}

fn apply_light_animation(
    ctx: &ScriptCtx,
    id: EntityId,
    animation: Option<LightAnimation>,
) -> Result<(), ScriptError> {
    let mut reg = ctx.registry.borrow_mut();
    apply_light_animation_inner(&mut reg, id, animation)
}

/// Takes an already-borrowed registry to avoid a second `borrow_mut` on the same `RefCell` guard.
pub(crate) fn apply_light_animation_inner(
    registry: &mut EntityRegistry,
    id: EntityId,
    animation: Option<LightAnimation>,
) -> Result<(), ScriptError> {
    let current = registry
        .get_component::<LightComponent>(id)
        .map_err(ScriptError::from)?
        .clone();

    let validated = match animation {
        Some(a) => Some(validate_and_normalize(a, current.is_dynamic)?),
        None => None,
    };

    let mut next = current;
    next.animation = validated;
    registry
        .set_component(id, next)
        .map_err(ScriptError::from)?;
    Ok(())
}

const SET_LIGHT_ANIM_DOC: &str = "Overwrite the LightComponent.animation on the given entity. Pass null/nil to clear. \
     Non-unit direction samples are silently normalized; zero-length direction samples \
     and empty channel arrays error with InvalidArgument. \
     Definition context.";

#[allow(clippy::arc_with_non_send_sync)]
pub fn register_light_entity_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_set_light_animation(registry, ctx);
}

fn register_set_light_animation(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("setLightAnimation", {
            let ctx = ctx.clone();
            move |id: EntityId, animation: Option<LightAnimation>| -> Result<(), ScriptError> {
                apply_light_animation(&ctx, id, animation)
            }
        })
        .scope(ContextScope::DefinitionOnly)
        .doc(SET_LIGHT_ANIM_DOC)
        .param("id", "EntityId")
        .param("animation", "LightAnimation | null")
        .finish();
}

pub fn register_sequenced_light_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    registry.register("setLightAnimation", move |id, args| {
        let animation: Option<LightAnimation> =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setLightAnimation: failed to deserialize args: {e}"),
            })?;
        apply_light_animation(&ctx, id, animation).map_err(script_to_sequence_error)
    });
}

fn script_to_sequence_error(err: ScriptError) -> SequenceError {
    match err {
        ScriptError::InvalidArgument { reason } => SequenceError::InvalidArgument { reason },
        // EntityNotFound is pre-filtered by dispatch_sequence; this arm is a defensive path.
        other => SequenceError::ExecutionFailed {
            reason: other.to_string(),
        },
    }
}

/// Complements the engine shared type registrar.
pub fn register_shared_types(registry: &mut PrimitiveRegistry) {
    registry
        .register_enum("LightKind")
        .variant("Point", "")
        .variant("Spot", "")
        .variant("Directional", "")
        .finish();
    registry
        .register_enum("FalloffKind")
        .variant("Linear", "")
        .variant("InverseDistance", "")
        .variant("InverseSquared", "")
        .finish();
    registry
        .register_type("LightAnimation")
        .field("periodMs", "f32", "Total period of the loop, in milliseconds.")
        .field(
            "phase",
            "Option<f32>",
            "Starting phase in [0.0, 1.0). Values outside this range are normalized via rem_euclid.",
        )
        .field(
            "playCount",
            "Option<u32>",
            "Total full periods to play; null loops forever.",
        )
        .field(
            "startActive",
            "Option<bool>",
            "Whether the animation starts in the active state. null defaults to true; false mirrors the FGD `_start_inactive` flag.",
        )
        .field(
            "brightness",
            "Option<Vec<f32>>",
            "Per-sample brightness curve.",
        )
        .field(
            "color",
            "Option<Vec<Vec3>>",
            "Per-sample color curve. Accepted on dynamic and authored static lights; baked indirect stays at the authored color.",
        )
        .field(
            "direction",
            "Option<Vec<Vec3>>",
            "Per-sample direction curve. Non-unit samples are silently normalized.",
        )
        .finish();
    registry
        .register_type("LightComponent")
        .field("origin", "Vec3", "")
        .field("lightType", "LightKind", "")
        .field("intensity", "f32", "")
        .field("color", "Vec3", "")
        .field("falloffModel", "FalloffKind", "")
        .field("falloffRange", "f32", "")
        .field("coneAngleInner", "Option<f32>", "")
        .field("coneAngleOuter", "Option<f32>", "")
        .field("coneDirection", "Option<Vec3>", "")
        .field("isDynamic", "bool", "")
        .field("animation", "Option<LightAnimation>", "")
        .finish();
    registry
        .register_enum("WorldQueryComponent")
        .doc("Component-name literals accepted by `worldQuery` and the `world.query` SDK wrapper. New queryable component types extend this union.")
        .variant("light", "")
        .variant("transform", "")
        .variant("emitter", "")
        .variant("fog_volume", "")
        .variant("particle", "Always returns []. Engine-managed; scripts never iterate individual particles.")
        .variant("sprite_visual", "Always returns []. Engine-managed.")
        .finish();
    registry
        .register_type("WorldQueryFilter")
        .field(
            "component",
            "WorldQueryComponent",
            "Component name to query.",
        )
        .field(
            "tag",
            "Option<String>",
            "Optional tag filter (exact string match).",
        )
        .finish();
    registry
        .register_type("Entity")
        .doc("Generic entity handle returned by `world.query` when the component type is not known at compile time.")
        .field("id", "EntityId", "")
        .field("position", "Vec3", "Entity position at query time.")
        .field("tags", "Vec<String>", "The entity's tags at query time. Empty array if untagged.")
        .finish();
    registry
        .register_type("EmitterEntity")
        .doc("Entity handle returned by `world.query` when filtering for billboard emitter entities.")
        .field("id", "EntityId", "")
        .field("position", "Vec3", "Emitter position at query time (from the entity's Transform).")
        .field(
            "tags",
            "Vec<String>",
            "The entity's tags at query time. Empty array if untagged.",
        )
        .field(
            "component",
            "BillboardEmitterComponent",
            "Full emitter component snapshot at query time.",
        )
        .finish();
    registry
        .register_type("LightEntity")
        .doc("Entity handle returned by `world.query` when filtering for light entities.")
        .field("id", "EntityId", "")
        .field("position", "Vec3", "Light origin at query time.")
        .field(
            "isDynamic",
            "bool",
            "Whether the light is driven by the runtime dynamic-light buffer. Static lights baked from FGD entities are not; descriptor-spawned lights always are.",
        )
        .field(
            "tags",
            "Vec<String>",
            "The entity's tags at query time. Empty array if untagged.",
        )
        .field(
            "component",
            "LightComponent",
            "Full component snapshot at query time.",
        )
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::light::{FalloffKind, LightKind};

    fn test_ctx_with_light(is_dynamic: bool, tag: Option<&str>) -> (ScriptCtx, EntityId) {
        let ctx = ScriptCtx::new();
        let id;
        {
            let mut reg = ctx.registry.borrow_mut();
            id = reg.spawn(Transform::default());
            reg.set_component(
                id,
                LightComponent {
                    origin: [1.0, 2.0, 3.0],
                    light_type: LightKind::Point,
                    intensity: 1.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: FalloffKind::InverseSquared,
                    falloff_range: 10.0,
                    cone_angle_inner: None,
                    cone_angle_outer: None,
                    cone_direction: None,
                    is_dynamic,
                    animated_slot: None,
                    animation: None,
                },
            )
            .unwrap();
            if let Some(t) = tag {
                reg.set_tags(id, vec![t.to_string()]).unwrap();
            }
        }
        (ctx, id)
    }

    fn install_all(registry: &super::PrimitiveRegistry, qjs: &rquickjs::Ctx<'_>) {
        for p in registry.iter() {
            (p.quickjs_installer)(qjs).unwrap();
        }
    }

    fn install_all_lua(registry: &super::PrimitiveRegistry, lua: &mlua::Lua) {
        for p in registry.iter() {
            (p.luau_installer)(lua).unwrap();
        }
    }

    fn registry_for(ctx: ScriptCtx) -> PrimitiveRegistry {
        let mut r = PrimitiveRegistry::new();
        register_light_entity_primitives(&mut r, ctx);
        r
    }

    #[test]
    fn world_query_returns_all_light_bearing_entities() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let handles = collect_light_handles(&ctx, None);
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].id, id);
        assert!(handles[0].component.is_dynamic);
    }

    #[test]
    fn world_query_tag_filter_narrows_result() {
        let (ctx, _) = test_ctx_with_light(true, Some("hallway_wave"));
        let other;
        {
            let mut reg = ctx.registry.borrow_mut();
            other = reg.spawn(Transform::default());
            reg.set_component(
                other,
                LightComponent {
                    origin: [5.0, 5.0, 5.0],
                    light_type: LightKind::Point,
                    intensity: 1.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: FalloffKind::InverseSquared,
                    falloff_range: 10.0,
                    cone_angle_inner: None,
                    cone_angle_outer: None,
                    cone_direction: None,
                    is_dynamic: true,
                    animated_slot: None,
                    animation: None,
                },
            )
            .unwrap();
        }
        let matched = collect_light_handles(&ctx, Some("hallway_wave"));
        assert_eq!(matched.len(), 1);
        assert_ne!(matched[0].id, other);
    }

    #[test]
    fn world_query_empty_when_no_match() {
        let (ctx, _) = test_ctx_with_light(true, None);
        let handles = collect_light_handles(&ctx, Some("nonexistent_tag"));
        assert!(handles.is_empty());
    }

    #[test]
    fn set_light_animation_updates_registry() {
        let (ctx, id) = test_ctx_with_light(true, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 500.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.1, 0.9]),
                color: None,
                direction: None,
            }),
        )
        .unwrap();
        let reg = ctx.registry.borrow();
        let light = reg.get_component::<LightComponent>(id).unwrap();
        assert!(light.animation.is_some());
        assert_eq!(light.animation.as_ref().unwrap().period_ms, 500.0);
    }

    #[test]
    fn set_light_animation_null_clears_animation() {
        let (ctx, id) = test_ctx_with_light(true, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 500.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.1, 0.9]),
                color: None,
                direction: None,
            }),
        )
        .unwrap();
        apply_light_animation(&ctx, id, None).unwrap();
        let reg = ctx.registry.borrow();
        assert!(
            reg.get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_none()
        );
    }

    #[test]
    fn set_light_animation_rejects_entity_not_found() {
        let ctx = ScriptCtx::new();
        let bogus = EntityId::from_raw(0x0000_0001);
        let err = apply_light_animation(&ctx, bogus, None).unwrap_err();
        assert!(matches!(err, ScriptError::EntityNotFound(_)));
    }

    #[test]
    fn set_light_animation_rejects_entity_with_no_light_component() {
        let ctx = ScriptCtx::new();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let err = apply_light_animation(&ctx, id, None).unwrap_err();
        assert!(matches!(err, ScriptError::ComponentNotFound { .. }));
    }

    #[test]
    fn set_light_animation_rejects_zero_period() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let err = apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 0.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.1, 1.0]),
                color: None,
                direction: None,
            }),
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
    }

    #[test]
    fn set_light_animation_rejects_empty_required_channel() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let err = apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![]),
                color: None,
                direction: None,
            }),
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
    }

    /// Task 1b: brightness-only animation on a now-static
    /// (`is_dynamic == false`) script-driven-intensity light is admitted —
    /// the previous `is_dynamic` gate was incompatible with the
    /// animated-baked compose path. AC: "`setLightAnimation` accepts
    /// brightness-only animation on a now-static script-driven-intensity
    /// light without error."
    #[test]
    fn set_light_animation_accepts_brightness_on_static_script_driven_light() {
        let (ctx, id) = test_ctx_with_light(false, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.1, 0.9]),
                color: None,
                direction: None,
            }),
        )
        .expect("brightness-only on a static light must be admitted");
    }

    /// Task 1b: the previous "color animation requires dynamic" gate is
    /// retired alongside the geometry-axis redefinition of `is_dynamic`.
    /// Color animation routes through the animated-baked compose path
    /// (Task 2c), which fuses per-frame radiance — no SH bake drift.
    #[test]
    fn set_light_animation_accepts_color_on_static_light_after_task_1b() {
        let (ctx, id) = test_ctx_with_light(false, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: None,
                color: Some(vec![Vec3Lit([1.0, 0.0, 0.0])]),
                direction: None,
            }),
        )
        .expect("color animation on a static light is admitted post-1b");
    }

    #[test]
    fn set_light_animation_normalizes_phase() {
        let (ctx, id) = test_ctx_with_light(true, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: Some(2.75),
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.1, 1.0]),
                color: None,
                direction: None,
            }),
        )
        .unwrap();
        let reg = ctx.registry.borrow();
        let stored = reg
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .phase
            .unwrap();
        assert!((stored - 0.75).abs() < 1e-5, "phase: {stored}");
    }

    #[test]
    fn set_light_animation_normalizes_direction_samples() {
        let (ctx, id) = test_ctx_with_light(true, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: None,
                color: None,
                direction: Some(vec![Vec3Lit([2.0, 0.0, 0.0]), Vec3Lit([0.0, 3.0, 4.0])]),
            }),
        )
        .unwrap();
        let reg = ctx.registry.borrow();
        let dirs = reg
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .direction
            .clone()
            .unwrap();
        let d0 = dirs[0].as_f32_3();
        let d1 = dirs[1].as_f32_3();
        let len0 = (d0[0].powi(2) + d0[1].powi(2) + d0[2].powi(2)).sqrt();
        let len1 = (d1[0].powi(2) + d1[1].powi(2) + d1[2].powi(2)).sqrt();
        assert!((len0 - 1.0).abs() < 1e-5, "dir[0]: {:?}", dirs[0]);
        assert!((len1 - 1.0).abs() < 1e-5, "dir[1]: {:?}", dirs[1]);
    }

    #[test]
    fn set_light_animation_rejects_zero_length_direction() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let err = apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: None,
                color: None,
                direction: Some(vec![Vec3Lit([0.0, 0.0, 0.0])]),
            }),
        )
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
    }

    #[test]
    fn sequenced_set_light_animation_registers_under_expected_name() {
        let ctx = ScriptCtx::new();
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx);
        assert!(seq_reg.contains("setLightAnimation"));
    }

    #[test]
    fn sequenced_set_light_animation_applies_animation() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx.clone());

        let handler = seq_reg.get("setLightAnimation").unwrap();
        let args = serde_json::json!({
            "periodMs": 250.0,
            "brightness": [0.0, 1.0],
        });
        handler(id, &args).unwrap();

        let reg = ctx.registry.borrow();
        let stored = reg
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(stored.period_ms, 250.0);
        assert_eq!(stored.brightness.as_ref().unwrap(), &vec![0.0, 1.0]);
    }

    #[test]
    fn sequenced_set_light_animation_null_clears_animation() {
        let (ctx, id) = test_ctx_with_light(true, None);
        apply_light_animation(
            &ctx,
            id,
            Some(LightAnimation {
                period_ms: 100.0,
                phase: None,
                play_count: None,
                start_active: None,
                brightness: Some(vec![0.0, 1.0]),
                color: None,
                direction: None,
            }),
        )
        .unwrap();

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx.clone());
        let handler = seq_reg.get("setLightAnimation").unwrap();
        handler(id, &serde_json::Value::Null).unwrap();

        let reg = ctx.registry.borrow();
        assert!(
            reg.get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_none()
        );
    }

    #[test]
    fn sequenced_set_light_animation_rejects_zero_length_direction() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx.clone());
        let handler = seq_reg.get("setLightAnimation").unwrap();
        let args = serde_json::json!({
            "periodMs": 100.0,
            "direction": [{ "x": 0.0, "y": 0.0, "z": 0.0 }],
        });
        let err = handler(id, &args).unwrap_err();
        assert!(
            matches!(err, SequenceError::InvalidArgument { .. }),
            "got: {err:?}"
        );
    }

    /// Task 1b: sequenced path mirrors the script-side relaxation —
    /// color animation on a static light is admitted.
    #[test]
    fn sequenced_set_light_animation_accepts_color_on_static_light_after_task_1b() {
        let (ctx, id) = test_ctx_with_light(false, None);
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx.clone());
        let handler = seq_reg.get("setLightAnimation").unwrap();
        let args = serde_json::json!({
            "periodMs": 100.0,
            "color": [{ "x": 1.0, "y": 0.0, "z": 0.0 }],
        });
        handler(id, &args).expect("color on static light admitted post-1b");
    }

    #[test]
    fn sequenced_set_light_animation_rejects_malformed_args() {
        let (ctx, id) = test_ctx_with_light(true, None);
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_light_primitives(&mut seq_reg, ctx.clone());
        let handler = seq_reg.get("setLightAnimation").unwrap();
        let args = serde_json::json!({ "periodMs": "fast" });
        let err = handler(id, &args).unwrap_err();
        assert!(
            matches!(err, SequenceError::InvalidArgument { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn set_light_animation_quickjs_and_luau_produce_identical_output() {
        // Cross-runtime parity: same call through QuickJS and Luau must yield
        // bitwise-identical LightAnimation in the registry.
        let (ctx, id) = test_ctx_with_light(true, None);
        let r = registry_for(ctx.clone());

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        let raw = id.to_raw();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let script = format!(
                r#"
                setLightAnimation({raw}, {{
                    periodMs: 500,
                    phase: 0.25,
                    playCount: 4,
                    startActive: false,
                    brightness: [0.1, 1.0, 0.1],
                    color: [{{ x: 1, y: 0, z: 0 }}, {{ x: 0, y: 0, z: 1 }}],
                    direction: null,
                }});
                "#
            );
            let _: () = qjs.eval(script.as_str()).unwrap();
        });
        let from_quickjs = ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("QuickJS set_light_animation must have populated animation");

        apply_light_animation(&ctx, id, None).unwrap();
        assert!(
            ctx.registry
                .borrow()
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_none()
        );

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        lua.load(format!(
            r#"
            setLightAnimation({raw}, {{
                periodMs = 500,
                phase = 0.25,
                playCount = 4,
                startActive = false,
                brightness = {{0.1, 1.0, 0.1}},
                color = {{ {{x=1, y=0, z=0}}, {{x=0, y=0, z=1}} }},
                direction = nil,
            }})
            "#
        ))
        .exec()
        .unwrap();
        let from_luau = ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .clone()
            .expect("Luau set_light_animation must have populated animation");

        assert_eq!(
            from_quickjs, from_luau,
            "QuickJS and Luau must produce identical LightAnimation values for the same input"
        );
    }
}
