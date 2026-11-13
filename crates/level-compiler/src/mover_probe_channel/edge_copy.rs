// Enclosure-run edge copy: the first and last pose of each run of poses that
// enclose a probe carry the adjoining clear pose's value state.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)
//
// Why (Amendment 2, "Enclosure-exit dip", owner 2026-11-12): an enclosed mid
// pose stores its own dark state (Ruling B) beside the dark depth tile.
// Leaving the enclosure, the mean-and-variance depth blend opens the probe's
// Chebyshev weight concavely while its value arrives linearly, so for about a
// frame the probe weighs in while still mostly dark and the band beside it
// dips, then rises. Copying the clear neighbour's value onto the edge pose
// holds the value at its lit level while depth opens, as blending across did
// before Ruling B; the poses inside the run keep their own dark state, which
// is what stops a probe layer inside a thin wall lighting the room behind it.
//
// IP: a stored state the bake chooses from geometry, as every pose's state
// is. Depth keeps the dark tile at every enclosed pose; the section gains no
// per-probe bit, and compose still blends between neighbouring stored
// states with every valid probe's weight untouched (`ip-372` levers 2, 7).

use std::collections::HashMap;

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, PoseMark, is_terminus,
};
use rayon::prelude::*;

use super::bake::MoverBounceJob;
use super::direct::{LightEntries, ProbeCells};
use super::posed_solid::PosedMovers;
use super::states::{BounceProbe, MoverBounceStates};
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// For each pose of a mover, in path order, the pose whose value state it
/// stores for one probe: itself, or the adjoining clear pose where it is the
/// first or last pose of a run of poses that enclose the probe (`enclosed`,
/// by the bake's enclosing rule). The run's first pose takes the clear pose
/// before it and its last pose the clear pose after it; a one-pose run takes
/// the nearer of the two in travel (`knots`, the poses' path fractions),
/// the later on a tie. A run end at a terminus keeps its own: a terminus
/// that encloses the probe reads the base. Poses inside a run keep their
/// own dark state.
///
/// Runs are found over the mover's knots as they stand, whatever placed
/// them, so a pose added further inside a run (a bounding pose) is an
/// interior pose and stays dark with no change here.
pub(crate) fn value_sources(enclosed: &[bool], knots: &[f32]) -> Vec<usize> {
    let count = enclosed.len();
    debug_assert_eq!(knots.len(), count, "one knot per pose");
    let mut sources: Vec<usize> = (0..count).collect();
    let mut at = 0;
    while at < count {
        if !enclosed[at] {
            at += 1;
            continue;
        }
        let first = at;
        while at < count && enclosed[at] {
            at += 1;
        }
        let last = at - 1;
        // A run is maximal, so a pose before a mid-pose first or after a
        // mid-pose last exists and is clear.
        if first == last {
            if !is_terminus(first, count) {
                let before = f64::from(knots[first]) - f64::from(knots[first - 1]);
                let after = f64::from(knots[first + 1]) - f64::from(knots[first]);
                sources[first] = if before < after { first - 1 } else { first + 1 };
            }
            continue;
        }
        if !is_terminus(first, count) {
            sources[first] = first - 1;
        }
        if !is_terminus(last, count) {
            sources[last] = last + 1;
        }
    }
    sources
}

/// Whether `sources` moves any pose's value.
fn copies(sources: &[usize]) -> bool {
    sources.iter().enumerate().any(|(pose, &from)| pose != from)
}

/// Apply [`value_sources`] to one mover's bounce states, before skipping
/// judges them, so skipping judges what compose reads. `enclosed(probe)`
/// gives each pose's enclosure of a probe; `knots` the poses' path
/// fractions. A copied pose takes what compose reads at its source pose: its
/// stored tile, or the base where the source reads it (a terminus storing
/// nothing reads the base, so the mid pose marks base). A probe left storing
/// nothing at any pose reads the base everywhere and leaves the states.
/// Depth is untouched: every enclosed pose keeps the dark tile. Returns the
/// (probe, pose) values copied.
pub(crate) fn copy_bounce_edges(
    states: &mut MoverBounceStates,
    knots: &[f32],
    enclosed: impl Fn(u32) -> Vec<bool> + Sync,
) -> usize {
    if states.pose_count <= 2 || states.bounce.is_empty() {
        return 0;
    }
    let copied: Vec<(Option<BounceProbe>, usize)> = std::mem::take(&mut states.bounce)
        .into_par_iter()
        .map(|probe| {
            let sources = value_sources(&enclosed(probe.probe), knots);
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
/// bake's enclosing rule, at the probe's own position; knots are the
/// mover's. An entry whose every q is then 255 gates nothing and is
/// dropped, as is a light left with none. Runs after the per-light memo, which keeps the traced q, so the
/// memo is unaffected and a warm build equals a cold one.
pub(crate) fn copy_direct_edges(
    lights: &mut Vec<LightEntries>,
    movers: &PosedMovers,
    cells: &ProbeCells,
    layout: &ProbeGridLayout,
) {
    let by_id: HashMap<u32, usize> = movers
        .movers()
        .iter()
        .enumerate()
        .filter(|(_, mover)| mover.solids.len() > 2)
        .map(|(at, mover)| (mover.mover_id, at))
        .collect();
    if by_id.is_empty() {
        return;
    }
    for light in lights.iter_mut() {
        light.entries.par_iter_mut().for_each(|entry| {
            let Some(&at) = by_id.get(&entry.mover_id) else {
                return;
            };
            let mover = &movers.movers()[at];
            let poses = entry.pose_count;
            debug_assert_eq!(poses, mover.solids.len());
            for (slot, probe) in cells
                .valid_probes(entry.cell as usize)
                .into_iter()
                .enumerate()
            {
                let point = vec3_from(layout.probe_positions[probe]);
                let enclosed: Vec<bool> = mover
                    .solids
                    .iter()
                    .map(|solid| solid.encloses(point))
                    .collect();
                let sources = value_sources(&enclosed, &mover.knots);
                if !copies(&sources) {
                    continue;
                }
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
/// scene, at each probe's position in `layout`, and the job's knots.
pub(crate) fn copy_job_bounce_edges(
    states: &mut MoverBounceStates,
    job: &MoverBounceJob,
    layout: &ProbeGridLayout,
) -> usize {
    copy_bounce_edges(states, &job.knots, |probe| {
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
