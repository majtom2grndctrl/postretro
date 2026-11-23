// Reflection-probe cap: which placed probes keep a slot past the per-level probe cap, and the refusal and warnings for the rest.
// See: context/lib/resource_management.md §5.1 (Drops and cap), §5.4 (Cap)

use crate::map_data::MapReflectionVolume;

use super::{PlacedProbe, ReflectorSource, tied};
use crate::reflection_probes::front_side::describe;

/// Whether `probe` binds water: such a probe keeps its slot ahead of every
/// probe binding world faces alone.
pub(super) fn binds_water(probe: &PlacedProbe) -> bool {
    !probe.fluid_faces.is_empty()
}

/// Refuse the build when the authored probes binding water pass the cap
/// alone: no drop order makes room for them. The error counts them and
/// names each box that makes one with the probes it makes. Authored probes
/// binding world faces alone never refuse: they drop first instead.
pub(super) fn refuse_authored_water_past_cap(
    probes: &[PlacedProbe],
    volumes: &[MapReflectionVolume],
    cap: usize,
) -> anyhow::Result<()> {
    let water = probes.iter().filter(|probe| binds_water(probe)).count();
    if water <= cap {
        return Ok(());
    }
    let boxes: Vec<String> = volumes
        .iter()
        .filter_map(|volume| {
            let made: Vec<&PlacedProbe> = probes
                .iter()
                .filter(|probe| probe.source_entity == volume.source_entity)
                .collect();
            let binding_water = made.iter().filter(|probe| binds_water(probe)).count();
            (binding_water > 0).then(|| {
                format!(
                    "{} makes {} probe(s), {binding_water} binding water",
                    volume.source_label,
                    made.len()
                )
            })
        })
        .collect();
    anyhow::bail!(
        "{water} reflection volume probe(s) bind water, past the {cap}-probe cap: {}; remove or \
         merge reflection volumes until at most {cap} probes bind water",
        boxes.join(", ")
    )
}

/// Keep at most `limit` derived water probes, the room `cap` leaves after
/// the authored probes binding water, the largest bound surface area first;
/// areas within [`super::TIE_RELATIVE_TOLERANCE`] tie, and ties go to the
/// earlier probe, which is fluid entity then face order. Each kept slot goes
/// to the earliest probe tied with the largest area left, so the choice is
/// deterministic though a tolerance is not a total order. The kept probes
/// stay in their order. Dropped probes' faces bind nothing; one warning
/// counts them and names each one's faces.
pub(super) fn apply_probe_cap(
    probes: Vec<PlacedProbe>,
    limit: usize,
    cap: usize,
) -> Vec<PlacedProbe> {
    let keep = keep_largest(&probes, limit);
    let dropped: Vec<String> = probes
        .iter()
        .zip(&keep)
        .filter(|&(_, &kept)| !kept)
        .map(|(probe, _)| {
            format!(
                "fluid entity {}'s probe ({})",
                probe.source_entity,
                faces_of(probe)
            )
        })
        .collect();
    if !dropped.is_empty() {
        log::warn!(
            "[ReflectionProbes] {} derived probe(s) past the {cap}-probe cap dropped, \
             smallest bound water first; their faces reflect nothing: {}",
            dropped.len(),
            dropped.join("; ")
        );
    }
    retain(probes, &keep)
}

/// Which of `probes`, each binding world faces alone, keep a slot in the
/// `limit` the probes binding water leave: authored and derived alike, the
/// smallest bound area dropping first, by [`apply_probe_cap`]'s tie rule in
/// the order given. One warning counts the dropped probes and names each
/// one's faces, and for an authored probe its box (`volumes`) and facing.
pub(super) fn world_probes_kept(
    probes: &[&PlacedProbe],
    volumes: &[MapReflectionVolume],
    limit: usize,
    cap: usize,
) -> Vec<bool> {
    let keep = keep_largest(probes, limit);
    let dropped: Vec<String> = probes
        .iter()
        .zip(&keep)
        .filter(|&(_, &kept)| !kept)
        .map(|(probe, _)| {
            let source = if probe.authored {
                let label = volumes
                    .iter()
                    .find(|volume| volume.source_entity == probe.source_entity)
                    .map_or_else(
                        || format!("entity {}", probe.source_entity),
                        |volume| volume.source_label.clone(),
                    );
                format!("{label} probe {}", probe.facing)
            } else {
                "derived probe".to_string()
            };
            format!("{source} ({})", faces_of(probe))
        })
        .collect();
    if !dropped.is_empty() {
        log::warn!(
            "[ReflectionProbes] {} glossy world probe(s) past the {cap}-probe cap \
             dropped, smallest bound area first; their faces reflect nothing: {}",
            dropped.len(),
            dropped.join("; ")
        );
    }
    keep
}

/// Each face `probe` binds, as the front-side warning names them.
fn faces_of(probe: &PlacedProbe) -> String {
    let sources = probe
        .fluid_faces
        .iter()
        .map(|&(volume, face)| ReflectorSource::Fluid { volume, face })
        .chain(
            probe
                .world_faces
                .iter()
                .map(|&face| ReflectorSource::World { face }),
        );
    sources
        .map(|source| describe(&source))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which of `probes` keep one of at most `limit` slots, the largest bound
/// area first by [`apply_probe_cap`]'s tie rule.
fn keep_largest<P: std::borrow::Borrow<PlacedProbe>>(probes: &[P], limit: usize) -> Vec<bool> {
    let area = |i: usize| probes[i].borrow().bound_area;
    if probes.len() <= limit {
        return vec![true; probes.len()];
    }
    let mut keep = vec![false; probes.len()];
    for _ in 0..limit {
        let largest = (0..probes.len())
            .filter(|&i| !keep[i])
            .map(area)
            .fold(f64::NEG_INFINITY, f64::max);
        let pick = (0..probes.len())
            .find(|&i| !keep[i] && tied(area(i), largest))
            .expect("a probe is left while fewer than `limit` are kept");
        keep[pick] = true;
    }
    keep
}

/// The `items` `keep` marks, in their order.
pub(super) fn retain<T>(items: Vec<T>, keep: &[bool]) -> Vec<T> {
    items
        .into_iter()
        .zip(keep)
        .filter_map(|(item, &kept)| kept.then_some(item))
        .collect()
}
