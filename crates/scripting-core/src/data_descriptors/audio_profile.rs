// VM-agnostic resolution of the manifest's `audio.attenuation` block, shared by
// the QuickJS and Luau drains so both runtimes validate and degrade identically.
// See: context/lib/scripting.md §1 · context/lib/audio.md §5

use super::{ModAttenuation, ModAttenuationCurve};

/// One authored `audio.attenuation` field as a VM drain classifies it. The
/// drains only sort a raw value into these buckets; every validation rule and
/// warning lives in [`resolve_authored_attenuation`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AuthoredAttenuationField {
    /// Missing, `null`, `undefined`, or `nil`: the engine seed applies silently.
    Absent,
    Number(f64),
    String(String),
    /// Any other value type (boolean, object, table, function, ...).
    Other,
}

/// Resolve the authored attenuation fields against the engine seed.
///
/// Absent fields take the seed. A malformed field warns naming it, and so does
/// a merged `minDistance >= maxDistance`; either way the whole seeded
/// attenuation applies, since mixing authored and seeded halves would produce
/// a falloff nobody authored. Never fails: presentation preferences must not
/// reject an otherwise valid manifest.
pub(crate) fn resolve_authored_attenuation(
    scope: &str,
    min_distance: AuthoredAttenuationField,
    max_distance: AuthoredAttenuationField,
    curve: AuthoredAttenuationField,
) -> ModAttenuation {
    let seed = ModAttenuation::DEFAULT;
    let min_distance = authored_distance(scope, "minDistance", min_distance, seed.min_distance);
    let max_distance = authored_distance(scope, "maxDistance", max_distance, seed.max_distance);
    let curve = authored_curve(scope, curve, seed.curve);
    let (Some(min_distance), Some(max_distance), Some(curve)) = (min_distance, max_distance, curve)
    else {
        return seed;
    };
    if min_distance >= max_distance {
        log::warn!(
            "[Scripting] {scope}: `audio.attenuation.minDistance` ({min_distance}) must be less than `audio.attenuation.maxDistance` ({max_distance}); using the default attenuation"
        );
        return seed;
    }
    ModAttenuation {
        min_distance,
        max_distance,
        curve,
    }
}

/// `None` means the field was malformed and has already warned.
fn authored_distance(
    scope: &str,
    field: &str,
    authored: AuthoredAttenuationField,
    seed: f32,
) -> Option<f32> {
    let value = match authored {
        AuthoredAttenuationField::Absent => return Some(seed),
        AuthoredAttenuationField::Number(value) => Some(value),
        AuthoredAttenuationField::String(_) | AuthoredAttenuationField::Other => None,
    };
    let valid =
        value.filter(|value| value.is_finite() && *value >= 0.0 && (*value as f32).is_finite());
    if valid.is_none() {
        log::warn!(
            "[Scripting] {scope}: `audio.attenuation.{field}` must be a finite non-negative number; using the default attenuation"
        );
    }
    valid.map(|value| value as f32)
}

/// `None` means the field was malformed and has already warned.
fn authored_curve(
    scope: &str,
    authored: AuthoredAttenuationField,
    seed: ModAttenuationCurve,
) -> Option<ModAttenuationCurve> {
    let curve = match &authored {
        AuthoredAttenuationField::Absent => return Some(seed),
        AuthoredAttenuationField::String(value) => match value.as_str() {
            "linear" => Some(ModAttenuationCurve::Linear),
            "quadratic" => Some(ModAttenuationCurve::Quadratic),
            _ => None,
        },
        AuthoredAttenuationField::Number(_) | AuthoredAttenuationField::Other => None,
    };
    if curve.is_none() {
        log::warn!(
            "[Scripting] {scope}: `audio.attenuation.curve` must be `linear` or `quadratic`; using the default attenuation"
        );
    }
    curve
}
