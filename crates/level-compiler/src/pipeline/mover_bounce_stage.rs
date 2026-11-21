//! The mover probe channel's bounce half at its pipeline seam: every
//! analysed mover posed at each of its poses, with its near-field domain,
//! and priced by the SH working-set gate before any SH work; each mover
//! re-baked and finished into id 57's bounce and depth blocks inside the SH
//! base bake; then the mover tiles reserved ahead of id 59's.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::sync::Arc;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::bake_fidelity::BakeMode;
use crate::map_data::{MapKinematicMover, MapLight};
use crate::mover_light_gate::MoverPoses;
use crate::mover_probe_channel::{
    BounceBlocks, CarriedDepth, DepthReservation, MoverBounceJob, MoverBounceRequest,
    MoverChannelCost, NearFieldDomain, PosedMovers, PosedScene, reserve_mover_tiles,
    sh_base_bounce_count,
};
use crate::sh_bake::{ProbeDepthTiles, ProbeGridLayout, effective_tile_cap};

/// What the bounce half reads before the SH gates.
pub(super) struct BouncePlanInputs<'a> {
    pub(super) poses: &'a MoverPoses,
    /// `poses`' analysed movers posed once for the whole compile: the
    /// bounce scenes and the direct half's occluders share these solids.
    pub(super) posed: &'a PosedMovers,
    pub(super) movers: &'a [MapKinematicMover],
    /// The probe grid, laid out once for placement and this half.
    pub(super) layout: &'a ProbeGridLayout,
    pub(super) probe_spacing: f32,
    pub(super) bounce_fill: f32,
    /// Every static and animated baked light, for the direct half's bound.
    pub(super) direct_lights: &'a [&'a MapLight],
    pub(super) uncompressed: bool,
    pub(super) tile_cap: u32,
    /// `--mover-payload-max-size`: the most the bounce and depth blocks may
    /// hold.
    pub(super) payload_cap: u64,
    /// What the build bakes, from the flags: its fidelity caps each mover's
    /// posed bounces.
    pub(super) mode: BakeMode,
}

/// The bounce half's request and what the SH gate prices for the channel.
pub(super) struct BounceHalfPlan {
    pub(super) request: MoverBounceRequest,
    pub(super) cost: MoverChannelCost,
}

/// Pose every analysed mover at each of its poses, its `light_bounces`
/// clamped to the SH base's bounce count and capped by the build's fidelity
/// (`low` bakes at most 1), with its near-field domain and the
/// valid probes inside it, and price the channel: the scenes, the probe
/// grid's valid probes (classified now, with no ray), the domains and the
/// direct half's bound. No analysed mover or no probe grid, no job.
pub(super) fn plan_bounce_half(inputs: BouncePlanInputs<'_>) -> anyhow::Result<BounceHalfPlan> {
    let count = sh_base_bounce_count(inputs.bounce_fill);
    let layout = inputs.layout;
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
            let high_bounces = mover
                .light_bounces
                .map_or(count, |bounces| bounces.min(count));
            jobs.push(MoverBounceJob {
                mover_id: mover.mover_id,
                label: analysed.label.clone(),
                bounces: inputs.mode.fidelity().bounces(high_bounces),
                high_bounces,
                scenes,
                read_knots: solids.read_knots.clone(),
                domain_probes: domain.valid_probes(layout),
                domain,
            });
        }
    }
    let depth_cap = effective_tile_cap(inputs.tile_cap);
    let cost = MoverChannelCost::new(
        &jobs,
        layout,
        inputs.bounce_fill,
        depth_cap,
        inputs.payload_cap,
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
        payload_cap: inputs.payload_cap,
        #[cfg(test)]
        keep_states: false,
        #[cfg(test)]
        fail_read_back: Vec::new(),
    };
    Ok(BounceHalfPlan { request, cost })
}

/// Reserve the mover tiles ahead of id 59's for `blocks`, every mover's
/// finished blocks: id 59 drops its lowest-ranked tiles to make room,
/// keeping those of the probes still carrying mover depth, the bases their
/// skips read, and logs its counts once against the shrunk cap. Runs once,
/// after the last mover. A bake past the range has already failed, before
/// its mover skipped, and skipping only removes records, so every mover's
/// depth is whole here and the count exact. The reservation's own refusal
/// of live tiles past the range is a backstop that earlier refusal makes
/// unreachable. Where the range cannot hold the carrying probes' own id-59
/// tiles beside the live tiles, the build warns and drops the lowest-ranked
/// of them. `movers`: some mover finished, so the blocks log their line.
pub(super) fn finish_bounce_half(
    blocks: &BounceBlocks,
    movers: bool,
    base: &OctahedralShVolumeSection,
    depth_tiles: &mut ProbeDepthTiles,
    tile_cap: u32,
) -> anyhow::Result<DepthReservation> {
    let carrying = blocks.live_probes();
    let carried = CarriedDepth::exact(carrying.len());
    let reservation = reserve_mover_tiles(depth_tiles, base, carried, &carrying, tile_cap)?;
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
    if reservation.carrying_trimmed > 0 {
        log::warn!(
            "[MoverLight] probe depth tiles: the {}-tile range leaves id 59 {} tile(s) beside the \
             live tiles, too few for the {} probe(s) carrying mover depth that hold one; {} of \
             them drop, so those probes' live tiles blend through their averaged moments rather \
             than the tile their mover states were judged against. Raise \
             `--probe-depth-tile-cap`",
            effective_tile_cap(tile_cap),
            reservation.shrunk_cap,
            reservation.shrunk_cap as usize + reservation.carrying_trimmed,
            reservation.carrying_trimmed,
        );
    }
    depth_tiles.log(reservation.shrunk_cap);
    if movers {
        blocks.log();
    }
    Ok(reservation)
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
pub(super) mod tests;
