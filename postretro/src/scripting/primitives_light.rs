// Scripting primitives: `set_light_animation` (behavior-only).
// `world_query` lives in `primitives.rs` — it is a generic ECS primitive
// that happens to support lights; keeping it with the other entity primitives
// means adding a second queryable component only requires editing that file.
// See: context/lib/scripting.md

use super::components::light::{LightAnimation, LightComponent};
use super::conv::Vec3Lit;
use super::ctx::ScriptCtx;
use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry};
use super::registry::{Component, ComponentKind, EntityId};

// --- Shared logic: set_light_animation --------------------------------------
//
// Both runtimes go through these so behavior stays identical across QuickJS
// and Luau (see plan-2, settled-decisions: "no runtime gets a superset").

/// A single entity-handle snapshot produced by `world.query`. Carries the
/// `EntityId` plus a read-only copy of the live component data at query time.
/// The `world.ts` vocabulary module wraps this into a `LightEntity` script-visible object.
#[derive(Debug, Clone)]
pub(super) struct LightQueryHandle {
    id: EntityId,
    component: LightComponent,
    tags: Vec<String>,
}

/// Build every light-entity handle that matches the supplied `tag` filter.
/// Returns an empty vec when no entities match.
pub(super) fn collect_light_handles(ctx: &ScriptCtx, tag: Option<&str>) -> Vec<LightQueryHandle> {
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

/// Serialize a query-handle list into a serde_json array the FFI layers
/// forward to JS/Lua. Keeps the wire shape identical across runtimes.
pub(super) fn handles_to_json(handles: Vec<LightQueryHandle>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let arr: Vec<Value> = handles
        .into_iter()
        .map(|h| {
            // Component serializes with snake_case; rename only the well-known
            // scripting-facing keys to match SP7's external API spec.
            let comp = serialize_light_component_camel(&h.component);
            let mut obj = Map::with_capacity(5);
            obj.insert("id".to_string(), Value::from(h.id.to_raw()));
            let mut transform = Map::with_capacity(1);
            let [x, y, z] = h.component.origin;
            let mut position = Map::with_capacity(3);
            position.insert("x".to_string(), Value::from(x as f64));
            position.insert("y".to_string(), Value::from(y as f64));
            position.insert("z".to_string(), Value::from(z as f64));
            transform.insert("position".to_string(), Value::Object(position));
            obj.insert("transform".to_string(), Value::Object(transform));
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

/// Serialize a `LightComponent` into serde_json with `camelCase` for the
/// script-facing keys on the `animation` sub-object. Mirrors the renames in
/// the `LightAnimation` FFI impl in `conv.rs`.
fn serialize_light_component_camel(light: &LightComponent) -> serde_json::Value {
    let raw = serde_json::to_value(light).expect("LightComponent always serializes");
    rename_animation_keys(raw)
}

fn rename_animation_keys(mut value: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(ref mut obj) = value
        && let Some(anim) = obj.remove("animation")
    {
        let renamed = match anim {
            serde_json::Value::Object(inner) => {
                let mut out = serde_json::Map::with_capacity(inner.len());
                for (k, v) in inner {
                    let new_k = match k.as_str() {
                        "period_ms" => "periodMs".to_string(),
                        "play_count" => "playCount".to_string(),
                        _ => k,
                    };
                    out.insert(new_k, v);
                }
                serde_json::Value::Object(out)
            }
            other => other,
        };
        obj.insert("animation".to_string(), renamed);
    }
    value
}

// --- Validation for set_light_animation -------------------------------------

/// Validate and normalize an incoming `LightAnimation` against the spec's
/// error table. On success returns the animation with any non-unit direction
/// samples normalized and `phase` normalized into `[0.0, 1.0)`.
fn validate_and_normalize(
    mut anim: LightAnimation,
    target_is_dynamic: bool,
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
        if !target_is_dynamic {
            return Err(ScriptError::InvalidArgument {
                reason: "color animation is only permitted on dynamic lights; \
                         baked lights' SH indirect was computed at compile-time color"
                    .into(),
            });
        }
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
            // Silently normalize. Matches "unit-length invariant authoritatively
            // enforced here" from plan-2 §Sub-plan 6 error cases.
            *sample = Vec3Lit([x / len, y / len, z / len]);
        }
    }
    // Normalize phase into [0.0, 1.0) via rem_euclid, matching the GPU
    // evaluator. `None` and `Some(0.0)` both mean "start at the period head".
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

/// Apply a validated animation (or `None` to clear) to the entity's existing
/// `LightComponent`. Returns the error-mapping spec'd in plan-2 §Sub-plan 6.
fn apply_light_animation(
    ctx: &ScriptCtx,
    id: EntityId,
    animation: Option<LightAnimation>,
) -> Result<(), ScriptError> {
    // Read current component. Early-return the spec'd errors if entity is
    // missing or has no light component.
    let mut reg = ctx.registry.borrow_mut();
    let current = reg
        .get_component::<LightComponent>(id)
        .map_err(ScriptError::from)?
        .clone();

    let validated = match animation {
        Some(a) => Some(validate_and_normalize(a, current.is_dynamic)?),
        None => None,
    };

    let mut next = current;
    next.animation = validated;
    reg.set_component(id, next).map_err(ScriptError::from)?;
    Ok(())
}

// --- Primitive registration --------------------------------------------------

const SET_LIGHT_ANIM_DOC: &str = "Overwrite the LightComponent.animation on the given entity. Pass null/nil to clear. \
     Non-unit direction samples are silently normalized; zero-length direction samples \
     and color animations on non-dynamic lights error with InvalidArgument. \
     Behavior context only.";

#[allow(clippy::arc_with_non_send_sync)]
pub(crate) fn register_sp6_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_set_light_animation(registry, ctx);
}

fn register_set_light_animation(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("set_light_animation", {
            let ctx = ctx.clone();
            move |id: EntityId, animation: Option<LightAnimation>| -> Result<(), ScriptError> {
                apply_light_animation(&ctx, id, animation)
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc(SET_LIGHT_ANIM_DOC)
        .param("id", "EntityId")
        .param("animation", "LightAnimation | null")
        .finish();
}

/// Register the shared types referenced by SP6 primitive signatures into the
/// typedef generator. Complements `register_shared_types` in `primitives.rs`.
pub(crate) fn register_shared_types(registry: &mut PrimitiveRegistry) {
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
    // Field type spellings use Rust-style Option / Vec so the typedef
    // generator's `rust_to_ts` / `rust_to_luau` pass yields valid output
    // (`T | null` in TS, `T?` in Luau).
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
            "Per-sample color curve. Only valid on dynamic lights.",
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
        .field("castShadows", "bool", "")
        .field("isDynamic", "bool", "")
        .field("animation", "Option<LightAnimation>", "")
        .finish();
    registry
        .register_type("WorldQueryFilter")
        .field("component", "String", "Component name, e.g. \"light\".")
        .field(
            "tag",
            "Option<String>",
            "Optional tag filter (exact string match).",
        )
        .finish();
    registry
        .register_type("EntityTransform")
        .doc("Minimum transform shape guaranteed for all entity handles.")
        .field("position", "Vec3", "")
        .finish();
    registry
        .register_type("Entity")
        .doc("Generic entity handle returned by `world.query` when the component type is not known at compile time.")
        .field("id", "EntityId", "")
        .field("transform", "EntityTransform", "Entity position at query time.")
        .field("tag", "Option<String>", "The entity's tag at query time, if any.")
        .finish();
    registry
        .register_type("LightEntity")
        .doc("Entity handle returned by `world.query` when filtering for light entities.")
        .field("id", "EntityId", "")
        .field("transform", "EntityTransform", "Light origin at query time.")
        .field(
            "isDynamic",
            "bool",
            "Whether MapLight.is_dynamic was set on the source. Scripts use this to gate color animation.",
        )
        .field(
            "tag",
            "Option<String>",
            "The entity's tag at query time, if any.",
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
    use crate::scripting::components::light::{FalloffKind, LightKind};
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::registry::Transform;

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
                    cast_shadows: true,
                    is_dynamic,
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
        register_sp6_primitives(&mut r, ctx.clone());
        crate::scripting::primitives::register_world_query(&mut r, ctx);
        r
    }

    // --- world.query ---------------------------------------------------------

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
        // A second light without the tag must not appear.
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
                    cast_shadows: false,
                    is_dynamic: true,
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
    fn world_query_reachable_from_quickjs_returns_handle_array() {
        let (ctx, id) = test_ctx_with_light(true, Some("foo"));
        let r = registry_for(ctx);
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let script = r#"
                const hs = world_query({ component: "light", tag: "foo" });
                JSON.stringify(hs.map(h => ({
                    id: h.id,
                    x: h.transform.position.x,
                    tags: h.tags,
                    dyn: h.isDynamic,
                })))
            "#;
            let got: String = qjs.eval(script).unwrap();
            let expected = format!(r#"[{{"id":{},"x":1,"tags":["foo"],"dyn":true}}]"#, id.to_raw());
            assert_eq!(got, expected);
        });
    }

    #[test]
    fn world_query_reachable_from_luau_returns_handle_table() {
        let (ctx, _id) = test_ctx_with_light(true, None);
        let r = registry_for(ctx);
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let count: i64 = lua
            .load(
                r#"
                local hs = world_query({ component = "light" })
                return #hs
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(count, 1);
    }

    // --- world.query cross-runtime parity -----------------------------------
    //
    // The `set_light_animation_quickjs_and_luau_produce_identical_output` test
    // covers the mutate path; these cover the dispatch (read) path: shape of
    // the returned handles, error surface for unknown components, tag filter
    // behavior, and the `tag: <wrong type>` conversion error.

    #[test]
    fn world_query_light_component_returns_light_handles() {
        let (ctx, id) = test_ctx_with_light(true, Some("hallway_wave"));
        let r = registry_for(ctx);
        let raw = id.to_raw();

        // QuickJS: assert id, isDynamic, tags, transform.position.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = world_query({ component: "light", tag: "hallway_wave" });
                    JSON.stringify(hs.map(h => ({
                        id: h.id,
                        isDynamic: h.isDynamic,
                        tags: h.tags,
                        x: h.transform.position.x,
                        y: h.transform.position.y,
                        z: h.transform.position.z,
                    })))
                    "#,
                )
                .unwrap();
            let expected = format!(
                r#"[{{"id":{raw},"isDynamic":true,"tags":["hallway_wave"],"x":1,"y":2,"z":3}}]"#
            );
            assert_eq!(json, expected);
        });

        // Luau: assert the same fields via separate return values.
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (got_id, is_dynamic, first_tag, x, y, z): (i64, bool, String, f64, f64, f64) = lua
            .load(
                r#"
                local hs = world_query({ component = "light", tag = "hallway_wave" })
                local h = hs[1]
                return h.id, h.isDynamic, h.tags[1], h.transform.position.x,
                       h.transform.position.y, h.transform.position.z
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(got_id as u32, raw);
        assert!(is_dynamic);
        assert_eq!(first_tag, "hallway_wave");
        assert!((x - 1.0).abs() < 1e-5);
        assert!((y - 2.0).abs() < 1e-5);
        assert!((z - 3.0).abs() < 1e-5);
    }

    #[test]
    fn world_query_unknown_component_errors() {
        let (ctx, _id) = test_ctx_with_light(true, None);
        let r = registry_for(ctx);

        // QuickJS: catch and surface the message.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let msg: String = qjs
                .eval::<String, _>(
                    r#"try { world_query({ component: "decal" }); "no-throw" }
                       catch (e) { String(e.message || e) }"#,
                )
                .unwrap();
            assert!(
                msg.contains("invalid argument") && msg.contains("decal"),
                "expected InvalidArgument from QuickJS, got: {msg}"
            );
        });

        // Luau: pcall returns (ok, err); err stringifies to the same message.
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (ok, err): (bool, String) = lua
            .load(
                r#"
                local ok, err = pcall(function()
                    return world_query({ component = "decal" })
                end)
                return ok, tostring(err)
                "#,
            )
            .eval()
            .unwrap();
        assert!(!ok, "expected Luau call to error");
        assert!(
            err.contains("invalid argument") && err.contains("decal"),
            "expected InvalidArgument from Luau, got: {err}"
        );
    }

    #[test]
    fn world_query_tag_filter_excludes_unmatched() {
        // Two lights with distinct tags: tag filter must isolate one; querying
        // without a tag must yield both.
        let (ctx, first) = test_ctx_with_light(true, Some("alpha"));
        let second;
        {
            let mut reg = ctx.registry.borrow_mut();
            second = reg.spawn(Transform::default());
            reg.set_component(
                second,
                LightComponent {
                    origin: [9.0, 9.0, 9.0],
                    light_type: LightKind::Point,
                    intensity: 1.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: FalloffKind::InverseSquared,
                    falloff_range: 10.0,
                    cone_angle_inner: None,
                    cone_angle_outer: None,
                    cone_direction: None,
                    cast_shadows: false,
                    is_dynamic: true,
                    animation: None,
                },
            )
            .unwrap();
            reg.set_tags(second, vec!["beta".to_string()]).unwrap();
        }
        let r = registry_for(ctx);
        let first_raw = first.to_raw();

        // QuickJS: filtered → only "alpha"; unfiltered → both.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let filtered: String = qjs
                .eval(
                    r#"
                    const hs = world_query({ component: "light", tag: "alpha" });
                    JSON.stringify(hs.map(h => h.id))
                    "#,
                )
                .unwrap();
            assert_eq!(filtered, format!("[{first_raw}]"));
            let total: i32 = qjs
                .eval(r#"world_query({ component: "light" }).length"#)
                .unwrap();
            assert_eq!(total, 2);
        });

        // Luau: same checks.
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (filtered_count, filtered_id, total_count): (i64, i64, i64) = lua
            .load(
                r#"
                local hs = world_query({ component = "light", tag = "alpha" })
                local all = world_query({ component = "light" })
                return #hs, hs[1].id, #all
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(filtered_count, 1);
        assert_eq!(filtered_id as u32, first_raw);
        assert_eq!(total_count, 2);
    }

    #[test]
    fn world_query_tag_wrong_type_errors() {
        // Regression: numeric `tag` previously fell through `Option::ok()` and
        // returned all lights; now it must surface a conversion error so
        // typos / wrong-typed inputs are visible to script authors.
        let (ctx, _id) = test_ctx_with_light(true, Some("alpha"));
        let r = registry_for(ctx);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let result: String = qjs
                .eval::<String, _>(
                    r#"try { world_query({ component: "light", tag: 42 }); "no-throw" }
                       catch (e) { "threw" }"#,
                )
                .unwrap();
            assert_eq!(
                result, "threw",
                "QuickJS world_query with numeric tag must throw, not silently return all lights"
            );
        });

        // Luau: mlua coerces numbers to strings for `Option<String>`, so a
        // numeric `tag` becomes the string `"42"`. The contract being tested
        // is the same as the bug fix it covers: a wrong-typed tag must NOT
        // silently fall through to "no filter" and return every light. Either
        // erroring or treating the coerced string as a literal tag (which
        // matches no entity) is acceptable.
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let count: i64 = lua
            .load(
                r#"
                local ok, val = pcall(function()
                    return world_query({ component = "light", tag = 42 })
                end)
                if ok then
                    return #val
                else
                    return -1
                end
                "#,
            )
            .eval()
            .unwrap();
        assert_ne!(
            count, 1,
            "Luau world_query with numeric tag must not silently return the tagged light \
             as if no filter were applied"
        );
        // Defensive: the seeded fixture has exactly one light tagged \"alpha\".
        // A return of 1 here would mean the numeric tag was dropped to None
        // and all lights were returned (the original bug). 0 (no match) or
        // -1 (error) are both acceptable.
        assert!(count == 0 || count == -1, "got unexpected count: {count}");
    }

    // --- set_light_animation -------------------------------------------------

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

    #[test]
    fn set_light_animation_rejects_color_on_non_dynamic() {
        let (ctx, id) = test_ctx_with_light(false, None);
        let err = apply_light_animation(
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
        .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidArgument { .. }));
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

    // --- Context-scope enforcement ------------------------------------------

    #[test]
    fn both_primitives_are_behavior_only() {
        let ctx = ScriptCtx::new();
        let r = registry_for(ctx);
        let names_scopes: Vec<_> = r.iter().map(|p| (p.name, p.context_scope)).collect();
        for &(name, scope) in &names_scopes {
            assert_eq!(
                scope,
                ContextScope::BehaviorOnly,
                "primitive {name} must be BehaviorOnly"
            );
        }
    }

    #[test]
    fn world_query_stub_throws_wrong_context_in_definition_context() {
        let ctx = ScriptCtx::new();
        let r = registry_for(ctx);
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            for p in r.iter() {
                (p.quickjs_stub_installer)(&qjs).unwrap();
            }
            let msg: String = qjs
                .eval::<String, _>(
                    r#"try { world_query({component:"light"}); "no-throw" }
                       catch (e) { String(e.message || e) }"#,
                )
                .unwrap();
            assert!(msg.contains("not available"), "got: {msg}");
        });
    }

    #[test]
    fn set_light_animation_quickjs_and_luau_produce_identical_output() {
        // Cross-runtime parity: the Luau SDK is a port of the TypeScript SDK.
        // Running the same logical `set_light_animation` call through each
        // runtime must yield bitwise-identical `LightAnimation` values in the
        // registry. Exercises both the scalar (`brightness`) and `Vec3Lit`
        // (`color`) paths so a regression in either deserializer surfaces
        // here.
        let (ctx, id) = test_ctx_with_light(true, None);
        let r = registry_for(ctx.clone());

        // QuickJS run.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        let raw = id.to_raw();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let script = format!(
                r#"
                set_light_animation({raw}, {{
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

        // Reset and run the equivalent Luau script.
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
            set_light_animation({raw}, {{
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
