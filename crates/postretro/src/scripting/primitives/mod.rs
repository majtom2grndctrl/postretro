// Scripting primitives composition root.
// See: context/lib/scripting.md
//
// Per-domain primitive registration lives in sibling modules (`entity`,
// `light`, `world`); this file owns shared types and the `register_all`
// entry point that the engine and tests converge on.

pub(crate) mod entity;
pub(crate) mod light;
pub(crate) mod world;

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives_registry::PrimitiveRegistry;

/// Register the shared types referenced by day-one primitive signatures. These
/// feed the typedef generator (see: context/lib/scripting.md §7).
pub(crate) fn register_shared_types(registry: &mut PrimitiveRegistry) {
    registry.register_type("EntityId").brand("number").finish();
    registry
        .register_type("Vec3")
        .field("x", "f32", "")
        .field("y", "f32", "")
        .field("z", "f32", "")
        .finish();
    registry
        .register_type("EulerDegrees")
        .field("pitch", "f32", "")
        .field("yaw", "f32", "")
        .field("roll", "f32", "")
        .finish();
    registry
        .register_type("Transform")
        .field("position", "Vec3", "")
        .field("rotation", "EulerDegrees", "")
        .field("scale", "Vec3", "")
        .finish();
    registry
        .register_enum("ComponentKind")
        .variant("transform", "")
        .variant("light", "")
        .variant("billboard_emitter", "")
        .variant("particle_state", "")
        .variant("sprite_visual", "")
        .variant("fog_volume", "")
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .flat()
        .variant("transform", "Transform", "")
        .variant("light", "LightComponent", "")
        .variant("billboard_emitter", "BillboardEmitterComponent", "")
        .variant("particle_state", "ParticleState", "")
        .variant("sprite_visual", "SpriteVisual", "")
        .variant("fog_volume", "FogVolumeComponent", "")
        .finish();
    registry
        .register_type("LightDescriptor")
        .doc("Authored light component preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case across the FFI.")
        .field("color", "Vec3", "RGB color in [0, 1].")
        .field("intensity", "f32", "Static intensity scalar.")
        .field(
            "range",
            "f32",
            "Falloff range (maps onto LightComponent.falloffRange at spawn).",
        )
        .field(
            "is_dynamic",
            "bool",
            "Author hint; descriptor-spawned lights are always treated as dynamic at spawn (baked indirect not supported).",
        )
        .finish();
    registry
        .register_type("EntityTypeDescriptor")
        .doc("Argument shape for `registerEntity`. `components` is an optional sub-object carrying typed component presets.")
        .field("classname", "String", "FGD classname this descriptor binds to.")
        .field(
            "components?",
            "EntityTypeComponents",
            "Optional component presets attached at level-load spawn.",
        )
        .finish();
    registry
        .register_type("BillboardEmitterComponent")
        .doc("Engine-managed billboard emitter component shape. Carried by `BillboardEmitter` ECS entities and produced by SDK `emitter()`/`smokeEmitter()`/etc.")
        .field("rate", "f32", "Continuous spawn rate (particles/sec). 0 = inactive.")
        .field("burst", "Option<u32>", "")
        .field("spread", "f32", "")
        .field("lifetime", "f32", "")
        .field("velocity", "Vec3", "")
        .field("buoyancy", "f32", "")
        .field("drag", "f32", "")
        .field("size_over_lifetime", "Vec<f32>", "")
        .field("opacity_over_lifetime", "Vec<f32>", "")
        .field("color", "Vec3", "")
        .field("sprite", "String", "")
        .field("spin_rate", "f32", "")
        .field("spin_animation", "Option<SpinAnimation>", "")
        .finish();
    registry
        .register_type("SpinAnimation")
        .doc("Spin tween shape consumed by `setSpinRate`.")
        .field("duration", "f32", "")
        .field("rate_curve", "Vec<f32>", "")
        .finish();
    registry
        .register_type("FogAnimation")
        .doc("Animation curves attached to a fog volume by the `setFogAnimation` reaction primitive. Four independent channels share `periodMs` / `phase` / `playCount`: `density` modulates volumetric density, `saturation` modulates SH-irradiance saturation, `minBrightness` modulates the scatter brightness floor, and `lightRange` scales how far lights reach inside the fog. At least one curve must be present when `playCount` is finite — otherwise the animation has nothing to settle to. `phase` is normalized into `[0, 1)`. `playCount = null` loops forever; finite counts have the bridge write back each channel's final keyframe as static state on completion. There is no `startActive` flag — fog has no GPU descriptor for the curve, so absence (`null`) is the only inactive state.")
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
            "density",
            "Option<Vec<f32>>",
            "Per-sample density curve. null leaves the static density unchanged.",
        )
        .field(
            "saturation",
            "Option<Vec<f32>>",
            "Per-sample saturation curve. null leaves the static saturation unchanged.",
        )
        .field(
            "minBrightness",
            "Option<Vec<f32>>",
            "Per-sample animation curve for the `min_brightness` channel (scatter brightness floor). null leaves the static min_brightness unchanged. Each sample clamped to `[0, +∞)`; empty curve is rejected.",
        )
        .field(
            "lightRange",
            "Option<Vec<f32>>",
            "Per-sample animation curve for the `light_range` channel (scales how far lights reach inside this fog). null leaves the static light_range unchanged. Each sample must be strictly positive and finite; non-positive or non-finite samples clamp to `0.001`; empty curve is rejected.",
        )
        .finish();
    registry
        .register_type("FogVolumeComponent")
        .doc("Script-facing fog-volume component shape. Carried by `FogVolume` ECS entities; the AABB is baked at level load and lives in the FogVolumeBridge side-table — it is not exposed here because it is not runtime-settable.")
        .field("density", "f32", "Volumetric fog density inside the AABB.")
        .field("glow", "f32", "How much the fog lights up near light sources. 0 = stays dark even under bright lights, 1 = picks up full light color. Raise for misty glow, lower for thick opaque smoke.")
        .field("edgeSoftness", "f32", "Edge softness in world units: 0 = hard cutoff at the brush face, larger = wider linear ramp inward from each face.")
        .field("falloff", "f32", "Radial falloff exponent. Consulted by the radial (`fog_lamp`, `fog_tube`) and ellipsoid (axis-aligned `fog_volume`) shader paths; stored but ignored by the plane-sweep (non-axis-aligned `fog_volume`) path.")
        .field("tint", "[f32; 3]", "Per-volume RGB scatter multiplier. Default `[1.0, 1.0, 1.0]`.")
        .field("saturation", "f32", "Saturation of transmitted SH irradiance: 0 = greyscale, 1 = natural, >1 = boosted. Default 1.0.")
        .field("minBrightness", "f32", "Floor on per-volume scatter brightness. Clamped to `[0, +∞)`. Default 0.0.")
        .field("lightRange", "f32", "Scales how far lights reach inside this fog. 1.0 = same range as open air, 2.0 = double range, 0.5 = half range. Strictly positive; clamps to 0.001. Default 1.0.")
        .field("animation", "Option<FogAnimation>", "Optional animation carrying any combination of density, saturation, minBrightness, and lightRange curves. null holds the static state.")
        .finish();
    registry
        .register_type("FogVolumeEntity")
        .doc("Entity handle returned by `world.query` when filtering for fog-volume entities.")
        .field("id", "EntityId", "")
        .field(
            "position",
            "Vec3",
            "Volume center at query time (AABB midpoint, baked at level load).",
        )
        .field(
            "tags",
            "Vec<String>",
            "The entity's tags at query time. Empty array if untagged.",
        )
        .field(
            "component",
            "FogVolumeComponent",
            "Full fog-volume component snapshot at query time.",
        )
        .finish();
    registry
        .register_type("EntityTypeComponents")
        .doc("Optional bag of component presets carried by `EntityTypeDescriptor.components`.")
        .field("light?", "Option<LightDescriptor>", "")
        .field("emitter?", "Option<BillboardEmitterComponent>", "")
        .field("movement?", "Option<PlayerMovementDescriptor>", "")
        .finish();
    registry
        .register_type("PlayerMovementDescriptor")
        .doc("Authored player-movement component preset. All four sub-objects are required when `movement` is present; the data-archetype spawn path materializes the runtime movement component from this.")
        .field("capsule", "CapsuleParams", "Collision capsule shape.")
        .field("ground", "GroundParams", "On-ground locomotion parameters.")
        .field("air", "AirParams", "Mid-air control parameters.")
        .field("fall", "FallParams", "Falling parameters.")
        .finish();
    registry
        .register_type("CapsuleParams")
        .doc("Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`.")
        .field("radius", "f32", "Capsule radius in world units. Must be > 0.")
        .field("halfHeight", "f32", "Cylinder half-height in world units. Must be > 0.")
        .finish();
    registry
        .register_type("GroundParams")
        .doc("On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization.")
        .field("speed", "f32", "Target ground speed in world units/sec.")
        .field("accel", "f32", "Ground acceleration in world units/sec².")
        .field("jumpVelocity", "f32", "Vertical launch velocity applied on jump.")
        .field("stepHeight", "f32", "Maximum step-up height in world units.")
        .field("maxSlope", "f32", "Maximum walkable slope in degrees; must lie in [0, 90].")
        .finish();
    registry
        .register_type("AirParams")
        .doc("Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`.")
        .field("forwardSteer", "f32", "Forward steering authority in [0, 1].")
        .field("accel", "f32", "Air acceleration in world units/sec².")
        .field("maxControlSpeed", "f32", "Speed cap that air-accel can push toward.")
        .field("bunnyHop", "bool", "Permit chained jumps on landing without releasing the jump input.")
        .field("jumps", "u32", "Additional jumps allowed in air after the initial ground jump. 0 disables air jumps.")
        .field("jumpCeiling", "f32", "Maximum upward velocity an air jump can reach; required when `jumps > 0`.")
        .finish();
    registry
        .register_type("FallParams")
        .doc("Falling parameters.")
        .field("terminalVelocity", "f32", "Terminal downward fall speed in world units/sec. Must be > 0.")
        .finish();
    registry
        .register_type("ModManifest")
        .doc("Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine.")
        .field("name", "String", "Human-readable mod name. Required.")
        .finish();
}

/// Register all engine primitives and shared types. Called at engine startup,
/// before any script runtime is created.
pub(crate) fn register_all(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_shared_types(registry);
    light::register_shared_types(registry);
    light::register_light_entity_primitives(registry, ctx.clone());
    world::register_world_primitives(registry, ctx.clone());
    entity::register_entity_primitives(registry, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::registry::Transform;

    fn registry_with_day_one() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ctx.clone());
        (r, ctx)
    }

    #[test]
    fn register_all_installs_expected_primitives() {
        let (r, _ctx) = registry_with_day_one();
        let names: Vec<_> = r.iter().map(|p| p.name).collect();
        for expected in [
            "entityExists",
            "worldQuery",
            "worldGetGravity",
            "worldSetGravity",
            "setLightAnimation",
            "getEntityProperty",
            "registerEntity",
        ] {
            assert!(names.contains(&expected), "missing primitive {expected}");
        }
        // The Live VM primitives are gone — they must NOT appear.
        for forbidden in [
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
            "registerHandler",
        ] {
            assert!(
                !names.contains(&forbidden),
                "primitive {forbidden} must be removed",
            );
        }
    }

    #[test]
    fn entity_exists_callable_from_quickjs_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        // Seed a live entity from Rust so we have a known-valid id.
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got_live: bool = jsctx.eval(format!("entityExists({raw})")).unwrap();
            assert!(got_live);

            let got_bogus: bool = jsctx
                .eval(format!("entityExists({})", raw.wrapping_add(1)))
                .unwrap();
            // raw+1 changes the low-16 index bits — a different, unallocated slot.
            assert!(!got_bogus);
        });
    }

    #[test]
    fn get_entity_property_returns_value_from_quickjs_when_set() {
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let mut kv = HashMap::new();
        kv.insert("wave".to_string(), "3".to_string());
        ctx.registry.borrow_mut().set_map_kvps(id, kv).unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: String = jsctx
                .eval(format!("getEntityProperty({raw}, 'wave')"))
                .unwrap();
            assert_eq!(got, "3");
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_unknown_key() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            // Entity exists but has no KVP bag — script sees `null`.
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'missing') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn get_entity_property_returns_null_for_entity_with_empty_kvp_bag() {
        // An entity spawned from a map placement but with an empty KVP map
        // writes no entry to the KVP side-table. `getEntityProperty` must
        // return null (not an error) for any key on such an entity — the
        // code path differs from "key absent from a non-empty bag".
        use std::collections::HashMap;
        let (r, ctx) = registry_with_day_one();

        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        ctx.registry
            .borrow_mut()
            .set_map_kvps(id, HashMap::new())
            .unwrap();
        let raw = id.to_raw();

        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        jsctx.with(|jsctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&jsctx).unwrap();
            }
            let got: bool = jsctx
                .eval(format!("getEntityProperty({raw}, 'anyKey') === null"))
                .unwrap();
            assert!(got);
        });
    }

    #[test]
    fn entity_exists_callable_from_luau_and_matches_registry() {
        let (r, ctx) = registry_with_day_one();
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        let raw = id.to_raw();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got_live: bool = lua
            .load(format!("return entityExists({raw})"))
            .eval()
            .unwrap();
        assert!(got_live);
    }
}
