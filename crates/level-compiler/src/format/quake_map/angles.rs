// Quake-family entity angle keys (`angle` / `angles` / `mangle`) to engine
// Euler radians — the one conversion point for point-entity orientation.
// See: context/lib/build_pipeline.md §Supported Map Formats

/// Convert Quake-authored angles into engine-convention Euler radians
/// (pitch, yaw, roll), reading whichever of `angles` / `mangle` / `angle` is
/// present in `props`. Returns `[0.0; 3]` when no angle key is set.
///
/// Malformed values log a `[Loader]`-style warning and fall back to zeros so
/// the compile keeps going; bad angle data is recoverable. A caller that must
/// refuse bad angles instead uses [`try_quake_to_engine_angles`].
pub fn quake_to_engine_angles(
    props: &std::collections::HashMap<String, String>,
    diagnostic_ref: &str,
) -> [f32; 3] {
    try_quake_to_engine_angles(props).unwrap_or_else(|reason| {
        log::warn!("[Loader] {diagnostic_ref}: {reason}; using zeros");
        [0.0; 3]
    })
}

/// The one Quake-angle conversion point for point entities: the fallible form
/// of [`quake_to_engine_angles`]. `Err` carries a reason naming the key.
///
/// Quake authoring conventions:
/// - `angles "pitch yaw roll"` (degrees): full three-axis orientation. `mangle`
///   is the same shape; some editors emit one or the other. Either wins over
///   `angle`.
/// - `angle <yaw>` (degrees): legacy single-axis form, yaw only. Two reserved
///   sentinel values pre-date `angles`: `-1` = straight up, `-2` = straight
///   down (encoded as ±90° pitch).
///
/// Engine convention: a `Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll)`
/// rotation, whose forward is `-Z`. Yaw keeps its sign. The axis swizzle
/// (`parse::quake_to_engine`: engine = (-qy, qz, -qx)) is a proper rotation that
/// takes Quake up (+Z) to engine up (+Y), so a counter-clockwise turn about up
/// stays counter-clockwise: Quake yaw θ faces (cos θ, sin θ, 0), which swizzles
/// to (-sin θ, 0, -cos θ) — exactly engine yaw θ applied to `-Z`. Yaw 90 faces
/// Quake +Y, the way TrenchBroom draws the arrow. Pitch and roll also keep their
/// sign; engine pitch is positive nose-up, matching the spotlight `angles`
/// convention (`"-90 0 0"` points down). A model authored facing `+Z` turns
/// half a revolution at spawn so it faces the arrow too
/// (`MapEntity::model_rotation_quat` in `postretro-sim`).
pub fn try_quake_to_engine_angles(
    props: &std::collections::HashMap<String, String>,
) -> Result<[f32; 3], String> {
    let to_radians = |key: &str, raw: &str, deg: [f32; 3]| {
        let radians = deg.map(f32::to_radians);
        if radians.iter().all(|v| v.is_finite()) {
            Ok(radians)
        } else {
            Err(format!("'{key}' has non-finite component(s) '{raw}'"))
        }
    };
    // Prefer `angles`/`mangle` (full 3-axis) over the legacy `angle` (yaw only).
    let three_axis = props
        .get("angles")
        .map(|raw| ("angles", raw))
        .or_else(|| props.get("mangle").map(|raw| ("mangle", raw)));
    if let Some((key, raw)) = three_axis {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok([0.0; 3]);
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(format!(
                "'{key}' has {} components, expected 3",
                parts.len()
            ));
        }
        let mut deg = [0.0f32; 3];
        for (slot, part) in deg.iter_mut().zip(&parts) {
            *slot = part
                .parse()
                .map_err(|_| format!("'{key}' contains non-numeric component(s) '{raw}'"))?;
        }
        return to_radians(key, raw, deg);
    }

    if let Some(raw) = props.get("angle") {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok([0.0; 3]);
        }
        let yaw_deg: f32 = trimmed
            .parse()
            .map_err(|_| format!("'angle' is not numeric ('{raw}')"))?;
        // Quake sentinels: -1 = up, -2 = down. Encoded as pitch alone.
        if (yaw_deg + 1.0).abs() < f32::EPSILON {
            return Ok([std::f32::consts::FRAC_PI_2, 0.0, 0.0]);
        }
        if (yaw_deg + 2.0).abs() < f32::EPSILON {
            return Ok([-std::f32::consts::FRAC_PI_2, 0.0, 0.0]);
        }
        return to_radians("angle", raw, [0.0, yaw_deg, 0.0]);
    }

    Ok([0.0; 3])
}
