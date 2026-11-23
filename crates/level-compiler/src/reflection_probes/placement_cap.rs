// Reflection-probe cap: which placed probes keep a slot past the per-level probe cap, and the warning for the rest.
// See: context/lib/resource_management.md §5.1 (Drops and cap), §5.4 (Cap)

use super::{PlacedProbe, tied};

/// Keep at most `limit` derived probes, the room `cap` leaves after the
/// authored ones, the largest bound surface area first; areas within
/// [`super::TIE_RELATIVE_TOLERANCE`] tie, and ties go to the earlier probe, which is
/// fluid entity then face order. Each kept slot goes to the earliest probe
/// tied with the largest area left, so the choice is deterministic though a
/// tolerance is not a total order. The kept probes stay in their order.
/// Dropped probes' faces bind nothing; one warning names the count.
pub(super) fn apply_probe_cap(
    probes: Vec<PlacedProbe>,
    limit: usize,
    cap: usize,
) -> Vec<PlacedProbe> {
    let dropped = probes.len().saturating_sub(limit);
    let kept = keep_largest(probes, limit);
    if dropped > 0 {
        log::warn!(
            "[ReflectionProbes] {dropped} derived probe(s) past the {cap}-probe cap dropped, \
             smallest bound water first; their faces reflect nothing"
        );
    }
    kept
}

/// [`apply_probe_cap`] for derived world probes, in the room water leaves:
/// the smallest bound area drops first.
pub(super) fn apply_world_probe_cap(
    probes: Vec<PlacedProbe>,
    limit: usize,
    cap: usize,
) -> Vec<PlacedProbe> {
    let dropped = probes.len().saturating_sub(limit);
    let kept = keep_largest(probes, limit);
    if dropped > 0 {
        log::warn!(
            "[ReflectionProbes] {dropped} glossy world probe(s) past the {cap}-probe cap \
             dropped, smallest bound area first; their faces reflect nothing"
        );
    }
    kept
}

/// At most `limit` of `probes`, largest bound area first by the tie rule of
/// [`apply_probe_cap`], in their order.
fn keep_largest(probes: Vec<PlacedProbe>, limit: usize) -> Vec<PlacedProbe> {
    if probes.len() <= limit {
        return probes;
    }
    let mut keep = vec![false; probes.len()];
    for _ in 0..limit {
        let largest = (0..probes.len())
            .filter(|&i| !keep[i])
            .map(|i| probes[i].bound_area)
            .fold(f64::NEG_INFINITY, f64::max);
        let pick = (0..probes.len())
            .find(|&i| !keep[i] && tied(probes[i].bound_area, largest))
            .expect("a probe is left while fewer than `limit` are kept");
        keep[pick] = true;
    }
    probes
        .into_iter()
        .zip(keep)
        .filter_map(|(probe, kept)| kept.then_some(probe))
        .collect()
}
