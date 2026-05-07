// `setFogParams` reaction primitive: combined partial-update path that
// applies any subset of `{density, scatter, edgeSoftness, falloff}` to every
// fog volume matching the reaction's tag. Each field is validated
// independently; invalid fields are dropped, valid fields applied; the
// component is mutated once per target with the merged result.
// See: context/lib/scripting.md §11 (Reaction primitives) and
// `context/plans/in-progress/fog-volume-reactions/index.md`.

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogParamsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scatter: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) edge_softness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) falloff: Option<f32>,
}

/// Validated subset of fields, distilled from `SetFogParamsArgs` by the
/// dispatch entry point. Each `Option<f32>` here represents a field that
/// passed validation and should be merged onto every target.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ValidatedFields {
    density: Option<f32>,
    scatter: Option<f32>,
    edge_softness: Option<f32>,
    falloff: Option<f32>,
}

impl ValidatedFields {
    fn is_empty(&self) -> bool {
        self.density.is_none()
            && self.scatter.is_none()
            && self.edge_softness.is_none()
            && self.falloff.is_none()
    }

    fn apply_to(&self, comp: &mut FogVolumeComponent) {
        if let Some(d) = self.density {
            comp.density = d;
        }
        if let Some(s) = self.scatter {
            comp.scatter = s;
        }
        if let Some(e) = self.edge_softness {
            comp.edge_softness = e;
        }
        if let Some(f) = self.falloff {
            comp.falloff = f;
        }
    }
}

fn validate(args: &SetFogParamsArgs) -> ValidatedFields {
    let mut out = ValidatedFields::default();

    if let Some(d) = args.density {
        if d.is_finite() && d >= 0.0 {
            out.density = Some(d);
        } else {
            log::warn!(
                "[Scripting] setFogParams: density {d} is negative or non-finite; clamping to 0.0"
            );
            out.density = Some(0.0);
        }
    }

    if let Some(s) = args.scatter {
        if s.is_finite() && (0.0..=1.0).contains(&s) {
            out.scatter = Some(s);
        } else if s.is_finite() {
            let clamped = s.clamp(0.0, 1.0);
            log::warn!(
                "[Scripting] setFogParams: scatter {s} is outside [0.0, 1.0]; clamping to {clamped}"
            );
            out.scatter = Some(clamped);
        } else {
            log::warn!("[Scripting] setFogParams: scatter {s} is non-finite; clamping to 0.0");
            out.scatter = Some(0.0);
        }
    }

    if let Some(e) = args.edge_softness {
        if e.is_finite() && e >= 0.0 {
            out.edge_softness = Some(e);
        } else {
            log::warn!(
                "[Scripting] setFogParams: edgeSoftness {e} is negative or non-finite; clamping to 0.0"
            );
            out.edge_softness = Some(0.0);
        }
    }

    if let Some(f) = args.falloff {
        if f.is_finite() && f > 0.0 {
            out.falloff = Some(f);
        } else {
            // Per validation table, falloff out-of-range is dropped (not
            // clamped) — the component's existing falloff is preserved.
            log::warn!(
                "[Scripting] setFogParams: falloff {f} is non-positive or non-finite; dropping field"
            );
        }
    }

    out
}

/// Apply the merged subset of `args` to every target's `FogVolumeComponent`.
///
/// Per-target behavior:
/// - Missing component → `log::warn!`, skip.
/// - All fields invalid (after validation) → no write for any target; no
///   dirty flag is set.
/// - Empty target set → no-op, debug log.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &SetFogParamsArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] setFogParams: empty target set, no-op");
        return Ok(());
    }

    let fields = validate(args);
    if fields.is_empty() {
        log::debug!("[Scripting] setFogParams: no valid fields after validation; no writes");
        return Ok(());
    }

    for &id in targets {
        let current = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => *c,
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogParams: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        let mut next = current;
        fields.apply_to(&mut next);
        if next == current {
            continue;
        }
        if let Err(e) = registry.set_component(id, next) {
            log::warn!("[Scripting] setFogParams: failed to write component on {id}: {e:?}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::Transform;

    fn sample_fog() -> FogVolumeComponent {
        FogVolumeComponent {
            density: 0.5,
            scatter: 0.6,
            edge_softness: 0.25,
            falloff: 2.0,
        }
    }

    fn spawn_fog(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(id, sample_fog()).unwrap();
        id
    }

    #[test]
    fn applies_partial_update_leaving_absent_fields_unchanged() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: Some(1.5),
                scatter: None,
                edge_softness: Some(0.75),
                falloff: None,
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 1.5);
        assert_eq!(after.scatter, 0.6); // unchanged
        assert_eq!(after.edge_softness, 0.75);
        assert_eq!(after.falloff, 2.0); // unchanged
    }

    #[test]
    fn applies_full_update() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: Some(2.0),
                scatter: Some(0.4),
                edge_softness: Some(0.1),
                falloff: Some(3.5),
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 2.0);
        assert_eq!(after.scatter, 0.4);
        assert_eq!(after.edge_softness, 0.1);
        assert_eq!(after.falloff, 3.5);
    }

    #[test]
    fn invalid_falloff_dropped_other_fields_applied() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: Some(1.0),
                scatter: None,
                edge_softness: None,
                falloff: Some(-1.0),
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 1.0);
        assert_eq!(after.falloff, 2.0); // unchanged — invalid falloff dropped
    }

    #[test]
    fn out_of_range_density_and_scatter_clamped() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: Some(-3.0),
                scatter: Some(2.0),
                edge_softness: None,
                falloff: None,
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 0.0);
        assert_eq!(after.scatter, 1.0);
    }

    #[test]
    fn all_invalid_results_in_no_write() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        // Only falloff is provided, and it's invalid → drops; no other fields.
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: None,
                scatter: None,
                edge_softness: None,
                falloff: Some(f32::NAN),
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(*after, sample_fog());
    }

    #[test]
    fn empty_args_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(&mut reg, &[id], &SetFogParamsArgs::default()).unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(*after, sample_fog());
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[],
            &SetFogParamsArgs {
                density: Some(9.0),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 0.5);
    }

    #[test]
    fn non_fog_target_is_skipped_with_warn() {
        let mut reg = EntityRegistry::new();
        let bare = reg.spawn(Transform::default());
        let fog = spawn_fog(&mut reg);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[bare, fog],
                &SetFogParamsArgs {
                    density: Some(1.5),
                    ..Default::default()
                },
            )
            .unwrap();
        });

        assert_eq!(
            reg.get_component::<FogVolumeComponent>(fog)
                .unwrap()
                .density,
            1.5
        );
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("no FogVolumeComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn args_deserialize_camelcase_partial() {
        let v = serde_json::json!({ "edgeSoftness": 0.3, "falloff": 1.25 });
        let parsed: SetFogParamsArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.density, None);
        assert_eq!(parsed.scatter, None);
        assert_eq!(parsed.edge_softness, Some(0.3));
        assert_eq!(parsed.falloff, Some(1.25));
    }

    /// Cross-runtime parity: same `setFogParams` args dispatched through the
    /// reaction primitive registry — once with the JSON value produced by
    /// QuickJS evaluating a JS object literal, once with the JSON value
    /// produced by Luau evaluating a Lua table — must mutate
    /// `FogVolumeComponent` to identical state.
    ///
    /// Mirrors the two input shapes used by
    /// `set_light_animation_quickjs_and_luau_produce_identical_output` in
    /// `crates/postretro/src/scripting/primitives/light.rs`: a JS object
    /// literal with camelCase keys, and a Luau table with the same keys.
    #[test]
    fn set_fog_params_quickjs_and_luau_produce_identical_output() {
        use crate::scripting::reactions::registry::{
            ReactionPrimitiveRegistry, register_fog_reaction_primitives,
        };

        // 1) Capture the JSON produced by QuickJS evaluating a JS object
        //    literal mirroring the shape an author would call setFogParams
        //    with.
        let rt = rquickjs::Runtime::new().unwrap();
        let jsctx = rquickjs::Context::full(&rt).unwrap();
        let from_quickjs: serde_json::Value = jsctx.with(|qjs| {
            let val: rquickjs::Value = qjs
                .eval(
                    r#"
                    ({
                        density: 1.25,
                        scatter: 0.4,
                        edgeSoftness: 0.5,
                        falloff: 3.0,
                    })
                    "#,
                )
                .unwrap();
            crate::scripting::conv::js_to_json(&qjs, val).unwrap()
        });

        // 2) Capture the JSON produced by Luau evaluating an equivalent table.
        let lua = mlua::Lua::new();
        let lua_val: mlua::Value = lua
            .load(
                r#"
                return {
                    density = 1.25,
                    scatter = 0.4,
                    edgeSoftness = 0.5,
                    falloff = 3.0,
                }
                "#,
            )
            .eval()
            .unwrap();
        let from_luau = crate::scripting::conv::lua_to_json(lua_val).unwrap();

        // 3) Dispatch each through the reaction registry against a freshly
        //    spawned fog volume; assert the resulting components match.
        let mut prim_reg = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut prim_reg);

        let mut reg_a = EntityRegistry::new();
        let id_a = spawn_fog(&mut reg_a);
        prim_reg
            .dispatch("setFogParams", &mut reg_a, &[id_a], &from_quickjs)
            .unwrap();
        let after_a = *reg_a.get_component::<FogVolumeComponent>(id_a).unwrap();

        let mut reg_b = EntityRegistry::new();
        let id_b = spawn_fog(&mut reg_b);
        prim_reg
            .dispatch("setFogParams", &mut reg_b, &[id_b], &from_luau)
            .unwrap();
        let after_b = *reg_b.get_component::<FogVolumeComponent>(id_b).unwrap();

        assert_eq!(
            after_a, after_b,
            "QuickJS and Luau must produce identical FogVolumeComponent state \
             for the same setFogParams input"
        );
        // Sanity: the dispatch actually changed at least one field.
        assert_ne!(after_a, sample_fog());
    }
}
