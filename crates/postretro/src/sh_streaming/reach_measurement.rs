//! Measurement harness: SH and lightmap mandatory demand per camera cell under the id-51 reach.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency, Lightmap cell-block residency)

// The go/no-go check for an id-51 mandatory tier: whether its worst cell,
// with fixed metadata, fits the SH floor. For every camera cell (not solid,
// not exterior), the SH mandatory tier the id-51 reach would hold: the
// clusters of the cell's id-51 entries with lead <= L (lead 0 is the cell's
// own dilated visible set, so Visible is inside it), plus pinned clusters,
// closed over baked owners. Bytes are the controller's logical charge per
// cluster (`requested_resident_bytes`), the figure its 256 MiB floor
// compares after fixed metadata and whole-resident scatter. Beside it, the
// lightmap's mandatory blocks for the same cell and L, as its controller
// demands them: the blocks of those cells plus pinned blocks. A block is a
// pair when it carries an id-42 shadowmask half. Pool layers are the 2048²
// layers a fresh shelf placement of only that set needs (the shared
// level-format placement): a floor under the runtime pool, which also holds
// band blocks and placement history.
//
// Id 51 already carries teleporter arrival cells as zero-length routes, so
// comparing a PRL against its teleporter-free twin isolates what arrival
// origins add. These are bake figures, not runtime VRAM.
//
// Run from the workspace root, once per PRL:
//
//   POSTRETRO_SH_REACH_PRL=$PWD/content/dev/maps/campaign-test.prl \
//     cargo test -p postretro --bin postretro sh_reach_mandatory_bytes_from_prl \
//     -- --ignored --nocapture
//
// Optional: `POSTRETRO_SH_REACH_LEADS` = comma-separated metres (default
// `16,32`; the first is the gate's L). `POSTRETRO_SH_REACH_CSV` = a path to
// write every camera cell's row at every lead; unset prints the rows.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use glam::Vec3;
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::lightmap_pool::place_all_resident;
use postretro_level_loader::LevelWorld;

use super::topology::PlannerTopology;
use crate::lightmap_streaming::block_map::LevelBlockMap;
use crate::lightmap_streaming::source::{LightmapBlockSource, ManifestBlockSource};
use crate::streaming::cell_demand::{DEFAULT_LEAD_METRES, LEAD_UNITS_PER_METRE};
use crate::streaming::cluster_hints::{ClusterHints, decode_level_hints};

const PRL_ENV: &str = "POSTRETRO_SH_REACH_PRL";
const LEADS_ENV: &str = "POSTRETRO_SH_REACH_LEADS";
const CSV_ENV: &str = "POSTRETRO_SH_REACH_CSV";
const GATE_FLOOR_BYTES: u64 = super::budget::DEFAULT_GPU_FLOOR_BYTES;
const MIB: f64 = 1024.0 * 1024.0;
const CSV_HEADER: &str = "lead_m,camera_cell,center_x,center_y,center_z,mandatory_cells,\
     sh_clusters,sh_bytes,lightmap_blocks,lightmap_pairs,lightmap_half_bytes,\
     shadowmask_half_bytes,lightmap_pair_bytes,lightmap_pool_layers";

/// The level's streamed blocks: the controller's block map plus each block's
/// unaligned extent for the shared placement.
struct Blocks {
    map: LevelBlockMap,
    extents: Vec<(u32, u32)>,
    alignment: u32,
}

/// One camera cell's mandatory set at one lead.
#[derive(Debug, Clone, Copy)]
struct CellSample {
    camera_cell: u32,
    center: Vec3,
    /// Id-51 entries within the lead: the cells whose units are mandatory.
    mandatory_cells: u32,
    sh_clusters: u32,
    sh_bytes: u64,
    lightmap_blocks: u32,
    /// Blocks carrying both an id-22 and an id-42 half.
    lightmap_pairs: u32,
    lightmap_half_bytes: u64,
    shadowmask_half_bytes: u64,
    lightmap_pair_bytes: u64,
    lightmap_pool_layers: u32,
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
    blocks: Option<&Blocks>,
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

    let data = &world.cells[camera_cell as usize];
    let mut sample = CellSample {
        camera_cell,
        center: (data.bounds_min + data.bounds_max) * 0.5,
        mandatory_cells: within.len() as u32,
        sh_clusters: clusters.len() as u32,
        sh_bytes,
        lightmap_blocks: 0,
        lightmap_pairs: 0,
        lightmap_half_bytes: 0,
        shadowmask_half_bytes: 0,
        lightmap_pair_bytes: 0,
        lightmap_pool_layers: 0,
    };
    if let Some(blocks) = blocks {
        let mut demanded: BTreeSet<u32> = within
            .iter()
            .flat_map(|&cell| blocks.map.blocks_of_cell(cell))
            .collect();
        demanded.extend(blocks.map.pinned_blocks().iter().copied());
        for &block in &demanded {
            let facts = blocks.map.facts(block);
            sample.lightmap_pairs += u32::from(facts.shadowmask_bytes > 0);
            sample.lightmap_half_bytes += facts.lightmap_bytes;
            sample.shadowmask_half_bytes += facts.shadowmask_bytes;
            sample.lightmap_pair_bytes += facts.pair_bytes();
        }
        sample.lightmap_blocks = demanded.len() as u32;
        let extents: Vec<(u32, u32)> = demanded
            .iter()
            .map(|&block| blocks.extents[block as usize])
            .collect();
        sample.lightmap_pool_layers =
            place_all_resident(&extents, blocks.alignment, LIGHTMAP_POOL_LAYER_EDGE)
                .expect("loader-validated block extents place")
                .layer_count;
    }
    sample
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

fn print_bytes(label: &str, samples: &[CellSample], field: fn(&CellSample) -> u64) {
    let (median, p95, max) = spread(samples.iter().map(field).collect());
    println!(
        "{label:<26} median {:.3} MiB, p95 {:.3} MiB, max {:.3} MiB",
        mib(median),
        mib(p95),
        mib(max)
    );
}

fn print_counts(label: &str, samples: &[CellSample], field: fn(&CellSample) -> u64) {
    let (median, p95, max) = spread(samples.iter().map(field).collect());
    println!("{label:<26} median {median}, p95 {p95}, max {max}");
}

fn csv_row(out: &mut String, metres: f32, s: &CellSample) {
    writeln!(
        out,
        "{metres},{},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{},{}",
        s.camera_cell,
        s.center.x,
        s.center.y,
        s.center.z,
        s.mandatory_cells,
        s.sh_clusters,
        s.sh_bytes,
        s.lightmap_blocks,
        s.lightmap_pairs,
        s.lightmap_half_bytes,
        s.shadowmask_half_bytes,
        s.lightmap_pair_bytes,
        s.lightmap_pool_layers,
    )
    .expect("write to String");
}

fn report_lead(
    world: &LevelWorld,
    topology: &PlannerTopology,
    blocks: Option<&Blocks>,
    metres: f32,
    camera_cells: &[u32],
    rows: &mut String,
) -> u64 {
    let lead = (metres * LEAD_UNITS_PER_METRE as f32).round() as u32;
    let samples: Vec<CellSample> = camera_cells
        .iter()
        .map(|&cell| sample_cell(world, topology, blocks, cell, lead))
        .collect();
    for sample in &samples {
        csv_row(rows, metres, sample);
    }
    let worst = samples
        .iter()
        .max_by_key(|sample| sample.sh_bytes)
        .copied()
        .expect("camera cells");
    println!("\n## L = {metres} m ({} camera cells)", samples.len());
    print_counts("mandatory cells", &samples, |s| {
        u64::from(s.mandatory_cells)
    });
    print_bytes("SH mandatory bytes", &samples, |s| s.sh_bytes);
    print_counts("SH mandatory clusters", &samples, |s| {
        u64::from(s.sh_clusters)
    });
    if blocks.is_some() {
        print_bytes("lightmap mandatory bytes", &samples, |s| {
            s.lightmap_pair_bytes
        });
        print_bytes("  id-22 half bytes", &samples, |s| s.lightmap_half_bytes);
        print_bytes("  id-42 half bytes", &samples, |s| s.shadowmask_half_bytes);
        print_counts("lightmap blocks", &samples, |s| {
            u64::from(s.lightmap_blocks)
        });
        print_counts("lightmap pairs", &samples, |s| u64::from(s.lightmap_pairs));
        print_counts("lightmap pool layers", &samples, |s| {
            u64::from(s.lightmap_pool_layers)
        });
    } else {
        println!("lightmap: not streamed in this PRL");
    }
    println!(
        "worst SH cell {}: {} clusters, {:.3} MiB SH; lightmap {} blocks, {:.3} MiB, {} layers",
        worst.camera_cell,
        worst.sh_clusters,
        mib(worst.sh_bytes),
        worst.lightmap_blocks,
        mib(worst.lightmap_pair_bytes),
        worst.lightmap_pool_layers,
    );
    worst.sh_bytes
}

fn load_blocks(world: &LevelWorld, hints: &ClusterHints) -> Option<Blocks> {
    let manifest = world.lightmap_stream_manifest()?;
    let source = ManifestBlockSource::new(Arc::clone(manifest));
    let map = LevelBlockMap::build(&source, world.cells.len(), Some(hints)).expect("block map");
    let extents = (0..source.block_count())
        .map(|block| {
            let summary = source.block_summary(block).expect("indexed block");
            (u32::from(summary.width), u32::from(summary.height))
        })
        .collect();
    Some(Blocks {
        map,
        extents,
        alignment: source.block_alignment(),
    })
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
    let blocks = load_blocks(&world, &hints);

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
        "PRL: {path}\n{} cells ({} camera), {} clusters ({:.3} MiB whole map), \
         pinned + owners {} clusters ({:.3} MiB), id-51 max lead {:.1} m, {} entries",
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
        residency_set.entries.len(),
    );
    if let Some(blocks) = &blocks {
        let whole: u64 = (0..blocks.map.block_count() as u32)
            .map(|block| blocks.map.facts(block).pair_bytes())
            .sum();
        println!(
            "lightmap: {} blocks ({:.3} MiB whole map), {} pinned blocks",
            blocks.map.block_count(),
            mib(whole),
            blocks.map.pinned_blocks().len(),
        );
    }

    let mut rows = format!("{CSV_HEADER}\n");
    let mut gate_max = 0;
    for (index, &metres) in leads.iter().enumerate() {
        let max = report_lead(
            &world,
            &topology,
            blocks.as_ref(),
            metres,
            &camera_cells,
            &mut rows,
        );
        if index == 0 {
            gate_max = max;
        }
    }
    println!(
        "\nGate (L = {} m): worst-cell SH mandatory {:.3} MiB of the {:.0} MiB floor, \
         before fixed metadata and whole-resident scatter",
        leads[0],
        mib(gate_max),
        mib(GATE_FLOOR_BYTES)
    );
    match std::env::var(CSV_ENV) {
        Ok(csv) => {
            std::fs::write(&csv, rows).expect("write per-cell CSV");
            println!("per-cell rows: {csv}");
        }
        Err(_) => print!("\n## Per-cell rows\n{rows}"),
    }
}
