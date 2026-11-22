// The bounce-and-depth bake over every mover the SH base bake re-bakes: its
// jobs, what their poses share, and each mover finished in request order,
// reduced, judged against the tile range, skipped and encoded, before the
// next runs.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::Instant;

use postretro_level_format::mover_probe_channel::Knot;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::absent_bounces::AbsentLaterBounces;
use super::blocks::{BounceBlocks, MoverPayloadCapExceeded};
use super::bounce::{light_terms, posed_later_progress_units};
use super::depth::{DepthUnion, MoverAbsentDepth, MoverTileRangeExceeded};
use super::direct::ProbeCells;
use super::edge_copy::copy_job_bounce_edges;
use super::gather::valid_probes;
use super::near_field::NearFieldDomain;
use super::pose_bake::{PoseBaker, PosedBounce, ResolvedPoses};
use super::pose_memo::{PoseKind, PoseMemo};
use super::posed_scene::PosedScene;
use super::skip::{SkipBars, SkipCounts, skip_redundant_mid_poses};
use super::states::{MoverBounceStates, MoverPoseStates, PoseState, reduce_poses};
use crate::bake_control::BakeControl;
use crate::cache::CacheKey;
use crate::map_data::MapLight;
use crate::sh_bake::{ProbeDepthTiles, ProbeGridLayout, ShBakeCtx};
use crate::surface_radiance::{
    LATER_BOUNCE_COUNT, LayerPlan, PatchRadiance, PatchReachIndex, PatchSet,
};

/// The bounces id 34 bakes at `fill`: bounce one, and the later bounces
/// unless the fill is 0. A mover's `light_bounces` above it bakes it.
pub(crate) fn sh_base_bounce_count(fill: f32) -> u32 {
    1 + if fill == 0.0 { 0 } else { LATER_BOUNCE_COUNT }
}

/// One mover's poses to re-bake.
pub(crate) struct MoverBounceJob {
    pub(crate) mover_id: u32,
    pub(crate) label: String,
    /// Bounces a pose this build bakes re-bakes posed, bounce one first:
    /// `high_bounces` at the build's fidelity
    /// ([`crate::bake_fidelity::BakeFidelity::bounces`]). Later bounces past
    /// them are id 34's own. 0 bakes no bounce and traces only the poses'
    /// probe depth.
    pub(crate) bounces: u32,
    /// The mover's own count, its `light_bounces` clamped to the SH base's:
    /// what a `high` build bakes, and the memo entry a `low` build serves
    /// before baking. Equals `bounces` at high, and wherever it is at most 1.
    pub(crate) high_bounces: u32,
    /// The mover posed at each pose, in path order: the first and last are
    /// its termini.
    pub(crate) scenes: Vec<PosedScene>,
    /// The knots compose reads, as the posed mover holds them
    /// ([`super::posed_solid::PosedMover::read_knots`]): where enclosure
    /// runs and their clear neighbours are found
    /// ([`super::edge_copy::value_sources`]).
    pub(crate) read_knots: Vec<Knot>,
    /// What a mid pose gathers over: the swept brushes widened by the
    /// gather's lift and one probe spacing. Its probes take the mid pose's
    /// gather, its static patches the mid pose's posed later bounces.
    pub(crate) domain: NearFieldDomain,
    /// The valid probes inside `domain`, ascending by dense index.
    pub(crate) domain_probes: Vec<u32>,
}

impl MoverBounceJob {
    /// Whether pose `pose` is a terminus: the first or the last.
    pub(super) fn is_terminus(&self, pose: usize) -> bool {
        pose == 0 || pose + 1 == self.scenes.len()
    }

    /// Whether the mover has a pose between its termini.
    pub(super) fn has_mid_poses(&self) -> bool {
        self.scenes.len() > 2
    }

    /// Progress units the bake of pose `pose` at `bounces` posed bounces
    /// advances over `valid_probes` valid probes, `patches` static patches
    /// and `domain_patches` of them inside the near-field domain
    /// ([`Self::domain_patch_count`]) at `fill`: at a terminus, one per
    /// valid probe (the posed gather, or the depth-only trace) and one per
    /// patch chunk of each posed later bounce over every static patch; at a
    /// mid pose, one per domain probe (its gather, or its depth-only trace)
    /// and one per patch chunk of each posed later bounce over the static
    /// patches in its domain. Each posed later bounce also counts the pose's
    /// own patch chunks. Posed bounce one counts none. A memo hit advances
    /// them at once.
    pub(crate) fn pose_progress_units(
        &self,
        pose: usize,
        bounces: u32,
        valid_probes: usize,
        patches: usize,
        domain_patches: usize,
        fill: f32,
    ) -> usize {
        let terminus = self.is_terminus(pose);
        let gathered = if terminus {
            valid_probes
        } else {
            self.domain_probes.len()
        };
        if bounces == 0 || fill == 0.0 {
            return gathered;
        }
        let world = if terminus { patches } else { domain_patches };
        gathered + posed_later_progress_units(bounces - 1, world, self.scenes[pose].patches.len())
    }

    /// The static patches inside the near-field domain a mid pose's later
    /// bounces trace, counted only where some mid pose may trace one: at
    /// the mover's own count, which a pose a `high` entry serves holds.
    pub(crate) fn domain_patch_count(&self, patches: &PatchSet, fill: f32) -> usize {
        if fill != 0.0 && self.high_bounces > 1 && self.has_mid_poses() {
            self.domain.static_patches(patches).len()
        } else {
            0
        }
    }

    /// Progress units every pose of the job advances at the build's count
    /// ([`Self::pose_progress_units`]), before any memo lookup.
    pub(crate) fn progress_units(
        &self,
        valid_probes: usize,
        patches: &PatchSet,
        fill: f32,
    ) -> usize {
        let domain_patches = self.domain_patch_count(patches, fill);
        (0..self.scenes.len())
            .map(|pose| {
                self.pose_progress_units(
                    pose,
                    self.bounces,
                    valid_probes,
                    patches.len(),
                    domain_patches,
                    fill,
                )
            })
            .sum()
    }
}

/// The movers one SH base bake re-bakes, and how.
#[derive(Default)]
pub(crate) struct MoverBounceRequest {
    pub(crate) jobs: Vec<MoverBounceJob>,
    /// The PRL stores id 34 as f16, so the bar is exact equality.
    pub(crate) uncompressed: bool,
    /// The full re-bake (test only; production leaves both off):
    /// keep every valid probe's posed tile, hand each pose back, and reduce
    /// no mover.
    pub(crate) keep_every_tile: bool,
    pub(crate) keep_poses: bool,
    /// The tile range mover depth may fill: once the movers finished so far
    /// carry depth at more probes, judged on each mover's unskipped depth
    /// as it joins ([`super::depth::DepthUnion::verdict`]), the bake stops
    /// and the build fails by name before that mover skips. No mover keeps
    /// more than one depth record past it ([`super::states::reduce_poses`]),
    /// and a mover past it has none of its poses memoized.
    pub(crate) depth_cap: u32,
    /// `--mover-payload-max-size`: the most the bounce and depth blocks may
    /// hold. Checked as each mover's blocks join, after its skips; past it
    /// the bake stops and the build fails by name.
    pub(crate) payload_cap: u64,
    /// Test only: hand back each finished mover's reduced states, before
    /// its mid poses skip, detached from the ledger.
    #[cfg(test)]
    pub(crate) keep_states: bool,
    /// Test only: (mover, pose) pairs whose memo read fails at the slot
    /// after the lookup found them, as a cache entry corrupted or evicted
    /// between the two would.
    #[cfg(test)]
    pub(crate) fail_read_back: Vec<(u32, usize)>,
}

impl MoverBounceRequest {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

/// How one pose of a job resolves.
pub(crate) struct PoseSlot {
    /// Its memo key, when a cache is open: the entry it was served from, or
    /// the one a baked pose is written to.
    pub(crate) key: Option<CacheKey>,
    /// The posed bounces its state holds: the job's `high_bounces` when a
    /// `high` entry served it, else the job's `bounces`, at which it bakes.
    pub(crate) bounces: u32,
    /// Its memo held it at lookup: the bake reads it there again, and bakes
    /// it only when that read fails. A held pose advances its progress when
    /// it reads back, so one that fails to read back advances only as it
    /// bakes.
    pub(crate) held: bool,
}

/// One production job in request order, as the bake meets it: each pose
/// reads back from the memo where `poses` holds it, else bakes, and the
/// mover finishes.
pub(crate) struct MoverSlot<'a> {
    pub(crate) job: &'a MoverBounceJob,
    pub(crate) poses: &'a [PoseSlot],
}

/// Why the bounce bake stopped short: the build fails by this name.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MoverBounceRefusal {
    #[error(transparent)]
    TileRange(#[from] MoverTileRangeExceeded),
    #[error(transparent)]
    PayloadCap(#[from] MoverPayloadCapExceeded),
}

/// What [`bake_mover_bounce`] hands back.
#[derive(Default)]
pub(crate) struct MoverBounceBake {
    /// Id 57's bounce and depth blocks from every finished mover.
    pub(crate) blocks: BounceBlocks,
    /// Each finished mover with mid poses, in slot order, and what its
    /// skips kept and dropped.
    pub(crate) skips: Vec<(u32, SkipCounts)>,
    /// The refusal the bake stopped at: the movers' depth past the tile
    /// range, judged before that mover skipped, or its blocks past the
    /// payload cap, judged before they joined.
    pub(crate) refused: Option<MoverBounceRefusal>,
    /// Each pose's re-bake, when the request keeps them.
    pub(crate) poses: Vec<PosedBounce>,
    /// Each finished mover's reduced states before its skips, in slot
    /// order, when the request keeps them.
    #[cfg(test)]
    pub(crate) reduced: Vec<MoverBounceStates>,
}

/// What every baked pose shares, built at the first pose that bakes.
pub(super) struct PoseShared<'a> {
    pub(super) reach: PatchReachIndex<'a>,
    pub(super) terms: Vec<super::bounce::LightTerm<'a>>,
    #[cfg(test)]
    _reach: crate::live_bytes::Held,
}

impl<'a> PoseShared<'a> {
    fn new(inputs: &StaticBounce<'a>) -> Self {
        Self {
            reach: PatchReachIndex::new(inputs.patches.patches()),
            #[cfg(test)]
            _reach: crate::live_bytes::hold(
                "mover bounce reach index",
                inputs.patches.len() as u64 * crate::surface_radiance::REACH_INDEX_BYTES_PER_PATCH,
            ),
            terms: light_terms(inputs.lights, inputs.plan),
        }
    }
}

/// What every pose of one SH base bake reads: the gate's patches, the
/// plan's light order, and the gathered id 34 the bar compares against with
/// its depth tiles before mover tiles reserve their range.
pub(crate) struct StaticBounce<'a> {
    pub(crate) patches: &'a PatchSet,
    pub(crate) lights: &'a [&'a MapLight],
    pub(crate) plan: &'a LayerPlan,
    pub(crate) bounce_fill: f32,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) base: &'a OctahedralShVolumeSection,
    pub(crate) depth_tiles: &'a ProbeDepthTiles,
}

/// Builds what a baked pose reads when the stage built none of it before
/// the bake: on an id-34 volume hit whose every pose the lookup found, a
/// slot whose read-back fails bakes that pose, and needs these then.
pub(crate) trait PosedRadianceSource {
    /// The patches' static bounce one: every light's layer resolved and
    /// summed.
    fn bounce_one(&mut self) -> PatchRadiance;
    /// Id 34's later bounces from `bounce_one`: their memo, else gathered.
    fn absent(&mut self, bounce_one: &PatchRadiance) -> AbsentLaterBounces;
}

/// One of the inputs a baked pose reads: built before the bake, built at
/// the first slot that needs it, or not yet built.
enum Built<'a, T> {
    Given(&'a T),
    Owned(T),
    Absent,
}

impl<T> Built<'_, T> {
    fn get(&self) -> Option<&T> {
        match self {
            Self::Given(value) => Some(value),
            Self::Owned(value) => Some(value),
            Self::Absent => None,
        }
    }
}

/// Bounce one and id 34's later bounces as the posed bakes read them. The
/// stage hands in what it built for the jobs the lookup left pending; a
/// pose that bakes and needs more builds it from `source` at its slot, so
/// every mover, cold or warm, bakes at its own slot.
pub(crate) struct SlotRadiance<'a> {
    bounce_one: Built<'a, PatchRadiance>,
    absent: Built<'a, AbsentLaterBounces>,
    source: Option<&'a mut dyn PosedRadianceSource>,
    empty: PatchRadiance,
    #[cfg(test)]
    _absent: Option<crate::live_bytes::Held>,
}

impl<'a> SlotRadiance<'a> {
    /// What the stage built before the bake, and what builds the rest.
    pub(crate) fn new(
        bounce_one: Option<&'a PatchRadiance>,
        absent: Option<&'a AbsentLaterBounces>,
        source: Option<&'a mut dyn PosedRadianceSource>,
    ) -> Self {
        Self {
            bounce_one: bounce_one.map_or(Built::Absent, Built::Given),
            absent: absent.map_or(Built::Absent, Built::Given),
            source,
            empty: PatchRadiance::zeroed(0),
            #[cfg(test)]
            _absent: None,
        }
    }

    /// What a baked pose of `job` reads at `fill`: bounce one when it
    /// re-bakes radiance, id 34's later bounces when it reads them
    /// ([`AbsentLaterBounces::needed_by`]), each built first when missing.
    fn for_job(
        &mut self,
        job: &MoverBounceJob,
        fill: f32,
    ) -> (&PatchRadiance, Option<&AbsentLaterBounces>) {
        if job.bounces == 0 {
            return (&self.empty, self.absent.get());
        }
        if self.bounce_one.get().is_none() {
            let source = self
                .source
                .as_mut()
                .expect("a pose re-baking radiance has bounce one or a source");
            self.bounce_one = Built::Owned(source.bounce_one());
        }
        if self.absent.get().is_none() && AbsentLaterBounces::needed_by([job], fill) {
            let bounce_one = self.bounce_one.get().expect("built above");
            let source = self
                .source
                .as_mut()
                .expect("a pose reading id 34's later bounces has them or a source");
            let absent = source.absent(bounce_one);
            #[cfg(test)]
            {
                self._absent = Some(crate::live_bytes::hold(
                    "mover absent later bounces",
                    absent.heap_bytes(),
                ));
            }
            self.absent = Built::Owned(absent);
        }
        (
            self.bounce_one.get().expect("built above"),
            self.absent.get(),
        )
    }
}

/// What finishing a mover reads beside its states: the skip bars over id 34
/// and id 59 before mover tiles reserve their range, and id 34's grid.
struct Finish<'a> {
    bars: Option<SkipBars<'a>>,
    cells: Option<ProbeCells>,
    /// The movers whose blocks joined, in job order, for the cap's error.
    joined: Vec<String>,
    /// The loop's buffers in the test ledger.
    #[cfg(test)]
    held: Vec<crate::live_bytes::Held>,
}

/// Re-bake id 34 per pose of each slot whose memo misses, each mover posed
/// alone, and finish each mover before the next runs. Slots run in request
/// order, cold or warm: the lookup only planned. A slot reads back the
/// poses `memo` holds and bakes the rest, those whose read-back fails among
/// them, building what they read from `radiance` when it is the first to
/// need it. It reduces them; edge copy runs and its baked poses are written
/// to their memo, unless its depth alone passes the tile range. Its depth
/// then joins the union the tile-range verdict reads, before it skips, so
/// the verdict never hangs on job order ([`DepthUnion`]); past the range
/// the bake stops there and hands the refusal back. Otherwise its mid poses
/// skip, its states encode into id 57's blocks and drop, so no two movers'
/// states are live at once. A pose holds one posed copy of the patch
/// radiance and the later-bounce buffers while it runs, and drops them
/// before the next.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    radiance: &mut SlotRadiance<'_>,
    slots: Vec<MoverSlot<'_>>,
    request: &MoverBounceRequest,
    memo: Option<&PoseMemo<'_>>,
    control: &BakeControl,
) -> MoverBounceBake {
    if slots.is_empty() {
        return MoverBounceBake::default();
    }
    let valid = valid_probes(inputs.layout);
    let absent_depth = MoverAbsentDepth::new(inputs.base, inputs.depth_tiles);
    #[cfg(test)]
    let _absent_depth = crate::live_bytes::hold("mover absent depth", absent_depth.heap_bytes());
    let mut shared: Option<PoseShared<'_>> = None;
    let mut finish = Finish {
        bars: None,
        cells: None,
        joined: Vec::new(),
        #[cfg(test)]
        held: Vec::new(),
    };
    let mut out = MoverBounceBake::default();
    let mut union = DepthUnion::default();
    let count = slots.len();
    let mut slots = slots.into_iter().enumerate();
    while let Some((at, slot)) = slots.next() {
        let Some(states) = bake_slot(
            &SlotBake {
                sh_ctx,
                inputs,
                job: slot.job,
                pose_slots: slot.poses,
                request,
                memo,
                control,
                valid: &valid,
                absent_depth: &absent_depth,
            },
            &mut shared,
            radiance,
            &mut out.poses,
        ) else {
            continue;
        };
        #[cfg(test)]
        if request.keep_states {
            out.reduced.push(states.detached());
        }
        union.join(&states);
        let finished = union
            .verdict(at + 1 < count, request.depth_cap)
            .map_err(MoverBounceRefusal::from)
            .and_then(|()| {
                finish_mover(states, slot.job, inputs, request, &mut finish, &mut out)
                    .map_err(MoverBounceRefusal::from)
            });
        if let Err(refused) = finished {
            // The build fails by name before this mover's blocks join; the
            // jobs left never run, so their progress lands at once, their
            // held poses' included.
            control.advance(
                slots
                    .map(|(_, slot)| {
                        poses_progress_units(
                            slot.job,
                            slot.poses,
                            valid.len(),
                            inputs.patches,
                            inputs.bounce_fill,
                            |_| true,
                        )
                    })
                    .sum(),
            );
            out.refused = Some(refused);
            break;
        }
    }
    if out.refused.is_none() {
        log::info!(
            "[MoverLight] probe channel blocks: {} B of the {} B --mover-payload-max-size, from \
             {} mover(s)",
            out.blocks.bytes(),
            request.payload_cap,
            finish.joined.len(),
        );
    }
    out
}

/// What one slot's bake reads.
struct SlotBake<'s, 'a> {
    sh_ctx: &'s ShBakeCtx<'a>,
    inputs: &'s StaticBounce<'s>,
    job: &'s MoverBounceJob,
    pose_slots: &'s [PoseSlot],
    request: &'s MoverBounceRequest,
    memo: Option<&'s PoseMemo<'s>>,
    control: &'s BakeControl,
    valid: &'s [u32],
    absent_depth: &'s MoverAbsentDepth<'s>,
}

/// Resolve one slot's poses, each read back from the memo where it held
/// the pose, else baked, and reduce them to the mover's states, edge copy
/// run and its baked poses memoized. A pose whose read-back fails bakes
/// here, building the reach index, bounce one and id 34's later bounces
/// first when the slot is the first to need them. `None` when the request
/// keeps each pose's re-bake instead.
fn bake_slot<'s>(
    slot: &SlotBake<'s, '_>,
    shared: &mut Option<PoseShared<'s>>,
    radiance: &mut SlotRadiance<'_>,
    kept: &mut Vec<PosedBounce>,
) -> Option<MoverBounceStates> {
    let SlotBake {
        sh_ctx,
        inputs,
        job,
        pose_slots,
        request,
        memo,
        control,
        valid,
        absent_depth,
    } = *slot;
    let started = Instant::now();
    let held = read_held_poses(job, pose_slots, memo, inputs.layout, request);
    // A pose read back advances now; one whose read fails bakes, and
    // advances as it bakes.
    control.advance(poses_progress_units(
        job,
        pose_slots,
        valid.len(),
        inputs.patches,
        inputs.bounce_fill,
        |pose| held[pose].is_some(),
    ));
    let bounces: Vec<u32> = pose_slots.iter().map(|slot| slot.bounces).collect();
    let resolved = if held.iter().all(Option::is_some) {
        ResolvedPoses::read_back(held)
    } else {
        let shared = shared.get_or_insert_with(|| PoseShared::new(inputs));
        let (bounce_one, absent) = radiance.for_job(job, inputs.bounce_fill);
        let baker = PoseBaker {
            sh_ctx,
            ctx: sh_ctx.ray_ctx(),
            inputs,
            bounce_one,
            absent,
            shared,
            job,
            request,
            control,
            valid,
        };
        baker.bake(held, &bounces, kept)?
    };
    #[cfg(test)]
    let _raw = resolved.held;
    let baked = resolved.baked;
    let poses = MoverPoseStates::from_poses(resolved.poses);
    let mut states = reduce_poses(job.mover_id, &poses, absent_depth, request.depth_cap);
    copy_job_bounce_edges(&mut states, job, inputs.layout);
    if let Some(memo) = memo
        && !states.depth_past_range(request.depth_cap)
    {
        write_baked_poses(memo, pose_slots, poses.into_poses(), &baked);
    } else {
        drop(poses);
    }
    if baked.contains(&true) {
        log::info!(
            "[MoverLight] probe channel bounce: mover `{}` (#{}) re-baked {} posed bounce(s) at \
             {} of {} pose(s), {} near-field probe(s): {} affected probe(s), {} probe(s) \
             carrying closed-pose depth, {:.1} s",
            job.label,
            job.mover_id,
            baked_bounces(&bounces, &baked),
            baked.iter().filter(|&&baked| baked).count(),
            job.scenes.len(),
            job.domain_probes.len(),
            states.bounce.len(),
            states.depth_carried,
            started.elapsed().as_secs_f64(),
        );
    }
    Some(states)
}

/// Skip a mover's redundant mid-pose states, then encode them into the
/// blocks and drop them. Its knots run through its author ramp first, which
/// no memo keys, so a ramp edit re-decides skips over memoized states.
/// Before they encode, the blocks' bytes with the mover's are checked
/// against the payload cap: past it, nothing joins and the build fails by
/// name. Logs one line per mover with mid poses.
fn finish_mover<'a>(
    mut states: MoverBounceStates,
    job: &MoverBounceJob,
    inputs: &StaticBounce<'a>,
    request: &MoverBounceRequest,
    finish: &mut Finish<'a>,
    out: &mut MoverBounceBake,
) -> Result<(), MoverPayloadCapExceeded> {
    if states.pose_count > 2 {
        let bars = finish.bars.get_or_insert_with(|| {
            let bars = SkipBars::new(
                inputs.base,
                MoverAbsentDepth::new(inputs.base, inputs.depth_tiles),
                request.uncompressed,
            );
            #[cfg(test)]
            finish.held.push(crate::live_bytes::hold(
                "mover skip bars",
                bars.heap_bytes(),
            ));
            bars
        });
        let started = Instant::now();
        let counts = skip_redundant_mid_poses(&mut states, &job.read_knots, bars);
        log::info!(
            "[MoverLight] probe channel skipping: mover `{}` (#{}): bounce keeps {} and skips \
             {} (probe, mid pose) state(s); depth keeps {} and skips {}; {} of the skipped \
             never read past the ramp; {:.2} s",
            job.label,
            states.mover_id,
            counts.bounce_stored,
            counts.bounce_skipped,
            counts.depth_stored,
            counts.depth_skipped,
            counts.squeezed,
            started.elapsed().as_secs_f64(),
        );
        out.skips.push((states.mover_id, counts));
    }
    let base = inputs.base;
    let cells = finish.cells.get_or_insert_with(|| {
        let cells = ProbeCells::new(base.grid_dimensions, |probe| {
            base.probes[probe].validity != 0
        });
        #[cfg(test)]
        finish.held.push(crate::live_bytes::hold(
            "mover probe cells",
            (cells.valid_masks.capacity() * size_of::<u64>()) as u64,
        ));
        cells
    });
    let bytes = out
        .blocks
        .bytes()
        .saturating_add(BounceBlocks::mover_bytes(&states, cells));
    if bytes > request.payload_cap {
        let joined: Vec<&str> = finish.joined.iter().map(String::as_str).collect();
        return Err(MoverPayloadCapExceeded {
            bytes,
            cap: request.payload_cap,
            mover: job.label.clone(),
            joined: if joined.is_empty() {
                "none".to_owned()
            } else {
                joined.join(", ")
            },
        });
    }
    out.blocks.push_mover(&states, cells);
    finish.joined.push(job.label.clone());
    #[cfg(test)]
    finish.held.push(crate::live_bytes::hold(
        "mover finish names",
        job.label.capacity() as u64 + size_of::<String>() as u64,
    ));
    Ok(())
}

/// The posed bounces the baked poses re-baked: one count, or each distinct
/// one where a pose whose `high` entry failed to read baked at it.
fn baked_bounces(bounces: &[u32], baked: &[bool]) -> String {
    let mut counts: Vec<u32> = bounces
        .iter()
        .zip(baked)
        .filter(|(_, baked)| **baked)
        .map(|(bounces, _)| *bounces)
        .collect();
    counts.sort_unstable();
    counts.dedup();
    if counts.is_empty() {
        return "no".to_owned();
    }
    counts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

/// Progress units the poses of `job` that `counted` picks advance, each at
/// the count its slot in `poses` resolves at
/// ([`MoverBounceJob::pose_progress_units`]).
pub(crate) fn poses_progress_units(
    job: &MoverBounceJob,
    poses: &[PoseSlot],
    valid_probes: usize,
    patches: &PatchSet,
    fill: f32,
    counted: impl Fn(usize) -> bool,
) -> usize {
    if !(0..poses.len()).any(&counted) {
        return 0;
    }
    let domain_patches = job.domain_patch_count(patches, fill);
    poses
        .iter()
        .enumerate()
        .filter(|(pose, _)| counted(*pose))
        .map(|(pose, slot)| {
            job.pose_progress_units(
                pose,
                slot.bounces,
                valid_probes,
                patches.len(),
                domain_patches,
                fill,
            )
        })
        .sum()
}

/// Each pose the memo held at lookup, read again; `None` where it held
/// none, or where the read now fails, so the bake bakes that pose.
fn read_held_poses(
    job: &MoverBounceJob,
    poses: &[PoseSlot],
    memo: Option<&PoseMemo<'_>>,
    layout: &ProbeGridLayout,
    request: &MoverBounceRequest,
) -> Vec<Option<PoseState>> {
    #[cfg(not(test))]
    let _ = request;
    poses
        .iter()
        .enumerate()
        .map(|(pose, slot)| {
            let (memo, key) = memo.zip(slot.key.as_ref()).filter(|_| slot.held)?;
            let state = memo.get(
                key,
                PoseKind::of(pose, poses.len()),
                layout,
                &job.domain_probes,
            );
            #[cfg(test)]
            if state.is_some() && request.fail_read_back.contains(&(job.mover_id, pose)) {
                crate::bake_counters::bump(|counts| counts.read_backs_failed += 1);
                return None;
            }
            state
        })
        .collect()
}

/// Write every pose `baked` marks to its memo key, dropping each pose's
/// states as it goes.
fn write_baked_poses(
    memo: &PoseMemo<'_>,
    slots: &[PoseSlot],
    poses: Vec<PoseState>,
    baked: &[bool],
) {
    for ((slot, state), &baked) in slots.iter().zip(poses).zip(baked) {
        if let Some(key) = slot.key.as_ref().filter(|_| baked) {
            memo.put(key, &state);
        }
    }
}
