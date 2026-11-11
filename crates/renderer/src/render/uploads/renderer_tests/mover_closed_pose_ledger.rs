//! GPU half on compiled maps: the ledger's closed-pose host row holds every
//! stored pose's depth tile id 57 carries, mid poses included, plus one base
//! tile per probe carrying mover depth, whole-loaded and streamed alike, and
//! reads nothing once the level unloads.
//! See: context/lib/resource_management.md §8.2 (Depth) ·
//! context/lib/rendering_pipeline.md §4 (Mover state lighting, Residency and
//! ledger)
use super::real_level::RealLevel;
use postretro_level_format::mover_probe_channel::{PoseMark, is_terminus};
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;
use postretro_level_loader::ShStreamingMode;
use std::collections::BTreeSet;

/// Each map named by `MOVER_LEDGER_PRLS` (`:`-separated compiled PRLs with
/// id 50 and id 57, some mover storing a mid pose's depth tile), under
/// `off` and `sync-proof`: the host row `mover_closed_pose_depth_tiles` is
/// the decoded f32 pairs of every pose tile the depth records store, plus a
/// base tile per distinct probe, and the report is gone after unload.
#[test]
#[ignore = "requires MOVER_LEDGER_PRLS: `:`-separated compiled PRLs with id 50 and id 57 \
            (mover-setpiece)"]
fn every_stored_poses_closed_pose_tile_is_ledgered_on_a_compiled_map_and_leaves_with_it() {
    let paths = std::env::var("MOVER_LEDGER_PRLS").expect("set MOVER_LEDGER_PRLS");
    for path in paths.split(':').filter(|path| !path.is_empty()) {
        for mode in [ShStreamingMode::Off, ShStreamingMode::SyncProof] {
            let Some(mut level) = RealLevel::load(path, mode) else {
                return;
            };
            let channel = level.channel();
            let stored: usize = channel
                .depth_records
                .iter()
                .map(|record| record.stored_tile_count())
                .sum();
            let mid_stored: usize = channel
                .depth_records
                .iter()
                .map(|record| {
                    record
                        .marks
                        .iter()
                        .enumerate()
                        .filter(|&(pose, &mark)| {
                            !is_terminus(pose, record.marks.len()) && mark == PoseMark::Stored
                        })
                        .count()
                })
                .sum();
            let probes: BTreeSet<u32> = channel
                .depth_records
                .iter()
                .map(|record| record.probe_index)
                .collect();
            assert!(
                mid_stored > 0,
                "{path}: some mover stores a mid pose's depth tile"
            );
            let report = level.renderer.sh_residency_report().expect("an SH report");
            let host: Vec<_> = report
                .host_rows
                .iter()
                .filter(|row| row.name == "mover_closed_pose_depth_tiles")
                .collect();
            assert_eq!(host.len(), 1, "{path} {mode:?}");
            let expected = ((stored + probes.len()) * PROBE_DEPTH_TILE_TEXELS) as u64 * 8;
            eprintln!(
                "{path} {mode:?}: {} depth record(s) over {} probe(s), {stored} stored tile(s) \
                 ({mid_stored} at mid poses); host row {} B",
                channel.depth_records.len(),
                probes.len(),
                host[0].bytes,
            );
            assert_eq!(host[0].bytes, expected, "{path} {mode:?}");
            level.renderer.release_level_resources();
            assert!(
                level.renderer.sh_residency_report().is_none(),
                "{path} {mode:?}: nothing after unload"
            );
        }
    }
}
