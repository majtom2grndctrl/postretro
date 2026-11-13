// Enclosure-run edge copy: which pose's value state each pose stores.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::HashMap;

use postretro_level_format::mover_probe_channel::{
    Knot, MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark, is_terminus,
};
use rayon::prelude::*;

use super::bake::MoverBounceJob;
use super::direct::{LightEntries, ProbeCells};
use super::posed_solid::{PosedMover, PosedMovers};
use super::states::{BounceProbe, MoverBounceStates};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// Gaps in t′ within this of each other tie. Knots are `f32`, so equal
/// spacings measured at different t′ (0.1 to 0.2 against 0.2 to 0.3) differ
/// in their last bits; a hundred-thousandth of the travel is far below any
/// pose spacing the lighting can show.
const TIE: f64 = 1.0e-5;

/// For each pose of a mover, in path order, the pose whose value state it
/// stores for one probe: itself, or a clear pose where it is the first or
/// last read pose of a run that encloses the probe.
///
/// Runs and their neighbours are found over `read`, the knots compose reads
/// (the mover's knots through its author ramp, [`effective_knots`]), at
/// each pose's t′. A pose the ramp squeezes out is never read, so it is
/// absent here: it neither belongs to a run nor serves as a source, and it
/// keeps its own state. `enclosed` holds one entry per pose, by the bake's
/// enclosing rule.
///
/// A maximal run of read poses that enclose the probe has its first pose
/// take the read clear pose before it, and its last pose the read clear
/// pose after it; either may be a terminus. A one-pose run takes the nearer
/// of the two in t′, the later on a tie (`TIE`). A run end at a terminus
/// keeps its own, since a terminus that encloses the probe reads the base.
/// Poses inside a run keep their own dark state, which stops a probe layer
/// inside a thin wall lighting the room behind it. A pose added further
/// inside a run (a bounding pose) is an interior pose, so it stays dark.
///
/// Why the edges copy: an enclosed pose stores the dark depth tile beside
/// its dark value. Leaving the enclosure, the mean-and-variance depth blend
/// opens the probe's Chebyshev weight concavely while its value arrives
/// linearly, so the probe weighs in while still mostly dark and the band
/// beside it dips before it rises. The clear neighbour's value at the edge
/// holds the value at its lit level while depth opens. Depth keeps the dark
/// tile at every enclosed pose.
///
/// IP: a stored state the bake chooses from geometry, as every pose's state
/// is. The section gains no per-probe bit, and compose still blends between
/// neighbouring stored states with every valid probe's weight untouched.
///
/// [`effective_knots`]: postretro_level_format::mover_probe_channel::effective_knots
pub(crate) fn value_sources(enclosed: &[bool], read: &[Knot]) -> Vec<usize> {
    let count = enclosed.len();
    debug_assert!(
        read.len() >= 2 && read[0].pose == 0 && read[read.len() - 1].pose as usize + 1 == count,
        "read knots run from the first pose to the last"
    );
    let mut sources: Vec<usize> = (0..count).collect();
    let pose = |k: usize| read[k].pose as usize;
    let last = read.len() - 1;
    let mut at = 0;
    while at <= last {
        if !enclosed[pose(at)] {
            at += 1;
            continue;
        }
        let first = at;
        while at <= last && enclosed[pose(at)] {
            at += 1;
        }
        let end = at - 1;
        // A run is maximal over the read knots, so the knot before a run
        // starting past knot 0, or after one ending before the last, is
        // read and clear.
        if first == end {
            if first != 0 && first != last {
                let before = f64::from(read[first].t) - f64::from(read[first - 1].t);
                let after = f64::from(read[first + 1].t) - f64::from(read[first].t);
                let from = if before < after - TIE {
                    first - 1
                } else {
                    first + 1
                };
                sources[pose(first)] = pose(from);
            }
            continue;
        }
        if first != 0 {
            sources[pose(first)] = pose(first - 1);
        }
        if end != last {
            sources[pose(end)] = pose(end + 1);
        }
    }
    sources
}

/// Whether `sources` moves any pose's value.
fn copies(sources: &[usize]) -> bool {
    sources.iter().enumerate().any(|(pose, &from)| pose != from)
}

/// Apply [`value_sources`] to one mover's bounce states, before skipping
/// judges them, so skipping judges what compose reads. `read` is the knots
/// compose reads; `enclosed(probe)` gives each pose's enclosure of a probe.
/// A copied pose takes what compose reads at its source pose: its stored
/// tile, or the base where the source reads it (a terminus storing nothing
/// reads the base, so the mid pose marks base). A probe left storing
/// nothing at any pose reads the base everywhere, so it is dropped from the
/// bounce states. Depth is untouched: every enclosed pose keeps the dark
/// tile.
/// Returns the (probe, pose) values copied.
pub(crate) fn copy_bounce_edges(
    states: &mut MoverBounceStates,
    read: &[Knot],
    enclosed: impl Fn(u32) -> Vec<bool> + Sync,
) -> usize {
    if states.pose_count <= 2 || states.bounce.is_empty() {
        return 0;
    }
    let copied: Vec<(Option<BounceProbe>, usize)> = std::mem::take(&mut states.bounce)
        .into_par_iter()
        .map(|probe| {
            let sources = value_sources(&enclosed(probe.probe), read);
            if !copies(&sources) {
                return (Some(probe), 0);
            }
            let moved = sources
                .iter()
                .enumerate()
                .filter(|&(pose, &from)| pose != from)
                .count();
            let probe = with_sources(&probe, &sources);
            (
                probe.marks.contains(&PoseMark::Stored).then_some(probe),
                moved,
            )
        })
        .collect();
    let mut moved = 0;
    let mut bounce = Vec::with_capacity(copied.len());
    for (probe, count) in copied {
        moved += count;
        bounce.extend(probe);
    }
    let depth = std::mem::take(&mut states.depth);
    states.replace(bounce, depth, 0);
    moved
}

/// `probe` with each pose's mark and tile taken from its source pose.
fn with_sources(probe: &BounceProbe, sources: &[usize]) -> BounceProbe {
    let count = sources.len();
    let mut tiles_at = Vec::with_capacity(count);
    let mut next = 0;
    for &mark in &probe.marks {
        tiles_at.push((mark == PoseMark::Stored).then(|| {
            let at = next;
            next += MOVER_IRRADIANCE_TILE_F16_COUNT;
            &probe.tiles[at..next]
        }));
    }
    let mut marks = Vec::with_capacity(count);
    let mut tiles = Vec::with_capacity(probe.tiles.len());
    for (pose, &from) in sources.iter().enumerate() {
        let mark = match probe.marks[from] {
            // What compose reads at a terminus storing nothing: the base.
            PoseMark::Unmarked if from != pose && is_terminus(from, count) => PoseMark::Base,
            mark => mark,
        };
        if let Some(tile) = tiles_at[from].filter(|_| mark == PoseMark::Stored) {
            tiles.extend_from_slice(tile);
        }
        marks.push(mark);
    }
    BounceProbe {
        probe: probe.probe,
        marks,
        tiles,
    }
}

/// Apply [`value_sources`] to every light's direct entries: at a copied
/// pose, each probe's q is its source pose's, which compose reads there
/// (every pose stores q). Enclosure is each mover's posed solid's, by the
/// bake's enclosing rule, at the probe's own position; the read knots are
/// the mover's. An entry whose every q is then 255 gates nothing and is
/// dropped, as is a light left with none. Runs after the per-light memo,
/// which keeps the traced q, so the memo is unaffected and a warm build
/// equals a cold one.
pub(crate) fn copy_direct_edges(
    lights: &mut Vec<LightEntries>,
    movers: &PosedMovers,
    cells: &ProbeCells,
    layout: &ProbeGridLayout,
) {
    let by_id: HashMap<u32, &PosedMover> = movers
        .movers()
        .iter()
        .filter(|mover| mover.solids.len() > 2)
        .map(|mover| (mover.mover_id, mover))
        .collect();
    if by_id.is_empty() {
        return;
    }
    // Enclosure depends on the mover and the probe alone, so each
    // (mover, cell) finds its probes' sources once for every light gating it.
    let mut keys: Vec<(u32, u32)> = lights
        .iter()
        .flat_map(|light| &light.entries)
        .filter(|entry| by_id.contains_key(&entry.mover_id))
        .map(|entry| (entry.mover_id, entry.cell))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    // Per (mover, cell), each valid probe's sources in slot order, where
    // they move some pose's q.
    let by_cell: HashMap<(u32, u32), Vec<Option<Vec<usize>>>> = keys
        .into_par_iter()
        .map(|(mover_id, cell)| {
            let mover = by_id[&mover_id];
            let probes: Vec<Option<Vec<usize>>> = cells
                .valid_probes(cell as usize)
                .into_iter()
                .map(|probe| {
                    let point = vec3_from(layout.probe_positions[probe]);
                    let enclosed: Vec<bool> = mover
                        .solids
                        .iter()
                        .map(|solid| solid.encloses(point))
                        .collect();
                    let sources = value_sources(&enclosed, &mover.read_knots);
                    copies(&sources).then_some(sources)
                })
                .collect();
            ((mover_id, cell), probes)
        })
        .collect();
    for light in lights.iter_mut() {
        light.entries.par_iter_mut().for_each(|entry| {
            let Some(probes) = by_cell.get(&(entry.mover_id, entry.cell)) else {
                return;
            };
            let poses = entry.pose_count;
            for (slot, sources) in probes.iter().enumerate() {
                let Some(sources) = sources else {
                    continue;
                };
                debug_assert_eq!(sources.len(), poses);
                let q = &mut entry.visibility[slot * poses..(slot + 1) * poses];
                let traced = q.to_vec();
                for (pose, &from) in sources.iter().enumerate() {
                    q[pose] = traced[from];
                }
            }
        });
        light
            .entries
            .retain(|entry| entry.visibility.iter().any(|&q| q < u8::MAX));
    }
    lights.retain(|light| !light.entries.is_empty());
}

/// [`copy_bounce_edges`] over one job's states: enclosure by each posed
/// scene, at each probe's position in `layout`, and the job's read knots.
pub(crate) fn copy_job_bounce_edges(
    states: &mut MoverBounceStates,
    job: &MoverBounceJob,
    layout: &ProbeGridLayout,
) -> usize {
    copy_bounce_edges(states, &job.read_knots, |probe| {
        let point = vec3_from(layout.probe_positions[probe as usize]);
        job.scenes
            .iter()
            .map(|scene| scene.encloses(point))
            .collect()
    })
}

#[cfg(test)]
#[path = "edge_copy_tests.rs"]
mod tests;
