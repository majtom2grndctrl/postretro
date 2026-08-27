// Entity/world scripting primitive handlers and registration.
// See: context/lib/scripting.md

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, ScriptCtx, ScriptError, Transform,
};
use postretro_lighting::script_primitives as light;
use postretro_scripting_core::primitive_adapters::{
    JsonValue, NullableString, WorldQueryFilterInput,
};
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};

pub(crate) fn register_entity_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("entityExists", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<bool, ScriptError> { entity_exists(&ctx, id) }
        })
        .scope(ContextScope::Both)
        .doc("Returns true if the entity id refers to a live entity.")
        .param("id", "EntityId")
        .finish();

    registry
        .register("getEntityProperty", {
            let ctx = ctx.clone();
            move |id: EntityId, key: String| -> Result<NullableString, ScriptError> {
                get_entity_property(&ctx, id, &key).map(NullableString)
            }
        })
        .scope(ContextScope::Both)
        .doc("Reads a per-placement KVP value authored on the source `.map` entity. Returns null when the key is absent or the entity has no KVP bag (e.g. runtime-spawned). Available in definition and data contexts.")
        .param("id", "EntityId")
        .param("key", "String")
        .finish();
}

pub(crate) fn entity_exists(ctx: &ScriptCtx, id: EntityId) -> Result<bool, ScriptError> {
    Ok(ctx.registry.borrow().exists(id))
}

pub(crate) fn get_entity_property(
    ctx: &ScriptCtx,
    id: EntityId,
    key: &str,
) -> Result<Option<String>, ScriptError> {
    let reg = ctx.registry.borrow();
    Ok(reg.get_map_kvp(id, key)?)
}

/// Parsed and validated form of the filter passed to `worldQuery`.
enum QueryFilter {
    Light {
        tag: Option<String>,
    },
    Transform {
        tag: Option<String>,
    },
    Emitter {
        tag: Option<String>,
    },
    FogVolume {
        tag: Option<String>,
    },
    KinematicMover {
        tag: Option<String>,
    },
    TriggerVolume {
        tag: Option<String>,
    },
    /// Always returns an empty array. Particles and sprite-visuals are
    /// engine-managed; scripts have no business iterating individual ones.
    AlwaysEmpty,
}

/// Parse the filter object passed to `worldQuery`. Unknown component names
/// surface as `InvalidArgument`.
fn parse_query_filter(component: &str, tag: Option<String>) -> Result<QueryFilter, ScriptError> {
    match component {
        "light" => Ok(QueryFilter::Light { tag }),
        "transform" => Ok(QueryFilter::Transform { tag }),
        "emitter" => Ok(QueryFilter::Emitter { tag }),
        "fog_volume" => Ok(QueryFilter::FogVolume { tag }),
        "kinematic_mover" => Ok(QueryFilter::KinematicMover { tag }),
        "trigger_volume" => Ok(QueryFilter::TriggerVolume { tag }),
        "particle" | "sprite_visual" => Ok(QueryFilter::AlwaysEmpty),
        other => Err(ScriptError::InvalidArgument {
            reason: format!(
                "worldQuery: unknown component `{other}`; supported: \
                 \"light\" | \"transform\" | \"emitter\" | \"fog_volume\" | \"kinematic_mover\" | \"trigger_volume\" | \"particle\" | \"sprite_visual\""
            ),
        }),
    }
}

const WORLD_QUERY_DOC: &str = "Return an array of raw entity snapshots matching the filter. Available in definition and data contexts. \
     Filter shape: { component: \"light\" | \"transform\" | \"emitter\" | \"fog_volume\" | \"kinematic_mover\" | \"trigger_volume\" | \"particle\" | \"sprite_visual\", tag?: string }. \
     `\"particle\"` and `\"sprite_visual\"` always return `[]` (engine-managed; scripts never iterate individual particles). \
     Unknown component values raise InvalidArgument. \
     The `world.ts` vocabulary module wraps these snapshots as `world.query` handles.";

const WORLD_GET_GRAVITY_DOC: &str = "Return the current world gravity in m/s² (negative = downward; positive = upward). \
     Seeded from the worldspawn `initialGravity` KVP at level load and persists until the next level load or a `worldSetGravity` call. \
     The `world.ts` vocabulary module wraps this as `world.getGravity`.";

const WORLD_SET_GRAVITY_DOC: &str = "Set the world gravity in m/s² (negative = downward; positive = upward). \
     NaN and non-finite values are silently ignored (a warning is logged) so a misbehaving script cannot wedge particle physics. \
     Effect is immediate and persists until the next level load or another `worldSetGravity` call. \
     The `world.ts` vocabulary module wraps this as `world.setGravity`.";

/// Collect transform handles as JSON. Every live entity carries `Transform`,
/// so this is effectively an entity query filtered only by tag.
fn collect_transform_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::Transform, tag) {
        let ComponentValue::Transform(t) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let mut obj = Map::with_capacity(3);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        let mut position = Map::with_capacity(3);
        position.insert("x".to_string(), Value::from(t.position.x as f64));
        position.insert("y".to_string(), Value::from(t.position.y as f64));
        position.insert("z".to_string(), Value::from(t.position.z as f64));
        obj.insert("position".to_string(), Value::Object(position));
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Collect billboard-emitter handles as JSON. `BillboardEmitterComponent` has
/// `#[serde(rename_all = "snake_case")]` so direct serialization gives the wire
/// field names without a manual mapping.
fn collect_emitter_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::BillboardEmitter, tag) {
        let ComponentValue::BillboardEmitter(e) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let comp = serde_json::to_value(e).expect("BillboardEmitterComponent always serializes");
        let mut obj = Map::with_capacity(4);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        obj.insert("component".to_string(), comp);
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Collect fog-volume handles as JSON. The component object is hand-rolled via
/// `camel_fields()` rather than serde so the script-facing camelCase keys don't
/// require a wire-affecting `#[serde(rename)]` on the struct.
fn collect_fog_volume_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};
    let reg = ctx.registry.borrow();
    let mut arr: Vec<Value> = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::FogVolume, tag) {
        let ComponentValue::FogVolume(f) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let comp = {
            let mut c = Map::with_capacity(7);
            for (key, value) in f.camel_fields() {
                c.insert(key.to_string(), Value::from(value as f64));
            }
            c.insert(
                "tint".to_string(),
                Value::Array(
                    f.tint
                        .iter()
                        .map(|x| Value::from(*x as f64))
                        .collect::<Vec<_>>(),
                ),
            );
            // `animation` crosses through serde so its camelCase wire shape
            // (periodMs, playCount) lands without manual mapping; absent
            // becomes JSON `null` (script-side `null` / Luau `nil`).
            let anim_json = match f.animation.as_ref() {
                Some(anim) => serde_json::to_value(anim).expect("FogAnimation always serializes"),
                None => Value::Null,
            };
            c.insert("animation".to_string(), anim_json);
            Value::Object(c)
        };
        let mut obj = Map::with_capacity(4);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        obj.insert("component".to_string(), comp);
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Collect kinematic-mover handles as JSON. Movers are queryable for their
/// position and tags, while their deterministic phase remains engine-owned.
fn collect_kinematic_mover_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};

    let reg = ctx.registry.borrow();
    let mut arr = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::KinematicMover, tag) {
        let ComponentValue::KinematicMover(_) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let mut obj = Map::with_capacity(3);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Collect trigger-volume handles as JSON. Triggers are queryable for their
/// placement identity only; arming and activation phase remain engine-owned.
fn collect_trigger_volume_handles_json(ctx: &ScriptCtx, tag: Option<&str>) -> serde_json::Value {
    use serde_json::{Map, Value};

    let reg = ctx.registry.borrow();
    let mut arr = Vec::new();
    for (id, value) in reg.query_by_component_and_tag(ComponentKind::TriggerVolume, tag) {
        let ComponentValue::TriggerVolume(_) = value else {
            continue;
        };
        let tags = reg.get_tags(id).unwrap_or(&[]).to_vec();
        let position = match reg.get_component::<Transform>(id) {
            Ok(t) => {
                let mut p = Map::with_capacity(3);
                p.insert("x".to_string(), Value::from(t.position.x as f64));
                p.insert("y".to_string(), Value::from(t.position.y as f64));
                p.insert("z".to_string(), Value::from(t.position.z as f64));
                Value::Object(p)
            }
            Err(_) => Value::Null,
        };
        let mut obj = Map::with_capacity(3);
        obj.insert("id".to_string(), Value::from(id.to_raw()));
        obj.insert("position".to_string(), position);
        obj.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        arr.push(Value::Object(obj));
    }
    Value::Array(arr)
}

/// Register the world-domain primitives: `worldQuery`, `worldGetGravity`, and
/// `worldSetGravity`. All three install in both definition and data contexts.
pub(crate) fn register_world_primitives(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_world_query(registry, ctx.clone());
    register_world_gravity(registry, ctx);
}

// Lives in world.rs because it dispatches across all component domains; per-domain helpers stay in their sibling primitive modules.
fn register_world_query(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    registry
        .register("worldQuery", {
            let ctx = ctx.clone();
            move |filter: WorldQueryFilterInput| -> Result<JsonValue, ScriptError> {
                let filter = parse_query_filter(&filter.component, filter.tag)?;
                match filter {
                    QueryFilter::Light { tag } => {
                        let handles = light::collect_light_handles(&ctx, tag.as_deref());
                        Ok(JsonValue(light::handles_to_json(handles)))
                    }
                    QueryFilter::Transform { tag } => Ok(JsonValue(
                        collect_transform_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::Emitter { tag } => Ok(JsonValue(collect_emitter_handles_json(
                        &ctx,
                        tag.as_deref(),
                    ))),
                    QueryFilter::FogVolume { tag } => Ok(JsonValue(
                        collect_fog_volume_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::KinematicMover { tag } => Ok(JsonValue(
                        collect_kinematic_mover_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::TriggerVolume { tag } => Ok(JsonValue(
                        collect_trigger_volume_handles_json(&ctx, tag.as_deref()),
                    )),
                    QueryFilter::AlwaysEmpty => Ok(JsonValue(serde_json::Value::Array(Vec::new()))),
                }
            }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_QUERY_DOC)
        .param("filter", "WorldQueryFilter")
        .finish();
}

fn register_world_gravity(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    // worldGetGravity ------------------------------------------------------
    registry
        .register("worldGetGravity", {
            let ctx = ctx.clone();
            move || -> Result<f32, ScriptError> { Ok(ctx.gravity.get()) }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_GET_GRAVITY_DOC)
        .finish();

    // worldSetGravity ------------------------------------------------------
    registry
        .register("worldSetGravity", {
            move |value: f32| -> Result<(), ScriptError> {
                if !value.is_finite() {
                    log::warn!("[Scripting] world.setGravity: rejected non-finite value");
                    return Ok(());
                }
                ctx.gravity.set(value);
                Ok(())
            }
        })
        .scope(ContextScope::Both)
        .doc(WORLD_SET_GRAVITY_DOC)
        .param("value", "f32")
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
    use postretro_entities::{
        KinematicMoverComponent, KinematicMoverMode, MoverCommand, NamedReaction,
        PrimitiveDescriptor, ReactionDescriptor, SequenceStep, TriggerActivation, TriggerFireMode,
        TriggerVolumeComponent,
    };
    use postretro_level_format::data_script::DataScriptSection;
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn registry_with_gravity() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_world_gravity(&mut r, ctx.clone());
        (r, ctx)
    }

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
                    follow_transform: false,
                    carrier: None,
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

    fn add_mover(ctx: &ScriptCtx, tag: Option<&str>) -> EntityId {
        let mut reg = ctx.registry.borrow_mut();
        let id = reg.spawn(Transform {
            position: Vec3::new(4.0, 5.0, 6.0),
            ..Transform::default()
        });
        reg.set_component(
            id,
            KinematicMoverComponent::new(
                9,
                postretro_entities::KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO, Vec3::X],
                    waypoint_names: vec!["start".to_string(), "end".to_string()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: KinematicMoverMode::PingPong,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            ),
        )
        .unwrap();
        if let Some(tag) = tag {
            reg.set_tags(id, vec![tag.to_string()]).unwrap();
        }
        id
    }

    fn add_trigger(ctx: &ScriptCtx, tag: Option<&str>) -> EntityId {
        let mut reg = ctx.registry.borrow_mut();
        let id = reg.spawn(Transform {
            position: Vec3::new(7.0, 8.0, 9.0),
            ..Transform::default()
        });
        reg.set_component(
            id,
            TriggerVolumeComponent::new(
                TriggerActivation::Touch,
                "lift".to_string(),
                "open_lift".to_string(),
                "close_lift".to_string(),
                MoverCommand::Start,
                TriggerFireMode::Multiple,
                0.0,
                false,
            ),
        )
        .unwrap();
        if let Some(tag) = tag {
            reg.set_tags(id, vec![tag.to_string()]).unwrap();
        }
        id
    }

    fn install_all(registry: &PrimitiveRegistry, qjs: &rquickjs::Ctx<'_>) {
        for p in registry.iter() {
            (p.quickjs_installer)(qjs).unwrap();
        }
    }

    fn install_all_lua(registry: &PrimitiveRegistry, lua: &mlua::Lua) {
        for p in registry.iter() {
            (p.luau_installer)(lua).unwrap();
        }
    }

    fn registry_for(ctx: ScriptCtx) -> PrimitiveRegistry {
        let mut r = PrimitiveRegistry::new();
        postretro_lighting::script_primitives::register_light_entity_primitives(
            &mut r,
            ctx.clone(),
        );
        register_world_primitives(&mut r, ctx);
        r
    }

    fn dev_script_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("content/dev/scripts")
            .join(name)
    }

    fn expected_trigger_fanout_reactions(trigger: EntityId) -> Vec<NamedReaction> {
        let by_tag = |name: &str, primitive: &str| NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: primitive.to_string(),
                target: None,
                tag: Some("fixture_tripwire".to_string()),
                on_complete: None,
                args: json!({}),
            }),
        };
        let by_id = |name: &str, primitive: &str| NamedReaction {
            name: format!("{name}.{}", trigger.to_raw()),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: trigger.into(),
                primitive: primitive.to_string(),
                args: json!({}),
            }]),
        };

        vec![
            by_tag("trigger.fixture.armByTag", "armTrigger"),
            by_tag("trigger.fixture.disarmByTag", "disarmTrigger"),
            by_id("trigger.fixture.armById", "armTrigger"),
            by_id("trigger.fixture.disarmById", "disarmTrigger"),
        ]
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
                const hs = worldQuery({ component: "light", tag: "foo" });
                JSON.stringify(hs.map(h => ({
                    id: h.id,
                    x: h.position.x,
                    tags: h.tags,
                    dyn: h.isDynamic,
                })))
            "#;
            let got: String = qjs.eval(script).unwrap();
            let expected = format!(
                r#"[{{"id":{},"x":1,"tags":["foo"],"dyn":true}}]"#,
                id.to_raw()
            );
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
                local hs = worldQuery({ component = "light" })
                return #hs
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn world_query_light_component_returns_light_handles() {
        let (ctx, id) = test_ctx_with_light(true, Some("hallway_wave"));
        let r = registry_for(ctx);
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "light", tag: "hallway_wave" });
                    JSON.stringify(hs.map(h => ({
                        id: h.id,
                        isDynamic: h.isDynamic,
                        tags: h.tags,
                        x: h.position.x,
                        y: h.position.y,
                        z: h.position.z,
                    })))
                    "#,
                )
                .unwrap();
            let expected = format!(
                r#"[{{"id":{raw},"isDynamic":true,"tags":["hallway_wave"],"x":1,"y":2,"z":3}}]"#
            );
            assert_eq!(json, expected);
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (got_id, is_dynamic, first_tag, x, y, z): (i64, bool, String, f64, f64, f64) = lua
            .load(
                r#"
                local hs = worldQuery({ component = "light", tag = "hallway_wave" })
                local h = hs[1]
                return h.id, h.isDynamic, h.tags[1], h.position.x,
                       h.position.y, h.position.z
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
    fn world_query_kinematic_mover_returns_tagged_mover_snapshots() {
        let (ctx, _) = test_ctx_with_light(true, Some("not_a_mover"));
        let id = add_mover(&ctx, Some("bridge-lift"));
        let r = registry_for(ctx);
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "kinematic_mover", tag: "bridge-lift" });
                    JSON.stringify(hs)
                    "#,
                )
                .unwrap();
            assert_eq!(
                json,
                format!(
                    r#"[{{"id":{raw},"position":{{"x":4,"y":5,"z":6}},"tags":["bridge-lift"]}}]"#
                )
            );
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (count, returned_id, tag): (i64, i64, String) = lua
            .load(
                r#"
                local hs = worldQuery({ component = "kinematic_mover", tag = "bridge-lift" })
                return #hs, hs[1].id, hs[1].tags[1]
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(returned_id as u32, raw);
        assert_eq!(tag, "bridge-lift");
    }

    #[test]
    fn world_query_trigger_volume_returns_identity_snapshot_without_runtime_state() {
        let (ctx, _) = test_ctx_with_light(true, Some("not_a_trigger"));
        let id = add_trigger(&ctx, Some("tripwire"));
        let r = registry_for(ctx);
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "trigger_volume", tag: "tripwire" });
                    JSON.stringify(hs)
                    "#,
                )
                .unwrap();
            assert_eq!(
                json,
                format!(r#"[{{"id":{raw},"position":{{"x":7,"y":8,"z":9}},"tags":["tripwire"]}}]"#)
            );
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (count, returned_id, tag, x, y, z): (i64, i64, String, f64, f64, f64) = lua
            .load(
                r#"
                local hs = worldQuery({ component = "trigger_volume", tag = "tripwire" })
                local h = hs[1]
                return #hs, h.id, h.tags[1], h.position.x, h.position.y, h.position.z
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(returned_id as u32, raw);
        assert_eq!(tag, "tripwire");
        assert_eq!((x, y, z), (7.0, 8.0, 9.0));
    }

    #[test]
    fn world_query_trigger_volume_sdk_handles_build_arm_and_disarm_steps_in_both_runtimes() {
        let (ctx, _) = test_ctx_with_light(true, Some("not_a_trigger"));
        let id = add_trigger(&ctx, Some("tripwire"));
        let r = registry_for(ctx);
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            postretro_scripting_core::quickjs::evaluate_prelude(&qjs).unwrap();
            let json: String = qjs
                .eval(
                    r#"
                    const h = world.query({ component: "trigger_volume", tag: "tripwire" })[0];
                    JSON.stringify({
                      id: h.id,
                      tags: h.tags,
                      arm: h.arm(),
                      disarm: h.disarm(),
                      exposesArmed: Object.hasOwn(h, "armed"),
                    })
                    "#,
                )
                .unwrap();
            assert_eq!(
                json,
                format!(
                    r#"{{"id":{raw},"tags":["tripwire"],"arm":[{{"id":{raw},"primitive":"armTrigger","args":{{}}}}],"disarm":[{{"id":{raw},"primitive":"disarmTrigger","args":{{}}}}],"exposesArmed":false}}"#
                )
            );
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        postretro_scripting_core::luau_prelude::evaluate_prelude(&lua, None).unwrap();
        let (returned_id, arm_primitive, disarm_primitive, exposes_armed, bridge_hidden): (
            i64,
            String,
            String,
            bool,
            bool,
        ) = lua
            .load(
                r#"
                local h = world:query({ component = "trigger_volume", tag = "tripwire" })[1]
                return h.id, h:arm()[1].primitive, h:disarm()[1].primitive,
                    h.armed ~= nil, wrapTriggerVolumeEntity == nil
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(returned_id as u32, raw);
        assert_eq!(arm_primitive, "armTrigger");
        assert_eq!(disarm_primitive, "disarmTrigger");
        assert!(!exposes_armed);
        assert!(bridge_hidden);
    }

    #[test]
    fn trigger_fanout_authoring_fixtures_produce_identical_closed_arm_disarm_reactions() {
        // Regression: the shipped fixtures must traverse the production TS bundler
        // and per-level VM paths, not merely remain unreferenced review examples.
        let ctx = ScriptCtx::new();
        let trigger = add_trigger(&ctx, Some("fixture_tripwire"));
        let primitives = registry_for(ctx.clone());
        let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx)
            .expect("fixture runtime constructs");
        let ts_fixture = dev_script_fixture("trigger-fanout-fixture.ts");
        let luau_fixture = dev_script_fixture("trigger-fanout-fixture.luau");
        let fixture_root = ts_fixture.parent().expect("fixture has a parent");

        // `bundle_entry` is the library implementation behind `scripts-build`.
        // The data runtime receives the resulting bytes exactly as a PRL section does.
        let ts_section = DataScriptSection {
            compiled_bytes: postretro_script_compiler::bundle_entry(&ts_fixture)
                .expect("TypeScript fixture bundles through scripts-build")
                .into_bytes(),
            source_path: ts_fixture.to_string_lossy().into_owned(),
        };
        let luau_section = DataScriptSection {
            compiled_bytes: std::fs::read(&luau_fixture).expect("Luau fixture reads"),
            source_path: luau_fixture.to_string_lossy().into_owned(),
        };

        let ts = runtime.run_data_script(&ts_section, fixture_root);
        let luau = runtime.run_data_script(&luau_section, fixture_root);
        let expected = expected_trigger_fanout_reactions(trigger);

        assert_eq!(
            ts.reactions, expected,
            "TS fixture queries the real trigger snapshot"
        );
        assert_eq!(
            luau.reactions, expected,
            "Luau fixture queries the real trigger snapshot"
        );
        assert_eq!(
            ts, luau,
            "both authoring runtimes register the same trigger control contract"
        );
    }

    #[test]
    fn trigger_event_presser_fixtures_produce_identical_wire_in_both_runtimes() {
        let ctx = ScriptCtx::new();
        let primitives = registry_for(ctx.clone());
        let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx)
            .expect("fixture runtime constructs");
        let ts_fixture = dev_script_fixture("trigger-event-presser-fixture.ts");
        let luau_fixture = dev_script_fixture("trigger-event-presser-fixture.luau");
        let fixture_root = ts_fixture.parent().expect("fixture has a parent");
        let ts_section = DataScriptSection {
            compiled_bytes: postretro_script_compiler::bundle_entry(&ts_fixture)
                .expect("TypeScript presser fixture bundles through scripts-build")
                .into_bytes(),
            source_path: ts_fixture.to_string_lossy().into_owned(),
        };
        let luau_section = DataScriptSection {
            compiled_bytes: std::fs::read(&luau_fixture).expect("Luau fixture reads"),
            source_path: luau_fixture.to_string_lossy().into_owned(),
        };

        let ts = runtime.run_data_script(&ts_section, fixture_root);
        let luau = runtime.run_data_script(&luau_section, fixture_root);

        assert_eq!(
            ts, luau,
            "TS and Luau must emit byte-equivalent descriptor data"
        );
        assert_eq!(ts.trigger_events.len(), 1);
        assert_eq!(ts.trigger_events[0].tag, "fixture_presser");
        assert_eq!(ts.trigger_events[0].event, "enter");
        assert_eq!(
            ts.trigger_events[0].fire,
            ["fixture.presser.damage", "fixture.presser.disarm"]
        );
    }

    #[test]
    fn world_query_handle_component_exposes_camel_case_keys() {
        // Regression: if `LightComponent`'s serde shape ever reverts to snake_case,
        // scripts silently see `undefined`/`nil` for `lightType`, `falloffModel`, etc.
        let (ctx, id) = test_ctx_with_light(true, Some("alpha"));
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut component = registry
                .get_component::<LightComponent>(id)
                .expect("fixture light exists")
                .clone();
            component.animated_slot = Some(5);
            registry
                .set_component(id, component)
                .expect("fixture light updates");
        }
        let r = registry_for(ctx);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "light", tag: "alpha" });
                    const c = hs[0].component;
                    JSON.stringify({
                        lightType: c.lightType,
                        falloffModel: c.falloffModel,
                        falloffRange: c.falloffRange,
                        isDynamic: c.isDynamic,
                        exposesAnimatedSlot: Object.hasOwn(c, "animatedSlot"),
                    })
                    "#,
                )
                .unwrap();
            assert_eq!(
                json,
                r#"{"lightType":"Point","falloffModel":"InverseSquared","falloffRange":10,"isDynamic":true,"exposesAnimatedSlot":false}"#
            );
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (light_type, falloff_model, falloff_range, is_dynamic, exposes_animated_slot): (
            String,
            String,
            f64,
            bool,
            bool,
        ) = lua
            .load(
                r#"
                local hs = worldQuery({ component = "light", tag = "alpha" })
                local c = hs[1].component
                return c.lightType, c.falloffModel, c.falloffRange, c.isDynamic,
                    c.animatedSlot ~= nil
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(light_type, "Point");
        assert_eq!(falloff_model, "InverseSquared");
        assert!((falloff_range - 10.0).abs() < 1e-5);
        assert!(is_dynamic);
        assert!(!exposes_animated_slot);
    }

    #[test]
    fn world_query_unknown_component_errors() {
        let (ctx, _id) = test_ctx_with_light(true, None);
        let r = registry_for(ctx);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let msg: String = qjs
                .eval::<String, _>(
                    r#"try { worldQuery({ component: "decal" }); "no-throw" }
                       catch (e) { String(e.message || e) }"#,
                )
                .unwrap();
            assert!(
                msg.contains("invalid argument") && msg.contains("decal"),
                "expected InvalidArgument from QuickJS, got: {msg}"
            );
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (ok, err): (bool, String) = lua
            .load(
                r#"
                local ok, err = pcall(function()
                    return worldQuery({ component = "decal" })
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
                    is_dynamic: true,
                    animated_slot: None,
                    follow_transform: false,
                    carrier: None,
                    animation: None,
                },
            )
            .unwrap();
            reg.set_tags(second, vec!["beta".to_string()]).unwrap();
        }
        let r = registry_for(ctx);
        let first_raw = first.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let filtered: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "light", tag: "alpha" });
                    JSON.stringify(hs.map(h => h.id))
                    "#,
                )
                .unwrap();
            assert_eq!(filtered, format!("[{first_raw}]"));
            let total: i32 = qjs
                .eval(r#"worldQuery({ component: "light" }).length"#)
                .unwrap();
            assert_eq!(total, 2);
        });

        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let (filtered_count, filtered_id, total_count): (i64, i64, i64) = lua
            .load(
                r#"
                local hs = worldQuery({ component = "light", tag = "alpha" })
                local all = worldQuery({ component = "light" })
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
    fn world_query_returns_both_tags_for_multi_tagged_entity() {
        // Regression: after `Option<String>` -> `Vec<String>` migration, a query
        // matching one tag must still surface all tags on the JS-facing handle.
        let (ctx, id) = test_ctx_with_light(true, None);
        {
            let mut reg = ctx.registry.borrow_mut();
            reg.set_tags(id, vec!["a".into(), "b".into()]).unwrap();
        }
        let r = registry_for(ctx);
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let json: String = qjs
                .eval(
                    r#"
                    const hs = worldQuery({ component: "light", tag: "a" });
                    JSON.stringify(hs.map(h => ({ id: h.id, tags: h.tags })))
                    "#,
                )
                .unwrap();
            assert!(
                json.contains(r#""tags":["a","b"]"#),
                "expected handle JSON to contain both tags, got: {json}"
            );
        });
    }

    #[test]
    fn world_query_tag_wrong_type_errors() {
        // Regression: numeric `tag` previously fell through `Option::ok()` and returned all lights.
        let (ctx, _id) = test_ctx_with_light(true, Some("alpha"));
        let r = registry_for(ctx);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|qjs| {
            install_all(&r, &qjs);
            let result: String = qjs
                .eval::<String, _>(
                    r#"try { worldQuery({ component: "light", tag: 42 }); "no-throw" }
                       catch (e) { "threw" }"#,
                )
                .unwrap();
            assert_eq!(
                result, "threw",
                "QuickJS world_query with numeric tag must throw, not silently return all lights"
            );
        });

        // Luau: mlua coerces numbers to strings, so tag=42 becomes "42". Either
        // erroring or matching no entity is acceptable; it must not return all lights.
        let lua = mlua::Lua::new();
        install_all_lua(&r, &lua);
        let count: i64 = lua
            .load(
                r#"
                local ok, val = pcall(function()
                    return worldQuery({ component = "light", tag = 42 })
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
        assert!(count == 0 || count == -1, "got unexpected count: {count}");
    }

    #[test]
    fn primitive_context_scopes() {
        let ctx = ScriptCtx::new();
        let r = registry_for(ctx);
        let scope_of = |name: &str| {
            r.iter()
                .find(|p| p.name == name)
                .map(|p| p.context_scope)
                .unwrap_or_else(|| panic!("primitive {name} not found"))
        };
        assert_eq!(scope_of("worldQuery"), ContextScope::Both);
        assert_eq!(scope_of("setLightAnimation"), ContextScope::DefinitionOnly);
    }

    #[test]
    fn get_gravity_reflects_seeded_value_from_quickjs() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-7.5);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: f64 = jsctx.eval("worldGetGravity()").unwrap();
            assert!((got - -7.5).abs() < 1e-5, "got {got}");
        });
    }

    #[test]
    fn set_gravity_updates_value_via_quickjs() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-9.81);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let _: () = jsctx.eval("worldSetGravity(3.5)").unwrap();
        });
        assert!((ctx.gravity.get() - 3.5).abs() < 1e-5);
    }

    #[test]
    fn set_gravity_ignores_nan_and_infinity() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-2.0);

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let _: () = jsctx.eval("worldSetGravity(NaN)").unwrap();
            let _: () = jsctx.eval("worldSetGravity(Infinity)").unwrap();
            let _: () = jsctx.eval("worldSetGravity(-Infinity)").unwrap();
        });
        assert_eq!(ctx.gravity.get(), -2.0);
    }

    #[test]
    fn get_gravity_callable_from_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-12.0);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got: f64 = lua.load("return worldGetGravity()").eval().unwrap();
        assert!((got - -12.0).abs() < 1e-5);
    }

    #[test]
    fn set_gravity_updates_value_via_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-9.81);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let _: () = lua.load("worldSetGravity(-5.0)").eval().unwrap();
        assert!((ctx.gravity.get() - -5.0).abs() < 1e-6);
    }

    #[test]
    fn set_gravity_ignores_nan_and_infinity_via_luau() {
        let (r, ctx) = registry_with_gravity();
        ctx.gravity.set(-2.0);

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let _: () = lua.load("worldSetGravity(math.huge)").eval().unwrap();
        let _: () = lua.load("worldSetGravity(-math.huge)").eval().unwrap();
        let _: () = lua.load("worldSetGravity(0/0)").eval().unwrap();
        assert!((ctx.gravity.get() - -2.0).abs() < 1e-6);
    }
}
