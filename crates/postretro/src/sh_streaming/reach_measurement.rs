//! Measurement harness: SH mandatory bytes per camera cell under the id-51 reach.
//! See: context/plans/in-progress/sh-streaming--reveal-gate-and-warm-horizon (task 1)

// The brief's go/no-go gate. For every camera cell (not solid, not exterior),
// the SH mandatory tier the id-51 reach would hold: the clusters of the
// cell's id-51 entries with lead <= L (lead 0 is the cell's own dilated
// visible set, so Visible is inside it), plus pinned clusters, closed over
// baked owners. Bytes are the controller's logical charge per cluster
// (`requested_resident_bytes`), the figure its 256 MiB floor compares after
// fixed metadata and whole-resident scatter. Beside it, the lightmap's
// mandatory pair bytes for the same cell and L, as its controller demands.
//
// Run from the workspace root, once per PRL:
//
//   POSTRETRO_SH_REACH_PRL=$PWD/content/dev/maps/campaign-test.prl \
//     cargo test -p postretro --bin postretro sh_reach_mandatory_bytes_from_prl \
//     -- --ignored --nocapture
//
// Optional: `POSTRETRO_SH_REACH_LEADS` = comma-separated metres (default
// `16,32`; the first is the gate's L).

use std::collections::BTreeSet;
use std::sync::Arc;

use postretro_level_loader::LevelWorld;

use super::topology::PlannerTopology;
use crate::lightmap_streaming::block_map::LevelBlockMap;
use crate::lightmap_streaming::levers::{DEFAULT_LEAD_METRES, LEAD_UNITS_PER_METRE};
use crate::lightmap_streaming::source::ManifestBlockSource;
use crate::streaming::cluster_hints::decode_level_hints;

const PRL_ENV: &str = "POSTRETRO_SH_REACH_PRL";
const LEADS_ENV: &str = "POSTRETRO_SH_REACH_LEADS";
const GATE_FLOOR_BYTES: u64 = super::budget::DEFAULT_GPU_FLOOR_BYTES;
const MIB: f64 = 1024.0 * 1024.0;

/// One camera cell's mandatory set at one lead.
#[derive(Debug, Clone, Copy)]
struct CellSample {
    camera_cell: u32,
    sh_clusters: u32,
    sh_bytes: u64,
    lightmap_blocks: u32,
    lightmap_bytes: u64,
}

/// Every owner a cluster set needs, transitively: the closure the controller
/// adds for Visible and Pinned targets (`close_owner_targets`).
fn close_owners(topology: &PlannerTopology, clusters: &mut BTreeSet<u32>) {
    let mut stack: Vec<u32> = clusters.iter().copied().collect();
    while let Some(cluster) = stack.pop() {
        for &owner in &topology.owners[cluster as usize] {
            if clusters.insert(owner) {
                stack.push(owner);
            }
        }
    }
}

fn sample_cell(
    world: &LevelWorld,
    topology: &PlannerTopology,
    blocks: Option<&LevelBlockMap>,
    camera_cell: u32,
    lead: u32,
) -> CellSample {
    let residency_set = world
        .cell_residency_set
        .as_ref()
        .expect("checked before sampling");
    let hints = &topology.hints;
    let within: Vec<u32> = residency_set
        .entries_for(camera_cell as usize)
        .iter()
        .filter(|entry| entry.lead <= lead)
        .map(|entry| entry.cell_id)
        .collect();

    let mut clusters: BTreeSet<u32> = within
        .iter()
        .map(|&cell| hints.cell_to_cluster[cell as usize])
        .collect();
    clusters.extend(hints.pinned.iter().copied());
    close_owners(topology, &mut clusters);
    let sh_bytes = clusters
        .iter()
        .map(|&cluster| topology.requested_resident_bytes[cluster as usize])
        .sum();

    let (lightmap_blocks, lightmap_bytes) = blocks.map_or((0, 0), |map| {
        let mut demanded: BTreeSet<u32> = within
            .iter()
            .flat_map(|&cell| map.blocks_of_cell(cell))
            .collect();
        demanded.extend(map.pinned_blocks().iter().copied());
        let bytes = demanded
            .iter()
            .map(|&block| map.facts(block).pair_bytes())
            .sum();
        (demanded.len() as u32, bytes)
    });

    CellSample {
        camera_cell,
        sh_clusters: clusters.len() as u32,
        sh_bytes,
        lightmap_blocks,
        lightmap_bytes,
    }
}

/// (median, p95, max) by nearest rank.
fn spread(mut values: Vec<u64>) -> (u64, u64, u64) {
    values.sort_unstable();
    let rank =
        |q: f64| values[((values.len() as f64 * q).ceil() as usize).clamp(1, values.len()) - 1];
    (rank(0.5), rank(0.95), *values.last().expect("non-empty"))
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB
}

fn report_lead(
    world: &LevelWorld,
    topology: &PlannerTopology,
    blocks: Option<&LevelBlockMap>,
    metres: f32,
    camera_cells: &[u32],
) -> u64 {
    let lead = (metres * LEAD_UNITS_PER_METRE as f32).round() as u32;
    let samples: Vec<CellSample> = camera_cells
        .iter()
        .map(|&cell| sample_cell(world, topology, blocks, cell, lead))
        .collect();
    let worst = samples
        .iter()
        .max_by_key(|sample| sample.sh_bytes)
        .copied()
        .expect("camera cells");
    let (sh_med, sh_p95, sh_max) = spread(samples.iter().map(|s| s.sh_bytes).collect());
    let (cl_med, cl_p95, cl_max) = spread(samples.iter().map(|s| s.sh_clusters as u64).collect());
    let (lm_med, lm_p95, lm_max) = spread(samples.iter().map(|s| s.lightmap_bytes).collect());
    let (bl_med, bl_p95, bl_max) =
        spread(samples.iter().map(|s| s.lightmap_blocks as u64).collect());
    println!("\n## L = {metres} m ({} camera cells)", samples.len());
    println!(
        "SH mandatory bytes   median {:.1} MiB, p95 {:.1} MiB, max {:.1} MiB",
        mib(sh_med),
        mib(sh_p95),
        mib(sh_max)
    );
    println!("SH mandatory clusters median {cl_med}, p95 {cl_p95}, max {cl_max}");
    if blocks.is_some() {
        println!(
            "lightmap mandatory   median {:.1} MiB, p95 {:.1} MiB, max {:.1} MiB",
            mib(lm_med),
            mib(lm_p95),
            mib(lm_max)
        );
        println!("lightmap blocks      median {bl_med}, p95 {bl_p95}, max {bl_max}");
    } else {
        println!("lightmap: not streamed in this PRL");
    }
    println!(
        "worst SH cell {}: {} clusters, {:.1} MiB SH; lightmap {} blocks, {:.1} MiB",
        worst.camera_cell,
        worst.sh_clusters,
        mib(worst.sh_bytes),
        worst.lightmap_blocks,
        mib(worst.lightmap_bytes)
    );
    sh_max
}

#[test]
#[ignore = "measurement helper; set POSTRETRO_SH_REACH_PRL"]
fn sh_reach_mandatory_bytes_from_prl() {
    let path = std::env::var(PRL_ENV).unwrap_or_else(|_| panic!("{PRL_ENV} must name a PRL"));
    let leads: Vec<f32> = std::env::var(LEADS_ENV)
        .unwrap_or_else(|_| format!("{DEFAULT_LEAD_METRES},32"))
        .split(',')
        .map(|value| value.trim().parse().expect("lead metres"))
        .collect();

    let world = postretro_level_loader::load_prl(&path).expect("load PRL");
    let residency_set = world
        .cell_residency_set
        .as_ref()
        .expect("the PRL carries no id 51: rebake it");
    let manifest = world
        .sh_stream_manifest()
        .expect("the PRL does not stream SH: needs a valid id-49/id-50 pair");
    let hints = decode_level_hints(world.cluster_directory())
        .expect("id-49 hints")
        .expect("id 49");
    let topology =
        PlannerTopology::from_manifest(manifest, Arc::clone(&hints)).expect("planner topology");
    let blocks = world.lightmap_stream_manifest().map(|manifest| {
        LevelBlockMap::build(
            &ManifestBlockSource::new(Arc::clone(manifest)),
            world.cells.len(),
            Some(&hints),
        )
        .expect("block map")
    });

    let camera_cells: Vec<u32> = (0..world.cells.len() as u32)
        .filter(|&cell| {
            let data = &world.cells[cell as usize];
            !data.is_solid && !data.is_exterior
        })
        .collect();
    let whole_map: u64 = topology.requested_resident_bytes.iter().sum();
    let pinned: BTreeSet<u32> = {
        let mut set = hints.pinned.clone();
        close_owners(&topology, &mut set);
        set
    };
    println!(
        "PRL: {path}\n{} cells ({} camera), {} clusters ({:.1} MiB whole map), \
         pinned + owners {} clusters ({:.1} MiB), id-51 max lead {:.1} m",
        world.cells.len(),
        camera_cells.len(),
        topology.cluster_count(),
        mib(whole_map),
        pinned.len(),
        mib(pinned
            .iter()
            .map(|&c| topology.requested_resident_bytes[c as usize])
            .sum()),
        residency_set.max_lead as f32 / LEAD_UNITS_PER_METRE as f32,
    );

    let mut gate_max = 0;
    for (index, &metres) in leads.iter().enumerate() {
        let max = report_lead(&world, &topology, blocks.as_ref(), metres, &camera_cells);
        if index == 0 {
            gate_max = max;
        }
    }
    println!(
        "\nGate (L = {} m): worst-cell SH mandatory {:.1} MiB of the {:.0} MiB floor, \
         before fixed metadata and whole-resident scatter",
        leads[0],
        mib(gate_max),
        mib(GATE_FLOOR_BYTES)
    );
}
