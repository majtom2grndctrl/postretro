// `light_area` key translation: a brush entity's keys into a canonical area `MapLight`.
// See: context/lib/build_pipeline.md §Custom FGD, §Compiler pipeline (Area lights)

use std::collections::HashMap;

use glam::DVec3;
use postretro_level_format::emissive::EMISSIVE_VALUE_CEILING;

use super::{
    QUAKE_INTENSITY_REFERENCE, TranslateError, parse_color255, parse_keyframe_animation,
    parse_optional_int,
};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType, animated_light_placeholder};
use crate::map_format::MapFormat;

const AREA_LIGHT_CLASSNAME: &str = "light_area";

/// `_glow` when the key is absent: the emitting faces' visible glow strength.
pub(crate) const DEFAULT_AREA_GLOW: f32 = 4.0;

/// Keys that mean nothing on an area light but survive a classname change
/// from `light` or `light_spot` in the editor: each warns once, naming the
/// light and key, and is ignored.
const INERT_KEYS: [&str; 8] = [
    "delay",
    "_light_size",
    "_cone",
    "_cone2",
    "angle",
    "angles",
    "mangle",
    "target",
];

pub(crate) fn is_area_light_classname(classname: &str) -> bool {
    classname == AREA_LIGHT_CLASSNAME
}

/// Diagnostic name for an area light: entity index in file order and
/// classname. A brush entity has no authored origin; the pipeline appends the
/// emitter centroid once its polygons resolve.
pub(crate) fn area_light_source_label(entity_index: usize) -> String {
    format!("entity {entity_index} '{AREA_LIGHT_CLASSNAME}'")
}

/// The keys of a `light_area` that do not live on its `MapLight`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AreaLightKeys {
    /// `_falloff_range` in metres. `None` derives the reach from the emitter
    /// once its polygons resolve.
    pub reach_override: Option<f32>,
    /// `_glow`, fixed. Kept off `MapLight`, so it enters no lighting key.
    pub glow: f32,
}

/// Translates a `light_area` entity's keys. `light_ref` names the light in
/// every warning and error. The record's `origin` and `falloff_range` are
/// placeholders until its emitter resolves after partitioning; every field an
/// area light does not read is normalised, so an inert authored key can
/// never change the record or its bytes.
pub(crate) fn translate_area_light(
    props: &HashMap<String, String>,
    light_ref: &str,
) -> Result<(MapLight, AreaLightKeys), TranslateError> {
    refuse_contradictory_keys(props, light_ref)?;
    for key in INERT_KEYS {
        if authored(props, key).is_some() {
            log::warn!(
                "[Compiler] {light_ref}: `{key}` does not apply to an area light; it is ignored"
            );
        }
    }

    let raw_intensity = parse_optional_int(props, "light")?
        .or(parse_optional_int(props, "_light")?)
        .map(|v| v as f32)
        .unwrap_or(QUAKE_INTENSITY_REFERENCE);
    if raw_intensity == 0.0 {
        log::warn!("[Compiler] {light_ref}: `light` is 0; it will contribute nothing");
    }
    let intensity = raw_intensity / QUAKE_INTENSITY_REFERENCE;

    let color = match authored(props, "_color") {
        Some(raw) => parse_color255(raw).ok_or_else(|| TranslateError::InvalidProperty {
            key: "_color",
            value: raw.to_owned(),
            reason: "expected three integers 0-255",
        })?,
        None => [1.0, 1.0, 1.0],
    };

    let reach_override = match parse_optional_int(props, "_falloff_range")? {
        None => None,
        Some(units) if units > 0 => {
            Some(units as f32 * MapFormat::IdTech2.units_to_meters() as f32)
        }
        Some(units) => {
            return Err(TranslateError::InvalidProperty {
                key: "_falloff_range",
                value: units.to_string(),
                reason: "must be > 0",
            });
        }
    };

    let bake_only = match parse_optional_int(props, "_bake_only")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "_bake_only",
                value: other.to_string(),
                reason: "expected 0 (false) or 1 (true)",
            });
        }
    };
    let authored_animated = match parse_optional_int(props, "_animated")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "_animated",
                value: other.to_string(),
                reason: "expected 0 (off) or 1 (on)",
            });
        }
    };
    let start_active = super::authored_light_start_active(props)?;
    // An area light that starts dark must be lightable at runtime, so
    // `_start_inactive 1` takes the same animated reservation `_animated 1`
    // does. Point and spot lights keep their own rule.
    let is_animated = authored_animated || !start_active;

    let has_curve =
        authored(props, "brightness_curve").is_some() || authored(props, "color_curve").is_some();
    let animation = if has_curve {
        Some(parse_keyframe_animation(
            &curve_props(props),
            light_ref,
            0,
            bake_only,
            is_animated,
            start_active,
        )?)
    } else if is_animated {
        Some(animated_light_placeholder(start_active))
    } else {
        None
    };

    let tags = props
        .get("_tags")
        .map(|s| s.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();

    let light = MapLight {
        origin: DVec3::ZERO,
        light_type: LightType::Area,
        carrier: String::new(),
        intensity,
        color,
        falloff_model: FalloffModel::InverseSquared,
        falloff_range: 0.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation,
        bake_only,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated,
        tags,
        shadow_type: ShadowType::StaticLightMap,
        area: None,
    };
    let keys = AreaLightKeys {
        reach_override,
        glow: read_glow(props, light_ref)?,
    };
    Ok((light, keys))
}

/// `_shadow_type sdf`, `direction_curve` and a non-zero `style` ask for
/// behaviour an area light cannot give: each fails the build by name.
fn refuse_contradictory_keys(
    props: &HashMap<String, String>,
    light_ref: &str,
) -> Result<(), TranslateError> {
    let refuse = |key: &'static str, reason: &str| TranslateError::AreaLightKey {
        light_ref: light_ref.to_owned(),
        key,
        reason: reason.to_owned(),
    };
    match authored(props, "_shadow_type") {
        None | Some("static_light_map") => {}
        Some("sdf") => {
            return Err(refuse(
                "_shadow_type",
                "an area light's shadow bakes into the lightmap; `sdf` is not available",
            ));
        }
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "_shadow_type",
                value: other.to_owned(),
                reason: "expected static_light_map",
            });
        }
    }
    if authored(props, "direction_curve").is_some() {
        return Err(refuse(
            "direction_curve",
            "an area light emits along its faces' normals and has no aim to animate",
        ));
    }
    match parse_optional_int(props, "style")? {
        None | Some(0) => Ok(()),
        Some(_) => Err(refuse(
            "style",
            "legacy style presets are not available on an area light; use `brightness_curve`",
        )),
    }
}

/// The curve keys, blanks dropped, so a key the editor left empty after a
/// classname change reads as absent.
fn curve_props(props: &HashMap<String, String>) -> HashMap<String, String> {
    [
        "period_ms",
        "_curve_phase",
        "brightness_curve",
        "color_curve",
    ]
    .into_iter()
    .filter_map(|key| Some((key.to_owned(), authored(props, key)?.to_owned())))
    .collect()
}

/// `_glow`: default 4. Past the emissive ceiling it reads the ceiling and
/// warns; a negative value or a non-number fails the build.
fn read_glow(props: &HashMap<String, String>, light_ref: &str) -> Result<f32, TranslateError> {
    let Some(raw) = authored(props, "_glow") else {
        return Ok(DEFAULT_AREA_GLOW);
    };
    let refuse = |reason: &str| TranslateError::AreaLightKey {
        light_ref: light_ref.to_owned(),
        key: "_glow",
        reason: format!("`{raw}` {reason}"),
    };
    let value: f32 = raw
        .parse()
        .ok()
        .filter(|value: &f32| !value.is_nan())
        .ok_or_else(|| refuse("is not a number"))?;
    if value < 0.0 {
        return Err(refuse("is negative; glow must be 0 or more"));
    }
    if value > EMISSIVE_VALUE_CEILING {
        log::warn!(
            "[Compiler] {light_ref}: `_glow` {raw} is past the emissive ceiling; it reads \
             {EMISSIVE_VALUE_CEILING}"
        );
        return Ok(EMISSIVE_VALUE_CEILING);
    }
    // `-0` reads as 0.
    Ok(if value == 0.0 { 0.0 } else { value })
}

/// A key's trimmed value, or `None` when absent or blank: TrenchBroom writes
/// an empty string for a cleared field.
fn authored<'a>(props: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    props
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
