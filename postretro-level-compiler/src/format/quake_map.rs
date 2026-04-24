// Quake-family .map FGD light translation.
// Converts a property bag (key-value pairs from shambler) plus origin and
// classname into the canonical `MapLight`. Owns the Quake `style` preset
// table and all degrees-to-radians / Quake-units-to-meters conversions at
// the translation boundary.
// See: context/plans/in-progress/lighting-foundation/1-fgd-canonical.md
// See: context/lib/build_pipeline.md §Custom FGD

use std::collections::HashMap;

use glam::DVec3;
use thiserror::Error;

use crate::map_data::{
    FalloffModel, KEYFRAME_RESAMPLE_RATE_HZ, LightAnimation, LightType, MapLight,
    resample_keyframes,
};
use crate::map_format::MapFormat;

/// Quake-family light classnames recognised by the translator.
pub const LIGHT_CLASSNAMES: &[&str] = &["light", "light_spot", "light_sun"];

/// Quake authoring reference for the `light` property. A mapper-authored
/// `light 300` (the Quake default and the "fully lit room" baseline)
/// translates to `MapLight.intensity = 1.0` after division by this
/// constant. Tunable if the retro aesthetic wants a different center, but
/// 300 matches the documented Quake `light.c` default and keeps existing
/// map values behaving as mappers expect.
const QUAKE_INTENSITY_REFERENCE: f32 = 300.0;

/// Returns true if the classname names a Quake-family light entity.
pub fn is_light_classname(classname: &str) -> bool {
    LIGHT_CLASSNAMES.contains(&classname)
}

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("unknown light classname: {0}")]
    UnknownClassname(String),

    #[error("light entity missing required property '{0}'")]
    MissingProperty(&'static str),

    #[error("light entity property '{key}' has invalid value '{value}': {reason}")]
    InvalidProperty {
        key: &'static str,
        value: String,
        reason: &'static str,
    },

    #[error(
        "light_spot has 'target' set but named-entity targeting is not supported until Milestone 6; use 'mangle' for spotlight direction"
    )]
    TargetNotSupported,

    /// Malformed keyframe in a `*_curve` FGD key. `light_ref` names the light
    /// (classname + origin) so authors can locate it in TrenchBroom.
    #[error("light {light_ref}: '{key}' — {reason}")]
    InvalidKeyframeCurve {
        key: &'static str,
        light_ref: String,
        reason: String,
    },

    /// `color_curve` authored on a light that is neither `_bake_only` nor
    /// `_dynamic`. Animated direct color on a baked light would drift from the
    /// SH indirect bake (Plan 2 Sub-plan 1 rule).
    #[error(
        "light {light_ref}: 'color_curve' — color animation is only valid on `_bake_only` or `_dynamic` lights. Either mark the light `_dynamic 1`, set `_bake_only 1`, or remove `color_curve`."
    )]
    ColorCurveOnBakedLight { light_ref: String },
}

/// Translate a Quake-family light entity into a canonical `MapLight`.
///
/// `props` is the raw property bag extracted by the parser. `origin` is the
/// already-converted engine-space position (meters, Y-up). `classname` selects
/// the light shape.
///
/// Validation errors block compilation; warnings log via `log::warn!` and
/// proceed with defaults. See §Validation rules in sub-plan 1.
pub fn translate_light(
    props: &HashMap<String, String>,
    origin: DVec3,
    classname: &str,
) -> Result<MapLight, TranslateError> {
    let light_type = match classname {
        "light" => LightType::Point,
        "light_spot" => LightType::Spot,
        "light_sun" => LightType::Directional,
        other => return Err(TranslateError::UnknownClassname(other.to_string())),
    };

    // -- Intensity (accept both "light" and "_light") --
    //
    // Quake authoring convention is a 0–300 "radiosity energy" scalar with
    // 300 as the default "fully lit room" value. The canonical `MapLight`
    // format is a modern 0–1+ linear multiplier on `color`, so we divide by
    // `QUAKE_INTENSITY_REFERENCE` at the translation boundary. A mapper-
    // authored `light 300` lands at `intensity 1.0` and multiplies its color
    // at full strength; `light 180` lands at `0.6`, and so on. Consumers
    // (direct light shader, SH baker) treat `intensity` as a straight
    // linear factor with no further scaling.
    let raw_intensity = parse_optional_int(props, "light")?
        .or(parse_optional_int(props, "_light")?)
        .map(|v| v as f32)
        .unwrap_or(QUAKE_INTENSITY_REFERENCE);

    if raw_intensity == 0.0 {
        log::warn!("light entity has intensity 0; it will contribute nothing");
    }

    let intensity = raw_intensity / QUAKE_INTENSITY_REFERENCE;

    // -- Color --
    let color = if let Some(color_str) = props.get("_color") {
        parse_color255(color_str).ok_or_else(|| TranslateError::InvalidProperty {
            key: "_color",
            value: color_str.clone(),
            reason: "expected three integers 0-255",
        })?
    } else {
        log::warn!("light entity missing '_color'; defaulting to white");
        [1.0, 1.0, 1.0]
    };

    // -- Falloff model --
    let falloff_model = match parse_optional_int(props, "delay")? {
        Some(0) | None => FalloffModel::Linear,
        Some(1) => FalloffModel::InverseDistance,
        Some(2) => FalloffModel::InverseSquared,
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "delay",
                value: other.to_string(),
                reason: "expected 0 (Linear), 1 (InverseDistance), or 2 (InverseSquared)",
            });
        }
    };

    // -- Falloff range --
    // `_fade` is authored in map units (Quake inches). Convert to engine
    // meters here so the canonical format is always in engine units.
    let map_scale = MapFormat::IdTech2.units_to_meters() as f32;
    let falloff_range = match light_type {
        LightType::Point | LightType::Spot => {
            let fade_units = parse_optional_int(props, "_fade")?
                .ok_or(TranslateError::MissingProperty("_fade"))?;
            if fade_units <= 0 {
                return Err(TranslateError::InvalidProperty {
                    key: "_fade",
                    value: fade_units.to_string(),
                    reason: "must be > 0",
                });
            }
            fade_units as f32 * map_scale
        }
        LightType::Directional => {
            // Directional lights ignore `falloff_range`. Store 0.0 for clarity.
            0.0
        }
    };

    // -- Cone angles and direction (Spot + Directional) --
    let mut cone_angle_inner = None;
    let mut cone_angle_outer = None;
    let mut cone_direction = None;

    match light_type {
        LightType::Spot => {
            if props.contains_key("target") {
                return Err(TranslateError::TargetNotSupported);
            }

            let inner_deg = match parse_optional_int(props, "_cone")? {
                Some(v) => v as f32,
                None => {
                    log::warn!("light_spot missing '_cone'; defaulting to 30 degrees inner");
                    30.0
                }
            };
            let outer_deg = match parse_optional_int(props, "_cone2")? {
                Some(v) => v as f32,
                None => {
                    log::warn!("light_spot missing '_cone2'; defaulting to 45 degrees outer");
                    45.0
                }
            };
            if inner_deg > outer_deg {
                log::warn!(
                    "light_spot _cone ({inner_deg}) > _cone2 ({outer_deg}); outer smaller than inner"
                );
            }
            cone_angle_inner = Some(inner_deg.to_radians());
            cone_angle_outer = Some(outer_deg.to_radians());

            let mangle_str = props
                .get("mangle")
                .filter(|s| !s.trim().is_empty())
                .ok_or(TranslateError::MissingProperty("mangle"))?;
            let dir = parse_mangle_direction(mangle_str).ok_or_else(|| {
                TranslateError::InvalidProperty {
                    key: "mangle",
                    value: mangle_str.clone(),
                    reason: "expected three numeric values: pitch yaw roll (degrees)",
                }
            })?;
            cone_direction = Some(dir);
        }
        LightType::Directional => {
            let dir = if let Some(mangle_str) = props.get("mangle").filter(|s| !s.trim().is_empty())
            {
                parse_mangle_direction(mangle_str).ok_or_else(|| {
                    TranslateError::InvalidProperty {
                        key: "mangle",
                        value: mangle_str.clone(),
                        reason: "expected three numeric values: pitch yaw roll (degrees)",
                    }
                })?
            } else {
                log::warn!("light_sun missing 'mangle'; defaulting to straight down (-90 0 0)");
                // "-90 0 0" → engine (0, -1, 0), matching sub-plan 1.
                parse_mangle_direction("-90 0 0").expect("built-in default mangle must parse")
            };
            cone_direction = Some(dir);
        }
        LightType::Point => {}
    }

    // -- Animation --
    let style = parse_optional_int(props, "style")?.unwrap_or_else(|| {
        log::warn!("light entity missing 'style'; defaulting to 0 (no animation)");
        0
    });

    let phase_raw = match props.get("_phase") {
        Some(s) => parse_f32(s).ok_or_else(|| TranslateError::InvalidProperty {
            key: "_phase",
            value: s.clone(),
            reason: "expected a float in 0.0-1.0",
        })?,
        None => 0.0,
    };
    let phase = if !(0.0..=1.0).contains(&phase_raw) {
        log::warn!("light _phase {phase_raw} outside 0.0-1.0; clamping");
        phase_raw.clamp(0.0, 1.0)
    } else {
        phase_raw
    };

    // `_start_inactive = 1` spawns the light dark. Defaults to 0 (active).
    // Only meaningful for animated lights — the flag rides on LightAnimation
    // because static lights have no runtime on/off state. We still parse and
    // warn on non-animated lights so authoring mistakes are visible.
    let start_inactive = match parse_optional_int(props, "_start_inactive")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "_start_inactive",
                value: other.to_string(),
                reason: "expected 0 (active at load) or 1 (inactive at load)",
            });
        }
    };

    // -- Bake only --
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

    // -- Dynamic flag --
    // Static (0) is the default: the light bakes into the lightmap + SH and
    // has no runtime presence. Dynamic (1) opts into the runtime direct path
    // with no bake contribution. Missing / non-integer values parse as static.
    let is_dynamic = match parse_optional_int(props, "_dynamic")? {
        None | Some(0) => false,
        Some(1) => true,
        Some(other) => {
            return Err(TranslateError::InvalidProperty {
                key: "_dynamic",
                value: other.to_string(),
                reason: "expected 0 (Static) or 1 (Dynamic)",
            });
        }
    };

    // -- Animation (curves override legacy style) --
    //
    // Curve authoring path: any of `brightness_curve`, `color_curve`, or
    // `direction_curve` present. These resample to uniform samples over
    // `period_ms` at compile time. If both `style` and `brightness_curve` are
    // present, the curve wins and `style` is ignored (warning emitted).
    let has_any_curve = props.contains_key("brightness_curve")
        || props.contains_key("color_curve")
        || props.contains_key("direction_curve");

    let animation = if has_any_curve {
        let light_ref = format_light_ref(classname, origin);

        // period_ms is required when curves are present.
        let period_ms_raw = props
            .get("period_ms")
            .ok_or(TranslateError::MissingProperty("period_ms"))?
            .trim();
        let period_ms: f32 =
            period_ms_raw
                .parse()
                .map_err(|_| TranslateError::InvalidProperty {
                    key: "period_ms",
                    value: period_ms_raw.to_string(),
                    reason: "expected a positive number (milliseconds)",
                })?;
        if !(period_ms > 0.0 && period_ms.is_finite()) {
            return Err(TranslateError::InvalidProperty {
                key: "period_ms",
                value: period_ms_raw.to_string(),
                reason: "expected a positive number (milliseconds)",
            });
        }

        // Curve phase (`_curve_phase`) is separate from the legacy `_phase`.
        let curve_phase = match props.get("_curve_phase") {
            Some(s) => {
                let v = parse_f32(s).ok_or_else(|| TranslateError::InvalidProperty {
                    key: "_curve_phase",
                    value: s.clone(),
                    reason: "expected a float in [0.0, 1.0)",
                })?;
                if !(0.0..1.0).contains(&v) {
                    log::warn!(
                        "light {light_ref}: '_curve_phase' {v} outside [0.0, 1.0); clamping"
                    );
                    v.clamp(0.0, 1.0 - f32::EPSILON)
                } else {
                    v
                }
            }
            None => 0.0,
        };

        // Warn + ignore `style` when a brightness curve is authored.
        if props.contains_key("brightness_curve") && style != 0 {
            log::warn!(
                "light {light_ref}: both 'brightness_curve' and 'style' set; \
                 'style' is ignored in favor of the authored curve"
            );
        }

        let brightness = if let Some(raw) = props.get("brightness_curve") {
            let keyframes = parse_scalar_curve(raw, "brightness_curve", &light_ref)?;
            Some(resample_keyframes(
                &keyframes,
                period_ms,
                KEYFRAME_RESAMPLE_RATE_HZ,
            ))
        } else {
            None
        };

        let color = if let Some(raw) = props.get("color_curve") {
            // Plan 2 Sub-plan 1 rule, surfaced here so the FGD key is named.
            if !bake_only && !is_dynamic {
                return Err(TranslateError::ColorCurveOnBakedLight { light_ref });
            }
            let keyframes = parse_vec3_curve(raw, "color_curve", &light_ref)?;
            Some(resample_keyframes(
                &keyframes,
                period_ms,
                KEYFRAME_RESAMPLE_RATE_HZ,
            ))
        } else {
            None
        };

        let direction = if let Some(raw) = props.get("direction_curve") {
            let keyframes = parse_vec3_curve(raw, "direction_curve", &light_ref)?;
            let mut samples = resample_keyframes(&keyframes, period_ms, KEYFRAME_RESAMPLE_RATE_HZ);
            // Direction samples are unit vectors at the authoring seam — the
            // GPU evaluator does not re-normalize.
            for v in samples.iter_mut() {
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if len > 1e-6 {
                    v[0] /= len;
                    v[1] /= len;
                    v[2] /= len;
                }
            }
            Some(samples)
        } else {
            None
        };

        Some(LightAnimation {
            period: period_ms / 1000.0,
            phase: curve_phase,
            brightness,
            color,
            direction,
            start_active: !start_inactive,
        })
    } else if style == 0 {
        if props.contains_key("_phase") && phase_raw != 0.0 {
            log::warn!("light _phase set but style=0 (no animation); phase has no effect");
        }
        if start_inactive {
            log::warn!(
                "light _start_inactive set but style=0 (no animation); static lights have no runtime toggle"
            );
        }
        None
    } else {
        match quake_style_animation(style, phase) {
            Some(mut anim) => {
                anim.start_active = !start_inactive;
                Some(anim)
            }
            None => {
                log::warn!("light style {style} has no preset defined; treating as constant");
                None
            }
        }
    };

    if bake_only && animation.is_some() {
        log::warn!(
            "light has _bake_only=1 and an animation set; animated indirect contribution will bake but the light has no runtime presence"
        );
    }

    Ok(MapLight {
        origin,
        light_type,
        intensity,
        color,
        falloff_model,
        falloff_range,
        cone_angle_inner,
        cone_angle_outer,
        cone_direction,
        animation,
        cast_shadows: true,
        bake_only,
        is_dynamic,
    })
}

// -- Property parsing helpers --

fn parse_optional_int(
    props: &HashMap<String, String>,
    key: &'static str,
) -> Result<Option<i32>, TranslateError> {
    match props.get(key) {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                // Accept integer-formatted floats as well ("30.0" → 30). FGD
                // editors occasionally emit values with trailing decimals.
                match trimmed.parse::<i32>() {
                    Ok(v) => Ok(Some(v)),
                    Err(_) => match trimmed.parse::<f32>() {
                        Ok(f) if f.fract() == 0.0 && f.is_finite() => Ok(Some(f as i32)),
                        _ => Err(TranslateError::InvalidProperty {
                            key,
                            value: s.clone(),
                            reason: "expected an integer",
                        }),
                    },
                }
            }
        }
        None => Ok(None),
    }
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok()
}

/// Parse a "R G B" triple (each 0-255) into linear RGB 0-1.
///
/// Conversion is direct division by 255 — no gamma correction. The FGD
/// colour picker produces sRGB values, but the lighting pipeline currently
/// treats authored colours as already linear. See sub-plan 2 for how this
/// feeds the SH baker.
fn parse_color255(s: &str) -> Option<[f32; 3]> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (i, p) in parts.iter().enumerate() {
        let v: i32 = p.parse().ok()?;
        if !(0..=255).contains(&v) {
            return None;
        }
        out[i] = v as f32 / 255.0;
    }
    Some(out)
}

/// Parse a Quake `mangle` string "pitch yaw roll" (degrees) into an
/// engine-space normalized aim vector.
///
/// Convention (per sub-plan 1): `"-90 0 0"` → `(0, -1, 0)` in engine space
/// (straight down). Roll is ignored for a direction vector.
///
/// Derivation:
/// 1. Quake forward vector from (pitch, yaw) — the convention that maps
///    `pitch=-90, yaw=0` to Quake `(0, 0, -1)` (down in Quake Z-up):
///    `qf_x = cos(pitch) * cos(yaw)`,
///    `qf_y = cos(pitch) * sin(yaw)`,
///    `qf_z = sin(pitch)`.
/// 2. Swizzle to engine space (Y-up):
///    `engine = (-qf_y, qf_z, -qf_x)`.
fn parse_mangle_direction(s: &str) -> Option<[f32; 3]> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let pitch_deg: f32 = parts[0].parse().ok()?;
    let yaw_deg: f32 = parts[1].parse().ok()?;
    // Roll parsed for validation (must be numeric) but unused.
    let _roll_deg: f32 = parts[2].parse().ok()?;

    let pitch = pitch_deg.to_radians();
    let yaw = yaw_deg.to_radians();

    let qf_x = pitch.cos() * yaw.cos();
    let qf_y = pitch.cos() * yaw.sin();
    let qf_z = pitch.sin();

    // Quake → engine swizzle (direction vector, no scale).
    // engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x.
    let ex = -qf_y;
    let ey = qf_z;
    let ez = -qf_x;

    let len = (ex * ex + ey * ey + ez * ez).sqrt();
    if len < 1e-6 {
        return None;
    }
    Some([ex / len, ey / len, ez / len])
}

// -- Keyframe curve parsing --
//
// Accepted syntax: space-separated bracketed entries, each a comma-separated
// list of numbers. `brightness_curve` entries are `[t_ms, value]`;
// `color_curve` and `direction_curve` entries are `[t_ms, a, b, c]`.
// Timestamps must be strictly monotonically increasing.

/// Format a short "classname @ (x, y, z)" label for error messages.
fn format_light_ref(classname: &str, origin: DVec3) -> String {
    format!(
        "{classname} @ ({:.3}, {:.3}, {:.3})",
        origin.x, origin.y, origin.z
    )
}

/// Split the curve value into individual bracketed entries.
///
/// Returns each entry's inner payload (without the brackets) as a `&str`.
/// Rejects nested brackets, unclosed brackets, and content outside brackets.
fn split_bracketed_entries<'a>(
    raw: &'a str,
    key: &'static str,
    light_ref: &str,
) -> Result<Vec<&'a str>, TranslateError> {
    let mut entries = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        let Some(open_rel) = rest.find('[') else {
            // Trailing non-whitespace without an opening bracket.
            if !rest.is_empty() {
                return Err(TranslateError::InvalidKeyframeCurve {
                    key,
                    light_ref: light_ref.to_string(),
                    reason: format!("unexpected content outside brackets: '{rest}'"),
                });
            }
            break;
        };
        // Any non-whitespace before the opening bracket is junk.
        let before = &rest[..open_rel];
        if !before.trim().is_empty() {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!("unexpected content outside brackets: '{}'", before.trim()),
            });
        }
        let after_open = &rest[open_rel + 1..];
        let Some(close_rel) = after_open.find(']') else {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: "unclosed '[' in keyframe list".to_string(),
            });
        };
        let inner = &after_open[..close_rel];
        if inner.contains('[') {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: "nested brackets are not allowed".to_string(),
            });
        }
        entries.push(inner);
        rest = after_open[close_rel + 1..].trim_start();
    }
    if entries.is_empty() {
        return Err(TranslateError::InvalidKeyframeCurve {
            key,
            light_ref: light_ref.to_string(),
            reason: "curve must contain at least one keyframe entry".to_string(),
        });
    }
    Ok(entries)
}

/// Parse a comma-separated list of floats from a bracketed entry's inner text.
fn parse_entry_numbers(inner: &str) -> Option<Vec<f32>> {
    inner
        .split(',')
        .map(|p| p.trim().parse::<f32>().ok())
        .collect()
}

/// Verify keyframe timestamps are strictly monotonically increasing.
fn check_monotonic<T>(
    keyframes: &[(f32, T)],
    key: &'static str,
    light_ref: &str,
) -> Result<(), TranslateError> {
    for window in keyframes.windows(2) {
        if window[1].0 <= window[0].0 {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!(
                    "keyframe timestamps must be strictly increasing: {} ms followed by {} ms",
                    window[0].0, window[1].0
                ),
            });
        }
    }
    Ok(())
}

fn parse_scalar_curve(
    raw: &str,
    key: &'static str,
    light_ref: &str,
) -> Result<Vec<(f32, f32)>, TranslateError> {
    let entries = split_bracketed_entries(raw, key, light_ref)?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let nums =
            parse_entry_numbers(entry).ok_or_else(|| TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!("'[{entry}]' contains a non-numeric value"),
            })?;
        if nums.len() != 2 {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!(
                    "'[{entry}]' has {} values; expected '[t_ms, value]' (2 values)",
                    nums.len()
                ),
            });
        }
        out.push((nums[0], nums[1]));
    }
    check_monotonic(&out, key, light_ref)?;
    Ok(out)
}

fn parse_vec3_curve(
    raw: &str,
    key: &'static str,
    light_ref: &str,
) -> Result<Vec<(f32, [f32; 3])>, TranslateError> {
    let entries = split_bracketed_entries(raw, key, light_ref)?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let nums =
            parse_entry_numbers(entry).ok_or_else(|| TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!("'[{entry}]' contains a non-numeric value"),
            })?;
        if nums.len() != 4 {
            return Err(TranslateError::InvalidKeyframeCurve {
                key,
                light_ref: light_ref.to_string(),
                reason: format!(
                    "'[{entry}]' has {} values; expected '[t_ms, a, b, c]' (4 values)",
                    nums.len()
                ),
            });
        }
        out.push((nums[0], [nums[1], nums[2], nums[3]]));
    }
    check_monotonic(&out, key, light_ref)?;
    Ok(out)
}

// -- Quake style preset table --

/// Map a Quake `style` integer (0-11) to a `LightAnimation`.
///
/// Classic brightness strings from Quake: each character `a`-`z` maps to
/// 0.0-1.0 (26 levels), sampled at 10 Hz. Style 0 is constant (no animation,
/// handled by the caller). Styles 12+ are reserved.
fn quake_style_animation(style: i32, phase: f32) -> Option<LightAnimation> {
    // Source: Quake 1 `r_light.c` / `m_menu.c` classic style strings.
    let pattern = match style {
        1 => "mmnmmommommnonmmonqnmmo", // flicker (first variety)
        2 => "abcdefghijklmnopqrstuvwxyzyxwvutsrqponmlkjihgfedcba", // slow strong pulse
        3 => "mmmmmaaaaammmmmaaaaaabcdefgabcdefg", // candle (first variety)
        4 => "mamamamamama",            // fast strobe
        5 => "jklmnopqrstuvwxyzyxwvutsrqponmlkj", // gentle pulse 1
        6 => "nmonqnmomnmomomno",       // flicker (second variety)
        7 => "mmmaaaabcdefgmmmmaaaammmaamm", // candle (second variety)
        8 => "mmmaaammmaaammmabcdefaaaammmmabcdefmmmaaaa", // candle (third variety)
        9 => "aaaaaaaazzzzzzzz",        // slow strobe (fourth variety)
        10 => "mmamammmmammamamaaamammma", // flourescent flicker
        11 => "abcdefghijklmnopqrrqponmlkjihgfedcba", // slow pulse, no black
        _ => return None,
    };

    let brightness: Vec<f32> = pattern
        .chars()
        .map(|c| {
            // 'a' → 0.0, 'z' → ~2.0 in Quake (each step = ~2/25 ≈ 0.08). The
            // classic mapping is `(c - 'a') * 2 / 25`, where 'm' (0.96) is
            // "normal" brightness. Normalised here so 'm' sits near 1.0.
            let step = (c as u8).saturating_sub(b'a') as f32;
            step * (2.0 / 25.0)
        })
        .collect();

    // Sampled at 10 Hz → period = samples * 0.1s.
    let period = brightness.len() as f32 * 0.1;

    Some(LightAnimation {
        period,
        phase,
        brightness: Some(brightness),
        color: None,
        direction: None,
        start_active: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn assert_vec_close(got: [f32; 3], want: [f32; 3], eps: f32, ctx: &str) {
        for i in 0..3 {
            assert!(
                (got[i] - want[i]).abs() < eps,
                "{ctx}: component {i} got {} want {} (tolerance {eps})",
                got[i],
                want[i]
            );
        }
    }

    // -- Basic valid translations --

    #[test]
    fn translates_valid_point_light() {
        let p = props(&[
            ("light", "250"),
            ("_color", "255 128 64"),
            ("_fade", "4096"),
            ("delay", "2"),
        ]);
        let light = translate_light(&p, DVec3::new(1.0, 2.0, 3.0), "light")
            .expect("point light should translate");

        assert_eq!(light.light_type, LightType::Point);
        // 250 / 300 (QUAKE_INTENSITY_REFERENCE)
        assert!((light.intensity - (250.0 / 300.0)).abs() < 1e-6);
        assert_vec_close(
            light.color,
            [1.0, 128.0 / 255.0, 64.0 / 255.0],
            1e-5,
            "color",
        );
        assert_eq!(light.falloff_model, FalloffModel::InverseSquared);
        // 4096 units * 0.0254 m/unit = 104.0384 m
        assert!((light.falloff_range - 104.0384).abs() < 1e-3);
        assert!(light.cone_angle_inner.is_none());
        assert!(light.cone_direction.is_none());
        assert!(light.animation.is_none());
        assert!(light.cast_shadows);
    }

    #[test]
    fn translates_valid_spot_light_via_mangle() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "2048"),
            ("_cone", "20"),
            ("_cone2", "40"),
            ("mangle", "-90 0 0"),
        ]);
        let light =
            translate_light(&p, DVec3::ZERO, "light_spot").expect("spot light should translate");

        assert_eq!(light.light_type, LightType::Spot);
        let inner = light.cone_angle_inner.expect("inner cone");
        let outer = light.cone_angle_outer.expect("outer cone");
        assert!((inner - 20.0f32.to_radians()).abs() < 1e-5);
        assert!((outer - 40.0f32.to_radians()).abs() < 1e-5);
        let dir = light.cone_direction.expect("cone direction");
        // -90 0 0 → straight down in engine space
        assert_vec_close(dir, [0.0, -1.0, 0.0], 1e-5, "spot direction");
    }

    #[test]
    fn translates_valid_directional_light() {
        let p = props(&[
            ("light", "200"),
            ("_color", "180 200 255"),
            ("mangle", "-45 0 0"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light_sun")
            .expect("directional light should translate");

        assert_eq!(light.light_type, LightType::Directional);
        // Directional ignores _fade.
        assert_eq!(light.falloff_range, 0.0);
        let dir = light.cone_direction.expect("directional dir");
        // -45 pitch, yaw 0: forward.y (Quake) = sin(-45) ≈ -0.707; forward.x (Quake) = cos(-45) ≈ 0.707.
        // Engine = (-qf_y, qf_z, -qf_x) = (0, -0.707, -0.707) normalised.
        assert_vec_close(dir, [0.0, -0.70710677, -0.70710677], 1e-4, "directional");
    }

    // -- Errors --

    #[test]
    fn point_missing_fade_errors() {
        let p = props(&[("light", "300"), ("_color", "255 255 255")]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        assert!(matches!(err, TranslateError::MissingProperty("_fade")));
    }

    #[test]
    fn spot_missing_fade_errors() {
        let p = props(&[
            ("light", "300"),
            ("_cone", "30"),
            ("_cone2", "45"),
            ("mangle", "0 0 0"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light_spot").expect_err("should error");
        assert!(matches!(err, TranslateError::MissingProperty("_fade")));
    }

    #[test]
    fn spot_missing_mangle_errors() {
        let p = props(&[
            ("light", "300"),
            ("_fade", "2048"),
            ("_cone", "30"),
            ("_cone2", "45"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light_spot").expect_err("should error");
        assert!(matches!(err, TranslateError::MissingProperty("mangle")));
    }

    #[test]
    fn spot_with_target_errors_pointing_to_mangle() {
        let p = props(&[
            ("light", "300"),
            ("_fade", "2048"),
            ("mangle", "-45 0 0"),
            ("target", "some_entity"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light_spot").expect_err("should error");
        assert!(matches!(err, TranslateError::TargetNotSupported));
    }

    #[test]
    fn mangle_non_numeric_errors() {
        let p = props(&[
            ("light", "300"),
            ("_fade", "2048"),
            ("_cone", "30"),
            ("_cone2", "45"),
            ("mangle", "down 0 banana"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light_spot").expect_err("should error");
        assert!(matches!(
            err,
            TranslateError::InvalidProperty { key: "mangle", .. }
        ));
    }

    #[test]
    fn unknown_classname_errors() {
        let err =
            translate_light(&props(&[]), DVec3::ZERO, "light_banana").expect_err("should error");
        assert!(matches!(err, TranslateError::UnknownClassname(_)));
    }

    // -- Property-name variation --

    #[test]
    fn accepts_underscore_light_alias() {
        let p = props(&[
            ("_light", "200"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        // 200 / 300 (QUAKE_INTENSITY_REFERENCE)
        assert!((light.intensity - (200.0 / 300.0)).abs() < 1e-6);
    }

    #[test]
    fn light_takes_precedence_over_underscore_light() {
        let p = props(&[
            ("light", "300"),
            ("_light", "999"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        // Authored 300 is the Quake reference value → normalized to 1.0.
        assert_eq!(light.intensity, 1.0);
    }

    // -- Style and animation --

    #[test]
    fn style_one_produces_animation_with_brightness_curve() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "1"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("style 1 should produce animation");
        let curve = anim.brightness.expect("brightness curve present");
        assert!(!curve.is_empty());
        // Period = samples * 0.1s — style 1 has 23 samples → 2.3s.
        assert!(
            (anim.period - curve.len() as f32 * 0.1).abs() < 1e-5,
            "period should match sample count at 10 Hz"
        );
        assert_eq!(anim.phase, 0.0);
        assert!(anim.color.is_none());
    }

    #[test]
    fn style_zero_produces_no_animation() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "0"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(light.animation.is_none());
    }

    #[test]
    fn phase_half_with_style_one_sets_phase() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "1"),
            ("_phase", "0.5"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        assert!((anim.phase - 0.5).abs() < 1e-6);
    }

    #[test]
    fn phase_out_of_range_is_clamped() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "1"),
            ("_phase", "1.5"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        assert!((anim.phase - 1.0).abs() < 1e-6);

        let p2 = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "1"),
            ("_phase", "-0.3"),
        ]);
        let light2 = translate_light(&p2, DVec3::ZERO, "light").expect("should translate");
        let anim2 = light2.animation.expect("animation present");
        assert_eq!(anim2.phase, 0.0);
    }

    #[test]
    fn phase_with_style_zero_is_ignored() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "0"),
            ("_phase", "0.5"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(light.animation.is_none());
    }

    // -- Defaults and warnings --

    #[test]
    fn directional_missing_mangle_defaults_to_down() {
        let p = props(&[("light", "200"), ("_color", "255 255 255")]);
        let light =
            translate_light(&p, DVec3::ZERO, "light_sun").expect("should translate with default");
        let dir = light.cone_direction.expect("direction");
        assert_vec_close(dir, [0.0, -1.0, 0.0], 1e-5, "default directional");
    }

    #[test]
    fn spot_with_cone_inner_larger_than_outer_warns_but_proceeds() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "2048"),
            ("_cone", "50"),
            ("_cone2", "30"),
            ("mangle", "-45 0 0"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light_spot").expect("should translate");
        let inner = light.cone_angle_inner.unwrap();
        let outer = light.cone_angle_outer.unwrap();
        assert!(inner > outer, "inner should remain larger than outer");
    }

    #[test]
    fn missing_color_defaults_to_white() {
        let p = props(&[("light", "300"), ("_fade", "1024")]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert_eq!(light.color, [1.0, 1.0, 1.0]);
    }

    // -- Unit conversion sanity --

    #[test]
    fn falloff_range_converts_quake_units_to_meters() {
        // 1000 Quake units at 0.0254 m/unit = 25.4 m.
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1000"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!((light.falloff_range - 25.4).abs() < 1e-4);
    }

    // -- _bake_only property --

    #[test]
    fn bake_only_default_is_false() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(!light.bake_only);
    }

    #[test]
    fn bake_only_zero_is_false() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_bake_only", "0"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(!light.bake_only);
    }

    #[test]
    fn bake_only_one_is_true() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_bake_only", "1"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(light.bake_only);
    }

    // -- _dynamic property --

    #[test]
    fn is_dynamic_default_is_false() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(!light.is_dynamic);
    }

    #[test]
    fn is_dynamic_zero_is_false() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_dynamic", "0"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(!light.is_dynamic);
    }

    #[test]
    fn is_dynamic_one_is_true() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_dynamic", "1"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        assert!(light.is_dynamic);
    }

    #[test]
    fn is_dynamic_invalid_errors() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_dynamic", "2"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        assert!(matches!(
            err,
            TranslateError::InvalidProperty {
                key: "_dynamic",
                ..
            }
        ));
    }

    // -- *_curve keyframe authoring --

    #[test]
    fn brightness_curve_produces_animation_samples_in_expected_range() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("brightness_curve", "[0, 0.1] [500, 1.0] [1000, 0.3]"),
            ("period_ms", "1000"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        let curve = anim.brightness.expect("brightness samples");
        // 1000 ms at 32 Hz → 32 samples.
        assert_eq!(curve.len(), 32);
        // Period is stored in seconds.
        assert!((anim.period - 1.0).abs() < 1e-6);
        // All resampled values must fall inside the authored 0.1..1.0 range
        // (Catmull-Rom with reflected endpoints on monotone endpoints stays
        // within the convex hull of these three keyframes).
        for v in &curve {
            assert!(
                *v >= 0.05 && *v <= 1.05,
                "brightness sample out of range: {v}"
            );
        }
    }

    #[test]
    fn direction_curve_produces_normalized_samples() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_cone", "20"),
            ("_cone2", "40"),
            ("mangle", "-90 0 0"),
            ("direction_curve", "[0, 1, 0, 0] [1000, 0, 0, 1]"),
            ("period_ms", "1000"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light_spot").expect("should translate");
        let anim = light.animation.expect("animation present");
        let dir = anim.direction.expect("direction samples");
        assert!(!dir.is_empty());
        for v in &dir {
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "direction sample not unit length: {v:?} (len {len})"
            );
        }
    }

    #[test]
    fn curve_wrong_arity_errors_with_key_named() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            // brightness_curve expects [t, v]; this has 3 values.
            ("brightness_curve", "[0, 0.5, 9] [500, 1.0]"),
            ("period_ms", "500"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        match err {
            TranslateError::InvalidKeyframeCurve { key, light_ref, .. } => {
                assert_eq!(key, "brightness_curve");
                assert!(light_ref.contains("light"));
            }
            other => panic!("expected InvalidKeyframeCurve, got {other:?}"),
        }
    }

    #[test]
    fn curve_non_monotonic_timestamps_error_with_key_named() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("brightness_curve", "[500, 0.5] [200, 1.0]"),
            ("period_ms", "1000"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        match err {
            TranslateError::InvalidKeyframeCurve { key, .. } => {
                assert_eq!(key, "brightness_curve");
            }
            other => panic!("expected InvalidKeyframeCurve, got {other:?}"),
        }
    }

    #[test]
    fn color_curve_on_baked_light_errors_naming_fgd_key() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("color_curve", "[0, 1, 0, 0] [500, 0, 1, 0]"),
            ("period_ms", "500"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        match err {
            TranslateError::ColorCurveOnBakedLight { light_ref } => {
                assert!(light_ref.contains("light"));
            }
            other => panic!("expected ColorCurveOnBakedLight, got {other:?}"),
        }
    }

    #[test]
    fn color_curve_on_bake_only_light_is_accepted() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_bake_only", "1"),
            ("color_curve", "[0, 1, 0, 0] [500, 0, 1, 0]"),
            ("period_ms", "500"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        assert!(anim.color.is_some());
    }

    #[test]
    fn color_curve_on_dynamic_light_is_accepted() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_dynamic", "1"),
            ("color_curve", "[0, 1, 0, 0] [500, 0, 1, 0]"),
            ("period_ms", "500"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        assert!(anim.color.is_some());
    }

    #[test]
    fn brightness_curve_wins_over_style() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("style", "1"),
            ("brightness_curve", "[0, 0.2] [1000, 0.8]"),
            ("period_ms", "1000"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        // Style 1 would produce a 2.3s period; the curve's 1.0s period means
        // the curve won.
        assert!((anim.period - 1.0).abs() < 1e-5);
    }

    #[test]
    fn curve_missing_period_ms_errors() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("brightness_curve", "[0, 0.5] [500, 1.0]"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        assert!(matches!(err, TranslateError::MissingProperty("period_ms")));
    }

    #[test]
    fn curve_phase_parses_separately_from_legacy_phase() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("brightness_curve", "[0, 0.2] [1000, 0.8]"),
            ("period_ms", "1000"),
            ("_curve_phase", "0.25"),
        ]);
        let light = translate_light(&p, DVec3::ZERO, "light").expect("should translate");
        let anim = light.animation.expect("animation present");
        assert!((anim.phase - 0.25).abs() < 1e-6);
    }

    #[test]
    fn bake_only_invalid_errors() {
        let p = props(&[
            ("light", "300"),
            ("_color", "255 255 255"),
            ("_fade", "1024"),
            ("_bake_only", "2"),
        ]);
        let err = translate_light(&p, DVec3::ZERO, "light").expect_err("should error");
        assert!(matches!(
            err,
            TranslateError::InvalidProperty {
                key: "_bake_only",
                ..
            }
        ));
    }
}
