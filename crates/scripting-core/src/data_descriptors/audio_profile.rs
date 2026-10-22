// VM-agnostic resolution of the manifest's `audio.attenuation` and `audio.ghost`
// blocks, shared by the QuickJS and Luau drains so both runtimes validate and
// degrade identically.
// See: context/lib/scripting.md §1 · context/lib/audio.md §1, §5

use super::{ModAttenuation, ModAttenuationCurve, ModGhostAudio};

/// One authored `audio` profile field as a VM drain classifies it. The drains
/// only sort a raw value into these buckets; every validation rule and warning
/// lives in [`resolve_authored_attenuation`] and [`resolve_authored_ghost_audio`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AuthoredAudioField {
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
    min_distance: AuthoredAudioField,
    max_distance: AuthoredAudioField,
    curve: AuthoredAudioField,
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
    authored: AuthoredAudioField,
    seed: f32,
) -> Option<f32> {
    let value = match authored {
        AuthoredAudioField::Absent => return Some(seed),
        AuthoredAudioField::Number(value) => Some(value),
        AuthoredAudioField::String(_) | AuthoredAudioField::Other => None,
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
    authored: AuthoredAudioField,
    seed: ModAttenuationCurve,
) -> Option<ModAttenuationCurve> {
    let curve = match &authored {
        AuthoredAudioField::Absent => return Some(seed),
        AuthoredAudioField::String(value) => match value.as_str() {
            "linear" => Some(ModAttenuationCurve::Linear),
            "quadratic" => Some(ModAttenuationCurve::Quadratic),
            _ => None,
        },
        AuthoredAudioField::Number(_) | AuthoredAudioField::Other => None,
    };
    if curve.is_none() {
        log::warn!(
            "[Scripting] {scope}: `audio.attenuation.curve` must be `linear` or `quadratic`; using the default attenuation"
        );
    }
    curve
}

/// Resolve the authored `audio.ghost` fields against the floor profile.
///
/// Absent fields take the floor value. A malformed field — not a number,
/// non-finite, or out of range (`volume` and `drive` in `0..=1`, cutoffs in
/// `20..=20000` Hz) — warns naming it, and so does a `highPassHz` at or above
/// `lowPassHz` once absent fields are filled; either way the whole floor
/// profile applies, never a mix of authored and floor halves. Never fails: a
/// presentation preference must not reject an otherwise valid manifest.
pub(crate) fn resolve_authored_ghost_audio(
    scope: &str,
    volume: AuthoredAudioField,
    high_pass_hz: AuthoredAudioField,
    low_pass_hz: AuthoredAudioField,
    drive: AuthoredAudioField,
) -> ModGhostAudio {
    let floor = ModGhostAudio::FLOOR;
    let cutoffs = ModGhostAudio::MIN_CUTOFF_HZ..=ModGhostAudio::MAX_CUTOFF_HZ;
    let volume = authored_ghost_field(scope, "volume", volume, floor.volume, 0.0..=1.0);
    let high_pass_hz = authored_ghost_field(
        scope,
        "highPassHz",
        high_pass_hz,
        floor.high_pass_hz,
        cutoffs.clone(),
    );
    let low_pass_hz =
        authored_ghost_field(scope, "lowPassHz", low_pass_hz, floor.low_pass_hz, cutoffs);
    let drive = authored_ghost_field(scope, "drive", drive, floor.drive, 0.0..=1.0);
    let (Some(volume), Some(high_pass_hz), Some(low_pass_hz), Some(drive)) =
        (volume, high_pass_hz, low_pass_hz, drive)
    else {
        return floor;
    };
    if high_pass_hz >= low_pass_hz {
        log::warn!(
            "[Scripting] {scope}: `audio.ghost.highPassHz` ({high_pass_hz}) must be below `audio.ghost.lowPassHz` ({low_pass_hz}); using the floor ghost bus profile"
        );
        return floor;
    }
    ModGhostAudio {
        volume,
        high_pass_hz,
        low_pass_hz,
        drive,
    }
}

/// `None` means the field was malformed and has already warned.
fn authored_ghost_field(
    scope: &str,
    field: &str,
    authored: AuthoredAudioField,
    floor: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Option<f32> {
    let value = match authored {
        AuthoredAudioField::Absent => return Some(floor),
        AuthoredAudioField::Number(value) => Some(value),
        AuthoredAudioField::String(_) | AuthoredAudioField::Other => None,
    };
    // Judged in f64 before the cast, so a value just past a bound is refused
    // rather than rounded onto it (the same order `setGhostAudio` uses).
    let (low, high) = (f64::from(*range.start()), f64::from(*range.end()));
    let valid = value
        .filter(|value| value.is_finite() && (low..=high).contains(value))
        .map(|value| value as f32);
    if valid.is_none() {
        log::warn!(
            "[Scripting] {scope}: `audio.ghost.{field}` must be a finite number in {}..={}; using the floor ghost bus profile",
            range.start(),
            range.end(),
        );
    }
    valid
}
