// Day-one primitives registered at engine startup.
// See: context/lib/scripting.md
//
// Every primitive captures `ScriptCtx` by `Rc` at registration time. To add a
// new subsystem to the scripting surface: add one field to `ScriptCtx` and a
// few `.register(...)` lines here.

use super::ctx::{ScriptCtx, ScriptEvent};
use super::error::ScriptError;
use super::primitives_registry::{ContextScope, PrimitiveRegistry};

/// Register the shared types referenced by day-one primitive signatures. These
/// feed the typedef generator (see: context/lib/scripting.md §7). No type-level
/// or field-level doc strings on day-one types — docs land per-plan as field
/// semantics are pinned down (e.g. plan 2 light types).
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
        .variant("Transform", "")
        .finish();
    registry
        .register_tagged_union("ComponentValue")
        .variant("Transform", "Transform", "")
        .finish();
    registry
        .register_type("ScriptEvent")
        .field("kind", "String", "")
        .field("payload", "Any", "")
        .finish();
}
use super::registry::{ComponentKind, ComponentValue, EntityId, Transform};

/// Register the seven day-one primitives. Called at engine startup, before
/// any script runtime is created.
pub(crate) fn register_all(registry: &mut PrimitiveRegistry, ctx: ScriptCtx) {
    register_shared_types(registry);

    // entity_exists --------------------------------------------------------
    registry
        .register("entity_exists", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<bool, ScriptError> {
                Ok(ctx.registry.borrow().exists(id))
            }
        })
        .scope(ContextScope::Both)
        .doc("Returns true if the entity id refers to a live entity.")
        .param("id", "EntityId")
        .finish();

    // spawn_entity ---------------------------------------------------------
    registry
        .register("spawn_entity", {
            let ctx = ctx.clone();
            move |transform: Transform| -> Result<EntityId, ScriptError> {
                ctx.registry
                    .borrow_mut()
                    .try_spawn(transform)
                    .ok_or_else(|| ScriptError::InvalidArgument {
                        reason: "entity slots exhausted".into(),
                    })
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Spawns a new entity with the given transform and returns its id.")
        .param("transform", "Transform")
        .finish();

    // despawn_entity -------------------------------------------------------
    registry
        .register("despawn_entity", {
            let ctx = ctx.clone();
            move |id: EntityId| -> Result<(), ScriptError> {
                ctx.registry.borrow_mut().despawn(id)?;
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Despawns a previously-spawned entity. Errors if the id is stale.")
        .param("id", "EntityId")
        .finish();

    // get_component --------------------------------------------------------
    registry
        .register("get_component", {
            let ctx = ctx.clone();
            move |id: EntityId, kind: ComponentKind| -> Result<ComponentValue, ScriptError> {
                let reg = ctx.registry.borrow();
                match kind {
                    ComponentKind::Transform => {
                        let t = reg.get_component::<Transform>(id)?;
                        Ok(ComponentValue::Transform(*t))
                    }
                }
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Reads a component of the given kind from an entity.")
        .param("id", "EntityId")
        .param("kind", "ComponentKind")
        .finish();

    // set_component --------------------------------------------------------
    registry
        .register("set_component", {
            let ctx = ctx.clone();
            move |id: EntityId,
                  kind: ComponentKind,
                  value: ComponentValue|
                  -> Result<(), ScriptError> {
                // Enforce the `kind` and `value` discriminants agree. Mismatches
                // are a script-side bug; we fail with a clear message rather
                // than silently using one of the two.
                match (kind, &value) {
                    (ComponentKind::Transform, ComponentValue::Transform(_)) => {}
                }
                let mut reg = ctx.registry.borrow_mut();
                match value {
                    ComponentValue::Transform(t) => reg.set_component(id, t)?,
                }
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Writes a component of the given kind onto an entity.")
        .param("id", "EntityId")
        .param("kind", "ComponentKind")
        .param("value", "ComponentValue")
        .finish();

    // emit_event (broadcast) ----------------------------------------------
    registry
        .register("emit_event", {
            let ctx = ctx.clone();
            move |event: ScriptEvent| -> Result<(), ScriptError> {
                ctx.events.borrow_mut().broadcast.push_back(event);
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Broadcasts an event to all listeners; drains at end of game logic.")
        .param("event", "ScriptEvent")
        .finish();

    // send_event (targeted) -----------------------------------------------
    registry
        .register("send_event", {
            let ctx = ctx.clone();
            move |target: EntityId, event: ScriptEvent| -> Result<(), ScriptError> {
                ctx.events.borrow_mut().targeted.push_back((target, event));
                Ok(())
            }
        })
        .scope(ContextScope::BehaviorOnly)
        .doc("Sends an event to a single entity; drains at end of game logic.")
        .param("target", "EntityId")
        .param("event", "ScriptEvent")
        .finish();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;

    fn registry_with_day_one() -> (PrimitiveRegistry, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ctx.clone());
        (r, ctx)
    }

    #[test]
    fn register_all_installs_seven_primitives() {
        let (r, _ctx) = registry_with_day_one();
        assert_eq!(r.len(), 7);
        let names: Vec<_> = r.iter().map(|p| p.name).collect();
        for expected in [
            "entity_exists",
            "spawn_entity",
            "despawn_entity",
            "get_component",
            "set_component",
            "emit_event",
            "send_event",
        ] {
            assert!(names.contains(&expected), "missing primitive {expected}");
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
            let got_live: bool = jsctx.eval(format!("entity_exists({raw})")).unwrap();
            assert!(got_live);

            let got_bogus: bool = jsctx
                .eval(format!("entity_exists({})", raw.wrapping_add(1)))
                .unwrap();
            // raw+1 changes the low-16 index bits — a different, unallocated slot.
            assert!(!got_bogus);
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
            .load(format!("return entity_exists({raw})"))
            .eval()
            .unwrap();
        assert!(got_live);
    }
}
