// `setFogParams` reaction primitive: combined partial-update path that
// applies any subset of `{density, glow, edgeSoftness, falloff, tint,
// saturation, minBrightness, lightRange}` to every fog volume matching
// the reaction's tag. density/glow/edgeSoftness/saturation/minBrightness
// clamp to 0.0 on invalid input; lightRange clamps to 0.001 on
// non-positive or non-finite input (matches `validate_pos_curve` in
// set_fog_animation.rs); falloff is dropped on invalid input (component
// preserved). Valid fields are applied in a single write per target.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, FogVolumeComponent};

use postretro_scripting_core::reaction_registry::ReactionError;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFogParamsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) density: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) glow: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) edge_softness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) falloff: Option<f32>,
    /// Scatter tint multiplier as `[r, g, b]` in linear 0-1 range.
    /// `[1, 1, 1]` = no tint. Each component clamped to `[0, +∞)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tint: Option<[f32; 3]>,
    /// Scatter saturation. 0 = greyscale, 1 = natural, >1 = boosted.
    /// Clamped to `[0, +∞)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) saturation: Option<f32>,
    /// Floor on per-volume glow brightness. Clamped to `[0, +∞)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) min_brightness: Option<f32>,
    /// Per-volume light range multiplier. Must be strictly positive;
    /// non-positive or non-finite inputs clamp to `0.001` (parity with the
    /// `light_range` curve channel in `setFogAnimation`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) light_range: Option<f32>,
}

/// Validated subset of fields, distilled from `SetFogParamsArgs` by the
/// dispatch entry point. Each `Option<_>` here represents a field that
/// passed validation and should be merged onto every target.
#[derive(Debug, Clone, Default, PartialEq)]
struct ValidatedFields {
    density: Option<f32>,
    glow: Option<f32>,
    edge_softness: Option<f32>,
    falloff: Option<f32>,
    tint: Option<[f32; 3]>,
    saturation: Option<f32>,
    min_brightness: Option<f32>,
    light_range: Option<f32>,
}

impl ValidatedFields {
    fn is_empty(&self) -> bool {
        self.density.is_none()
            && self.glow.is_none()
            && self.edge_softness.is_none()
            && self.falloff.is_none()
            && self.tint.is_none()
            && self.saturation.is_none()
            && self.min_brightness.is_none()
            && self.light_range.is_none()
    }

    fn apply_to(&self, comp: &mut FogVolumeComponent) {
        if let Some(d) = self.density {
            comp.density = d;
        }
        if let Some(s) = self.glow {
            comp.glow = s;
        }
        if let Some(e) = self.edge_softness {
            comp.edge_softness = e;
        }
        if let Some(f) = self.falloff {
            comp.falloff = f;
        }
        if let Some(t) = self.tint {
            comp.tint = t;
        }
        if let Some(s) = self.saturation {
            comp.saturation = s;
        }
        if let Some(m) = self.min_brightness {
            comp.min_brightness = m;
        }
        if let Some(l) = self.light_range {
            comp.light_range = l;
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

    if let Some(s) = args.glow {
        // NaN cannot be clamped to a meaningful value; treat it as 0.0.
        // Infinities are handled naturally by clamp (+inf → 1.0, -inf → 0.0).
        let clamped = if s.is_nan() {
            log::warn!("[Scripting] setFogParams: glow is NaN; clamping to 0.0");
            0.0
        } else {
            let c = s.clamp(0.0, 1.0);
            if !(0.0..=1.0).contains(&s) {
                log::warn!(
                    "[Scripting] setFogParams: glow {s} is outside [0.0, 1.0]; clamping to {c}"
                );
            }
            c
        };
        out.glow = Some(clamped);
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

    if let Some(t) = args.tint {
        let mut clamped = t;
        let mut warned = false;
        for (i, c) in clamped.iter_mut().enumerate() {
            if !c.is_finite() || *c < 0.0 {
                if !warned {
                    log::warn!(
                        "[Scripting] setFogParams: tint[{i}] {} is negative or non-finite; clamping to 0.0",
                        *c
                    );
                    warned = true;
                }
                *c = 0.0;
            }
        }
        out.tint = Some(clamped);
    }

    if let Some(s) = args.saturation {
        if s.is_finite() && s >= 0.0 {
            out.saturation = Some(s);
        } else {
            log::warn!(
                "[Scripting] setFogParams: saturation {s} is negative or non-finite; clamping to 0.0"
            );
            out.saturation = Some(0.0);
        }
    }

    if let Some(m) = args.min_brightness {
        if m.is_finite() && m >= 0.0 {
            out.min_brightness = Some(m);
        } else {
            log::warn!(
                "[Scripting] setFogParams: minBrightness {m} is negative or non-finite; clamping to 0.0"
            );
            out.min_brightness = Some(0.0);
        }
    }

    if let Some(l) = args.light_range {
        // `light_range = 0` would cause a divide-by-zero in the fog shader's
        // `clamp(1.0 - dist / (range * light_range), 0, 1)`. Clamp
        // non-positive or non-finite inputs up to a small positive minimum
        // (parity with `validate_pos_curve` in set_fog_animation.rs).
        if l.is_finite() && l > 0.0 {
            out.light_range = Some(l);
        } else {
            log::warn!(
                "[Scripting] setFogParams: lightRange {l} is non-positive or non-finite; clamping to 0.001"
            );
            out.light_range = Some(0.001);
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
        let mut next = match registry.get_component::<FogVolumeComponent>(id) {
            Ok(c) => c.clone(),
            Err(_) => {
                log::warn!(
                    "[Scripting] setFogParams: entity {id} has no FogVolumeComponent; skipping"
                );
                continue;
            }
        };
        fields.apply_to(&mut next);
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
            glow: 0.6,
            edge_softness: 0.25,
            falloff: 2.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            animation: None,
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
                edge_softness: Some(0.75),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 1.5);
        assert_eq!(after.glow, 0.6); // unchanged
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
                glow: Some(0.4),
                edge_softness: Some(0.1),
                falloff: Some(3.5),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 2.0);
        assert_eq!(after.glow, 0.4);
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
                falloff: Some(-1.0),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 1.0);
        assert_eq!(after.falloff, 2.0); // unchanged — invalid falloff dropped
    }

    #[test]
    fn out_of_range_density_and_glow_clamped() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                density: Some(-3.0),
                glow: Some(2.0),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 0.0);
        assert_eq!(after.glow, 1.0);
    }

    #[test]
    fn pos_infinity_glow_clamps_to_one() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                glow: Some(f32::INFINITY),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            1.0
        );
    }

    #[test]
    fn neg_infinity_glow_clamps_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                glow: Some(f32::NEG_INFINITY),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id).unwrap().glow,
            0.0
        );
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
                falloff: Some(f32::NAN),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(*after, sample_fog());
    }

    #[test]
    fn negative_tint_channel_clamped_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                tint: Some([0.5, -0.25, f32::INFINITY]),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.tint, [0.5, 0.0, 0.0]);
    }

    #[test]
    fn nan_saturation_clamped_to_zero() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                saturation: Some(f32::NAN),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.saturation, 0.0);
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
    fn set_fog_params_with_min_brightness_updates_field() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                min_brightness: Some(0.25),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.min_brightness, 0.25);
        // Sanity: other fields untouched.
        assert_eq!(after.light_range, 1.0);
        assert_eq!(after.density, 0.5);
    }

    #[test]
    fn set_fog_params_with_light_range_updates_field() {
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                light_range: Some(2.5),
                ..Default::default()
            },
        )
        .unwrap();
        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.light_range, 2.5);
        // Sanity: other fields untouched.
        assert_eq!(after.min_brightness, 0.0);
        assert_eq!(after.density, 0.5);
    }

    #[test]
    fn set_fog_params_clamps_nonpositive_light_range() {
        // `light_range = 0` would cause a divide-by-zero in the shader, so
        // the validator clamps non-positive (and non-finite) inputs up to a
        // small positive minimum (0.001) — parity with `validate_pos_curve`
        // in set_fog_animation.rs.
        let mut reg = EntityRegistry::new();
        let id = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id],
            &SetFogParamsArgs {
                light_range: Some(0.0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id)
                .unwrap()
                .light_range,
            0.001
        );

        let id2 = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id2],
            &SetFogParamsArgs {
                light_range: Some(-3.0),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id2)
                .unwrap()
                .light_range,
            0.001
        );

        let id3 = spawn_fog(&mut reg);
        dispatch(
            &mut reg,
            &[id3],
            &SetFogParamsArgs {
                light_range: Some(f32::NAN),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reg.get_component::<FogVolumeComponent>(id3)
                .unwrap()
                .light_range,
            0.001
        );
    }

    #[test]
    fn set_fog_params_args_deserialize_partial_camelcase_json_with_omitted_fields_as_none() {
        let v = serde_json::json!({ "edgeSoftness": 0.3, "falloff": 1.25 });
        let parsed: SetFogParamsArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.density, None);
        assert_eq!(parsed.glow, None);
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
                        glow: 0.4,
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
                    glow = 0.4,
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
        let after_a = reg_a
            .get_component::<FogVolumeComponent>(id_a)
            .unwrap()
            .clone();

        let mut reg_b = EntityRegistry::new();
        let id_b = spawn_fog(&mut reg_b);
        prim_reg
            .dispatch("setFogParams", &mut reg_b, &[id_b], &from_luau)
            .unwrap();
        let after_b = reg_b
            .get_component::<FogVolumeComponent>(id_b)
            .unwrap()
            .clone();

        assert_eq!(
            after_a, after_b,
            "QuickJS and Luau must produce identical FogVolumeComponent state \
             for the same setFogParams input"
        );
        // Sanity: the dispatch actually changed at least one field.
        assert_ne!(after_a, sample_fog());
    }
}
