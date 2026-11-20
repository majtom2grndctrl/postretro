//! Capture written presentation after successful reaction dispatch.
use crate::ctx::ScriptCtx;
use crate::registry::{ComponentKind, EntityId, EntityRegistry};
use postretro_entities::FireOrigin;
use postretro_entities::presentation_overrides::{PresentationComponent, PresentationOverride};

pub const REPLICATED_PRESENTATION_PRIMITIVES: [&str; 12] = [
    "setLightAnimation",
    "setFogDensity",
    "setFogGlow",
    "setFogEdgeSoftness",
    "setFogFalloff",
    "setFogParams",
    "setFogAnimation",
    "setEmitterRate",
    "setSpinRate",
    "setEmissiveBrightness",
    "setEmissiveColor",
    "setEmissiveAnimation",
];

fn kind(primitive: &str) -> Option<ComponentKind> {
    match primitive {
        "setLightAnimation" => Some(ComponentKind::Light),
        "setFogDensity" | "setFogGlow" | "setFogEdgeSoftness" | "setFogFalloff"
        | "setFogParams" | "setFogAnimation" => Some(ComponentKind::FogVolume),
        "setEmitterRate" | "setSpinRate" => Some(ComponentKind::BillboardEmitter),
        "setEmissiveBrightness" | "setEmissiveColor" | "setEmissiveAnimation" => {
            Some(ComponentKind::EmissiveVolume)
        }
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
    let now = ctx.presentation_script_time.get();
    let mut table = ctx.presentation_overrides.borrow_mut();
    for &id in targets {
        let key = (id, kind);
        if table.frame_start.contains_key(&key) {
            continue;
        }
        if let Some(component) = PresentationComponent::snapshot(registry, id, kind) {
            // The stored start names the stored record's run. It is this
            // component's start only while that run is still the one
            // installed: an uncaptured (every-machine) install since then
            // began a run whose start no record holds, so a later capture
            // names none and peers keep their own clocks. That includes an
            // uncaptured re-install the frame bridge took as a restart of
            // the record's finished run.
            let anim_start = table
                .records
                .get(&key)
                .filter(|record| {
                    record.component.same_animation(&component)
                        && !restarts_finished_run(
                            &component,
                            &record.component,
                            record.anim_start,
                            now,
                        )
                })
                .and_then(|record| record.anim_start);
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

/// Whether `component` re-installed an emissive curve, since `baseline`, onto
/// a finite run that started at `start` and has finished by `now`. The frame
/// bridge restarts such a run at the script time it sees the install instead
/// of settling it (`research.md` P6): the drain runs before the bridge
/// settles a finite run, so on the frame its count runs out the run is still
/// installed and an identical re-install keeps its serial. Only the install
/// touch and the known start show the restart. With no known start, peers
/// already run the curve on their own clocks and nothing here can tell.
fn restarts_finished_run(
    component: &PresentationComponent,
    baseline: &PresentationComponent,
    start: Option<f64>,
    now: f64,
) -> bool {
    let (PresentationComponent::Emissive(after), PresentationComponent::Emissive(before)) =
        (component, baseline)
    else {
        return false;
    };
    let (Some(animation), Some(start)) = (after.animation.as_ref(), start) else {
        return false;
    };
    after.install_touch != before.install_touch
        && animation.has_finished(start * 1000.0, now * 1000.0)
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
    let now = ctx.presentation_script_time.get();
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
            // An emissive run also compares its install serial, so a curve a
            // brightness write cleared and the drain re-installed restarts (P30),
            // as does a curve re-installed onto its own finished run (P6).
            let start = baseline.anim_start;
            if component.same_animation(&baseline.component)
                && !restarts_finished_run(&component, &baseline.component, start, now)
            {
                baseline.anim_start
            } else {
                Some(now)
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
