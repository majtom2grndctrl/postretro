//! Capture written presentation after successful reaction dispatch.
use crate::ctx::ScriptCtx;
use crate::registry::{ComponentKind, EntityId, EntityRegistry};
use postretro_entities::FireOrigin;
use postretro_entities::presentation_overrides::{PresentationComponent, PresentationOverride};

pub const REPLICATED_PRESENTATION_PRIMITIVES: [&str; 9] = [
    "setLightAnimation",
    "setFogDensity",
    "setFogGlow",
    "setFogEdgeSoftness",
    "setFogFalloff",
    "setFogParams",
    "setFogAnimation",
    "setEmitterRate",
    "setSpinRate",
];

fn kind(primitive: &str) -> Option<ComponentKind> {
    match primitive {
        "setLightAnimation" => Some(ComponentKind::Light),
        "setFogDensity" | "setFogGlow" | "setFogEdgeSoftness" | "setFogFalloff"
        | "setFogParams" | "setFogAnimation" => Some(ComponentKind::FogVolume),
        "setEmitterRate" | "setSpinRate" => Some(ComponentKind::BillboardEmitter),
        _ => None,
    }
}

pub(super) fn before_apply(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    primitive: &str,
    targets: &[EntityId],
) {
    let Some(kind) = kind(primitive) else {
        return;
    };
    let mut table = ctx.presentation_overrides.borrow_mut();
    for &id in targets {
        let key = (id, kind);
        if table.frame_start.contains_key(&key) {
            continue;
        }
        if let Some(component) = PresentationComponent::snapshot(registry, id, kind) {
            let anim_start = table.records.get(&key).and_then(|record| record.anim_start);
            table.frame_start.insert(
                key,
                PresentationOverride {
                    component,
                    anim_start,
                },
            );
        }
    }
}

pub(super) fn after_apply(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    primitive: &str,
    targets: &[EntityId],
) {
    let Some(kind) = kind(primitive) else {
        return;
    };
    let Some(validate) = ctx.presentation_override_validator.get() else {
        return;
    };
    // A host-only fire, or a host-only or host-mirrored `wait` landing on the
    // host: no client applies the write itself, so it replicates as an
    // override. The landing flag spans the landing's follow-ups, which run
    // after its fire context is restored.
    let host_only = ctx.landing.host_capture()
        || matches!(
            ctx.system_commands.fire_context().origin,
            FireOrigin::HostOnly(_)
        );
    let networked = ctx.presentation_networked_predicate.get();
    let mut table = ctx.presentation_overrides.borrow_mut();
    for &id in targets {
        if !host_only && !networked.is_some_and(|predicate| predicate(registry, id)) {
            continue;
        }
        let Some(component) = PresentationComponent::snapshot(registry, id, kind) else {
            continue;
        };
        let key = (id, kind);
        let anim_start = table.frame_start.get(&key).and_then(|baseline| {
            // LightBridge currently restarts on whole-component changes; the only
            // captured light writer changes animation alone. Keep future writers aligned.
            if component.same_animation(&baseline.component) {
                baseline.anim_start
            } else {
                Some(ctx.presentation_script_time.get())
            }
        });
        let record = PresentationOverride {
            component,
            anim_start,
        };
        if validate(&record) {
            table.records.insert(key, record);
        } else if table.warned_writes.insert((id, primitive.into())) {
            log::warn!(
                "[Net] presentation write `{primitive}` on {id:?} cannot be encoded; keeping its last valid override"
            );
        }
    }
}
