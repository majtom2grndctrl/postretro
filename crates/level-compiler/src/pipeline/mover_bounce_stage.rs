//! The mover probe channel's bounce half at its pipeline seam: every
//! analysed mover posed at each of its poses, with its near-field domain,
//! and priced by the SH working-set gate before any SH work; its states
//! re-baked inside the SH base bake; then id 57's bounce and depth blocks
//! laid out, with the mover tiles reserved ahead of id 59's.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::HashSet;
use std::sync::Arc;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::geometry::GeometryResult;
use crate::map_data::{MapKinematicMover, MapLight};
use crate::mover_light_gate::MoverPoses;
use crate::mover_probe_channel::{
    BounceBlocks, CarriedDepth, DepthReservation, MoverBounceJob, MoverBounceRequest,
    MoverBounceStates, MoverChannelCost, NearFieldDomain, PosedMovers, PosedScene, ProbeCells,
    reserve_mover_tiles, sh_base_bounce_count,
};
use crate::partition::BspTree;
use crate::sh_bake::{ProbeDepthTiles, effective_tile_cap, probe_grid_layout_of};

/// What the bounce half reads before the SH gates.
pub(super) struct BouncePlanInputs<'a> {
    pub(super) poses: &'a MoverPoses,
    /// `poses`' analysed movers posed once for the whole compile: the
    /// bounce scenes and the direct half's occluders share these solids.
    pub(super) posed: &'a PosedMovers,
    pub(super) movers: &'a [MapKinematicMover],
    pub(super) geometry: &'a GeometryResult,
    pub(super) tree: &'a BspTree,
    pub(super) exterior_leaves: &'a HashSet<usize>,
    pub(super) probe_spacing: f32,
    pub(super) bounce_fill: f32,
    /// Every static and animated baked light, for the direct half's bound.
    pub(super) direct_lights: &'a [&'a MapLight],
    pub(super) uncompressed: bool,
    pub(super) tile_cap: u32,
}

/// The bounce half's request and what the SH gate prices for the channel.
pub(super) struct BounceHalfPlan {
    pub(super) request: MoverBounceRequest,
    pub(super) cost: MoverChannelCost,
}

/// Pose every analysed mover at each of its poses, its `light_bounces`
/// clamped to the SH base's bounce count, with its near-field domain and the
/// valid probes inside it, and price the channel: the scenes, the probe
/// grid's valid probes (classified now, with no ray), the domains and the
/// direct half's bound. No analysed mover or no probe grid, no job.
pub(super) fn plan_bounce_half(inputs: BouncePlanInputs<'_>) -> anyhow::Result<BounceHalfPlan> {
    let count = sh_base_bounce_count(inputs.bounce_fill);
    let layout = probe_grid_layout_of(
        inputs.geometry,
        inputs.tree,
        inputs.exterior_leaves,
        inputs.probe_spacing,
    );
    let posed = inputs.posed;
    let mut jobs = Vec::new();
    if !layout.is_empty() {
        for (analysed, solids) in inputs.poses.analysed.iter().zip(posed.movers()) {
            debug_assert_eq!(analysed.mover_id, solids.mover_id);
            let mover = inputs
                .movers
                .iter()
                .find(|mover| mover.mover_id == analysed.mover_id)
                .expect("resolved movers are map movers");
            debug_assert_eq!(analysed.knots.len(), solids.solids.len());
            let scenes = solids
                .solids
                .iter()
                .enumerate()
                .map(|(pose, solid)| {
                    PosedScene::new(Arc::clone(solid), pose).map_err(|error| {
                        anyhow::anyhow!("kinematic_mover `{}` posed sides: {error}", mover.name)
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let domain = NearFieldDomain::new(analysed, inputs.probe_spacing);
            jobs.push(MoverBounceJob {
                mover_id: mover.mover_id,
                label: analysed.label.clone(),
                bounces: mover
                    .light_bounces
                    .map_or(count, |bounces| bounces.min(count)),
                scenes,
                knots: analysed.knots.clone(),
                domain_probes: domain.valid_probes(&layout),
                domain,
            });
        }
    }
    let depth_cap = effective_tile_cap(inputs.tile_cap);
    let cost = MoverChannelCost::new(
        &jobs,
        &layout,
        inputs.bounce_fill,
        depth_cap,
        inputs.direct_lights,
        posed,
    );
    if !jobs.is_empty() {
        log::info!(
            "[MoverLight] probe channel bounce half: {} mover(s), {} re-baking bounce, {} \
             depth only; {} pose(s), {} posed patch(es), {} near-field probe(s)",
            jobs.len(),
            cost.radiance_movers,
            cost.movers - cost.radiance_movers,
            jobs.iter().map(|job| job.scenes.len()).sum::<usize>(),
            jobs.iter()
                .flat_map(|job| &job.scenes)
                .map(|scene| scene.patches.len())
                .sum::<usize>(),
            jobs.iter()
                .map(|job| job.domain_probes.len())
                .sum::<usize>(),
        );
    }
    let request = MoverBounceRequest {
        jobs,
        uncompressed: inputs.uncompressed,
        keep_every_tile: false,
        keep_poses: false,
        depth_cap,
    };
    Ok(BounceHalfPlan { request, cost })
}

/// The bounce half's blocks over id 34's grid, with the mover tiles reserved
/// ahead of id 59's: id 59 drops its lowest-ranked tiles to make room and
/// logs its counts once against the shrunk cap. Live tiles alone past the
/// range fail the build by name, with the count a lower bound once the bake
/// `stopped` short of a job or the range cut a mover sharing the range.
pub(super) fn finish_bounce_half(
    movers: &[MoverBounceStates],
    stopped: bool,
    base: &OctahedralShVolumeSection,
    depth_tiles: &mut ProbeDepthTiles,
    tile_cap: u32,
) -> anyhow::Result<(BounceBlocks, DepthReservation)> {
    let cells = ProbeCells::new(base.grid_dimensions, |probe| {
        base.probes[probe].validity != 0
    });
    let blocks = BounceBlocks::new(movers, &cells);
    let carried = CarriedDepth::new(blocks.live_tile_count(), movers, stopped);
    let reservation = reserve_mover_tiles(depth_tiles, base, carried, tile_cap)?;
    if reservation.live > 0 {
        log::info!(
            "[MoverLight] probe depth tiles: {} live tile(s) reserved for mover depth ahead of id \
             59's {}-tile cap; id 59 keeps at most {}, {} more dropped",
            reservation.live,
            effective_tile_cap(tile_cap),
            reservation.shrunk_cap,
            reservation.trimmed,
        );
    }
    depth_tiles.log(reservation.shrunk_cap);
    if !movers.is_empty() {
        blocks.log();
    }
    Ok((blocks, reservation))
}

/// The Build Summary line for the bounce and depth blocks.
pub(super) fn summary_line(blocks: &BounceBlocks, reservation: &DepthReservation) -> String {
    let counts = blocks.counts();
    format!(
        "Mover probe channel bounce: {} entr(y/ies) from {} mover(s), {} affected probe(s), \
         payload {} B; depth: {} record(s) from {} mover(s), {} live tile(s) of the \
         {}-tile range, {} CPU-held closed-pose tile(s)",
        counts.bounce_entries,
        counts.bounce_movers,
        counts.affected_probes,
        counts.bounce_payload_bytes,
        counts.depth_records,
        counts.depth_movers,
        reservation.live,
        reservation.shrunk_cap as usize + reservation.live,
        counts.closed_pose_tiles,
    )
}

#[cfg(test)]
#[path = "mover_bounce_stage_tests.rs"]
mod tests;
