// SH base stage over the patch cache: layers cached or shaded, summed, gathered, then freed.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{add_later_bounces_keeping, later_bounce_layers, later_bounce_progress_units};
use super::gather::{PatchGather, PatchLookup};
use super::layer::LayerKeys;
use super::layer_set::{LayerCache, LayerPlan, resolve_layers};
use super::patches::PatchSet;
use super::sum::PatchRadiance;
use super::volume::{VolumeCache, volume_key};
use super::working_set::ShBaseHandoff;
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::light_namespaces::StaticBakedLights;
use crate::mover_probe_channel::{
    BounceMemo, MoverBounceJob, MoverBounceRequest, MoverBounceStates, PosedBounce, StaticBounce,
    StaticTail, bake_mover_bounce,
};
use crate::sh_bake::{
    GatheredVolume, ProbeDepthTiles, ProbeGridLayout, RadianceSource, ShBakeCtx, ShConfig,
    animation_tables, bake_sh_volume_over, probe_grid_layout, static_light_refs,
};
use crate::texel_footprint::EmbeddedEmitters;

/// The bake-only cache one SH base bake builds: the gate's patches and
/// the radiance the probe gather reads over them, every static light's
/// summed bounce one plus the map's fill of later bounces. It lives
/// only inside [`bake_sh_base`], dropped once the gather ends and before
/// the volume's cache write, so it is freed before the delta SH stage
/// begins; only the id-34 section leaves.
struct PatchCache {
    patches: PatchSet,
    radiance: PatchRadiance,
}

#[cfg(test)]
impl Drop for PatchCache {
    fn drop(&mut self) {
        stage_events::record(stage_events::PATCH_CACHE_DROPPED);
    }
}

/// What the SH base stage hands on: id 34 and, from the same gather, the
/// probe depth tiles (id 59).
#[derive(Debug, PartialEq)]
pub(crate) struct ShBase {
    pub(crate) section: OctahedralShVolumeSection,
    pub(crate) depth_tiles: ProbeDepthTiles,
}

/// What the mover probe channel's bounce bake hands on beside id 34.
#[derive(Default)]
pub(crate) struct MoverBounceOutcome {
    /// One per production job, ascending by mover id: from the memo or
    /// baked.
    pub(crate) movers: Vec<MoverBounceStates>,
    /// The dev switches' per-pose view.
    pub(crate) poses: Vec<PosedBounce>,
}

/// [`bake_sh_base_with_movers`] with no mover poses: id 34 and id 59 alone,
/// with the depth tiles' once-per-build line.
#[cfg(test)]
pub(crate) fn bake_sh_base(
    ctx: &ShBakeCtx<'_>,
    config: &ShConfig,
    handoff: ShBaseHandoff,
    embedded: &EmbeddedEmitters,
    cache: Option<&StageCache>,
    control: &BakeControl,
    tile_cap: u32,
) -> ShBase {
    let base = bake_sh_base_with_movers(
        ctx,
        config,
        handoff,
        embedded,
        cache,
        control,
        tile_cap,
        &MoverBounceRequest::default(),
    )
    .0;
    base.depth_tiles.log(tile_cap);
    base
}

/// The jobs the memo holds, by job, and the jobs left to bake.
struct MemoLookup<'r, 'c> {
    memo: Option<BounceMemo<'c>>,
    found: Vec<Option<MoverBounceStates>>,
    keys: Vec<Option<crate::cache::CacheKey>>,
    pending: Vec<&'r MoverBounceJob>,
}

impl<'r, 'c> MemoLookup<'r, 'c> {
    /// Read every job's memo under id 34's volume key, when a cache is open
    /// and the request is production's. Logs one `[cache]` line and one line
    /// per mover.
    fn new(
        movers: &'r MoverBounceRequest,
        volume: Option<&VolumeCache<'c>>,
        ctx: &ShBakeCtx<'_>,
        layout: &ProbeGridLayout,
    ) -> Self {
        let memo = volume
            .filter(|_| !movers.keep_poses)
            .map(|volume| BounceMemo::new(volume.cache, &volume.key, movers.uncompressed));
        let mut lookup = Self {
            memo,
            found: Vec::with_capacity(movers.jobs.len()),
            keys: Vec::with_capacity(movers.jobs.len()),
            pending: Vec::new(),
        };
        for job in &movers.jobs {
            let key = lookup
                .memo
                .as_ref()
                .map(|memo| memo.key(job.bounces, &job.scenes, ctx.bounce_materials));
            let found = lookup
                .memo
                .as_ref()
                .zip(key.as_ref())
                .and_then(|(memo, key)| memo.get(key, job.mover_id, layout));
            if lookup.memo.is_some() {
                log::info!(
                    "[MoverLight] probe channel bounce memo: mover `{}` (#{}) {}",
                    job.label,
                    job.mover_id,
                    if found.is_some() { "hit" } else { "miss" },
                );
            }
            if found.is_none() {
                lookup.pending.push(job);
            }
            lookup.found.push(found);
            lookup.keys.push(key);
        }
        if lookup.memo.is_some() {
            BounceMemo::log_outcome(movers.jobs.len(), movers.jobs.len() - lookup.pending.len());
        }
        lookup
    }

    /// Whether a pending job re-bakes radiance, which needs the patches'
    /// bounce one; a job at `light_bounces` 0 traces depth alone.
    fn needs_radiance(&self) -> bool {
        self.pending.iter().any(|job| job.bounces > 0)
    }

    /// Store each baked job's states and merge them with the memo's, by
    /// mover id.
    fn finish(
        mut self,
        movers: &MoverBounceRequest,
        baked: Vec<MoverBounceStates>,
    ) -> Vec<MoverBounceStates> {
        for states in baked {
            let Some(at) = movers
                .jobs
                .iter()
                .position(|job| job.mover_id == states.mover_id)
            else {
                continue;
            };
            if let (Some(memo), Some(key)) = (self.memo.as_ref(), self.keys[at].as_ref()) {
                memo.put(key, &states);
            }
            self.found[at] = Some(states);
        }
        let mut states: Vec<MoverBounceStates> = self.found.into_iter().flatten().collect();
        states.sort_by_key(|states| states.mover_id);
        states
    }
}

/// Id 34's later bounces the pending jobs keep past their posed ones, from
/// the first any job keeps, held apart from `kept`.
fn static_tail(
    jobs: &[&MoverBounceJob],
    fill: f32,
    kept: Vec<Vec<[f32; 3]>>,
) -> Option<StaticTail> {
    MoverBounceRequest::static_tail_from(jobs.iter().copied(), fill).map(|first| StaticTail {
        first,
        layers: kept,
    })
}

/// Bake id 34 through the surface radiance cache. Each distinct static
/// light content shades one layer over the patches in its reach, seeded by
/// its content digest ([`LayerPlan`]). The layers sum in content order into
/// bounce one; the later bounces gather patch to patch at the gate's fill
/// ([`ShBaseHandoff::bounce_fill`]), and the probe gather reads the result
/// by bilinear lookup. No layer keys on the fill. `embedded` is the bounce
/// family's set ([`crate::direct_sh_bake::bounce_embedded_emitters`]),
/// keyed by static-light position.
///
/// With `cache`, the plan (digests and flags alone, no shading) and the
/// probe grid key the gathered volume ([`volume_key`]). A hit is id 34 with
/// its animation tables rebuilt from this build's lights: no layer is read,
/// and every layer it sums is marked used so eviction spares them. A miss
/// reads each layer from the cache or shades it and writes it back the
/// moment it finishes, then sums, bounces, gathers and writes the volume.
/// Cache reads and writes take no governor permit. Cold (`cache == None`)
/// and warm bake the same bytes. Progress counts one unit per patch chunk
/// of each layer's reach, then one per patch chunk of each later bounce,
/// then one per probe; a hit advances them all at once.
///
/// The gather's hits also bin into each valid probe's depth tile, kept when
/// it changes the probe's visibility, at most `tile_cap` per level
/// (production passes `MAX_PROBE_DEPTH_TILES`). The tiles ride the volume
/// entry; the caller logs their kept and dropped counts once, after mover
/// depth reserves its share of the range.
///
/// Then the mover probe channel's bounce and depth bake runs for every
/// mover `movers` names whose memo, keyed on this volume's key, misses:
/// over the same patches while they live, before the patch cache drops, so
/// a cold build drops it once, after the mover bake. On a volume miss it
/// reads the bounce one this bake summed, kept before the later bounces add
/// to it, and the later bounces its movers keep past their posed ones; on a
/// hit no patch cache exists, so it sums the cached layers over the gate's
/// patch set and gathers those later bounces afresh. A mover at
/// `light_bounces` 0 needs neither. An empty request keeps nothing extra
/// and bakes today's bytes.
// `bake_sh_base`'s inputs plus the request.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_sh_base_with_movers(
    ctx: &ShBakeCtx<'_>,
    config: &ShConfig,
    handoff: ShBaseHandoff,
    embedded: &EmbeddedEmitters,
    cache: Option<&StageCache>,
    control: &BakeControl,
    tile_cap: u32,
    movers: &MoverBounceRequest,
) -> (ShBase, MoverBounceOutcome) {
    let ShBaseHandoff {
        patches,
        light_reach,
        bounce_fill,
    } = handoff;
    let lights = static_light_refs(ctx);
    let plan = LayerPlan::new(&lights, &light_reach, embedded);
    let layout = probe_grid_layout(ctx, config);
    #[cfg(test)]
    let _held = [
        crate::live_bytes::hold_vec("per-light metadata", &lights),
        crate::live_bytes::hold_vec("per-light metadata", &light_reach),
        crate::live_bytes::hold_vec("probe layout", &layout.probe_positions),
        crate::live_bytes::hold_vec("probe layout", &layout.validity),
    ];
    #[cfg(test)]
    crate::live_bytes::phase("plan and probe layout");
    let shading_units = plan.progress_units();
    let bounce_units = later_bounce_progress_units(patches.len(), bounce_fill);
    let total_units = shading_units + bounce_units + layout.total_probes();
    control.publish_total(total_units);

    // An empty grid gathers nothing, so it caches nothing either.
    let caches = cache.filter(|_| !layout.is_empty()).map(|cache| {
        let tracer_epoch = crate::ray_traversal::TRACER_EPOCH;
        let keys = LayerKeys::new(ctx.geometry, ctx.bounce_materials, tracer_epoch);
        let volume = VolumeCache {
            cache,
            key: volume_key(
                &keys,
                &plan,
                &layout,
                config.probe_spacing,
                bounce_fill,
                tracer_epoch,
                tile_cap,
            ),
        };
        (LayerCache { cache, keys }, volume)
    });
    if let Some((layers, volume)) = &caches
        && let Some((mut section, depth_tiles)) = volume.load(&layout)
    {
        for planned in &plan.layers {
            layers
                .cache
                .mark_used(&layers.keys.key(planned.digest, planned.embedded));
        }
        (section.animation_descriptors, section.slot_for_map_light) = animation_tables(ctx);
        #[cfg(test)]
        let _section = [
            crate::live_bytes::hold(
                "id-34 section",
                crate::sh_bake::section_heap_bytes(&section),
            ),
            crate::live_bytes::hold("probe depth tiles", depth_tiles.heap_bytes()),
        ];
        #[cfg(test)]
        crate::live_bytes::phase("volume load");
        control.advance(total_units);
        let lookup = MemoLookup::new(movers, Some(volume), ctx, &layout);
        let (baked, poses) = if lookup.pending.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let bounce_one = lookup.needs_radiance().then(|| {
                let layer_cache = caches.as_ref().map(|(layers, _)| layers);
                let resolved = resolve_layers(
                    &ctx.ray_ctx(),
                    &patches,
                    &lights,
                    &plan,
                    layer_cache,
                    control,
                );
                PatchRadiance::sum(patches.len(), &plan, &resolved)
            });
            let bounce_one = bounce_one.unwrap_or_else(|| PatchRadiance::zeroed(0));
            let tail =
                MoverBounceRequest::static_tail_from(lookup.pending.iter().copied(), bounce_fill)
                    .filter(|_| lookup.needs_radiance())
                    .map(|from| {
                        later_bounce_layers(
                            &ctx.ray_ctx(),
                            &patches,
                            &bounce_one,
                            bounce_fill,
                            from,
                            control,
                        )
                    });
            let tail = tail.and_then(|kept| static_tail(&lookup.pending, bounce_fill, kept));
            #[cfg(test)]
            let _tail = tail.as_ref().map(|tail| {
                tail.layers
                    .iter()
                    .map(|layer| crate::live_bytes::hold_vec("mover static later bounces", layer))
                    .collect::<Vec<_>>()
            });
            bake_mover_bounce(
                ctx,
                &StaticBounce {
                    patches: &patches,
                    bounce_one: &bounce_one,
                    lights: &lights,
                    plan: &plan,
                    bounce_fill,
                    layout: &layout,
                    base: &section,
                    depth_tiles: &depth_tiles,
                    static_tail: tail.as_ref(),
                },
                &lookup.pending,
                movers,
                control,
            )
        };
        #[cfg(test)]
        if !movers.is_empty() {
            stage_events::record(stage_events::MOVER_BOUNCE_DONE);
        }
        let states = lookup.finish(movers, baked);
        return (
            ShBase {
                section,
                depth_tiles,
            },
            MoverBounceOutcome {
                movers: states,
                poses,
            },
        );
    }

    let layers = resolve_layers(
        &ctx.ray_ctx(),
        &patches,
        &lights,
        &plan,
        caches.as_ref().map(|(layers, _)| layers),
        control,
    );
    #[cfg(test)]
    crate::live_bytes::phase("layers");
    let mut radiance = PatchRadiance::sum(patches.len(), &plan, &layers);
    #[cfg(test)]
    crate::live_bytes::phase("sum");
    drop(layers);
    let lookup = MemoLookup::new(
        movers,
        caches.as_ref().map(|(_, volume)| volume),
        ctx,
        &layout,
    );
    let bounce_one = lookup.needs_radiance().then(|| radiance.clone());
    // TEMPORARY (mover-light-flow--probe-channel task 2, B7 patch trace):
    // no-ops unless `POSTRETRO_DEV_PATCH_DUMP` is set.
    crate::mover_probe_channel::dev_trace::dump_patches("static-one", patches.patches(), &radiance);
    let keep_from =
        MoverBounceRequest::static_tail_from(lookup.pending.iter().copied(), bounce_fill)
            .filter(|_| lookup.needs_radiance());
    let kept = add_later_bounces_keeping(
        &ctx.ray_ctx(),
        &patches,
        &mut radiance,
        bounce_fill,
        keep_from,
        control,
    );
    let tail = keep_from.and_then(|_| static_tail(&lookup.pending, bounce_fill, kept));
    #[cfg(test)]
    let _tail = tail.as_ref().map(|tail| {
        tail.layers
            .iter()
            .map(|layer| crate::live_bytes::hold_vec("mover static later bounces", layer))
            .collect::<Vec<_>>()
    });
    crate::mover_probe_channel::dev_trace::dump_patches(
        "static-final",
        patches.patches(),
        &radiance,
    );
    #[cfg(test)]
    crate::live_bytes::phase("later bounces");
    let patch_cache = PatchCache { patches, radiance };
    let GatheredVolume {
        mut section,
        depth_tiles,
    } = {
        let gather = PatchGather::new(
            &patch_cache.patches,
            &patch_cache.radiance,
            PatchLookup::PRODUCTION,
        );
        bake_sh_volume_over(
            ctx,
            &layout,
            control,
            RadianceSource::Patches(&gather),
            tile_cap,
        )
    };
    #[cfg(test)]
    let _section = [
        crate::live_bytes::hold(
            "id-34 section",
            crate::sh_bake::section_heap_bytes(&section),
        ),
        crate::live_bytes::hold("probe depth tiles", depth_tiles.heap_bytes()),
    ];
    #[cfg(test)]
    crate::live_bytes::phase("probe gather");
    let (baked, poses) = if lookup.pending.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let empty = PatchRadiance::zeroed(0);
        bake_mover_bounce(
            ctx,
            &StaticBounce {
                patches: &patch_cache.patches,
                bounce_one: bounce_one.as_ref().unwrap_or(&empty),
                lights: &lights,
                plan: &plan,
                bounce_fill,
                layout: &layout,
                base: &section,
                depth_tiles: &depth_tiles,
                static_tail: tail.as_ref(),
            },
            &lookup.pending,
            movers,
            control,
        )
    };
    #[cfg(test)]
    if !movers.is_empty() {
        stage_events::record(stage_events::MOVER_BOUNCE_DONE);
    }
    #[cfg(test)]
    drop(_tail);
    drop(tail);
    drop(bounce_one);
    drop(patch_cache);
    if let Some((_, volume)) = &caches {
        volume.store(&mut section, &depth_tiles);
    }
    let states = lookup.finish(movers, baked);
    #[cfg(test)]
    crate::live_bytes::phase("volume store");
    (
        ShBase {
            section,
            depth_tiles,
        },
        MoverBounceOutcome {
            movers: states,
            poses,
        },
    )
}

/// Warn once for each static light only the bounce family's test flags:
/// an `sdf`-shadowed light, whose direct shadow bakes no texel or probe, so
/// the lightmap stage never names it. Lights the probe-point set flags, a
/// subset of the bounce set
/// ([`crate::direct_sh_bake::probe_point_emitters`]), keep that stage's
/// line. Both sets are keyed by position in `static_lights`.
/// Called on the caller thread every compile, cold or warm.
pub(crate) fn warn_bounce_only_emitters(
    bounce: &EmbeddedEmitters,
    probes: &EmbeddedEmitters,
    static_lights: &StaticBakedLights<'_>,
    light_source_labels: &[String],
) {
    let entries = static_lights.entries();
    EmbeddedEmitters::warn_bounce_only(
        "static",
        bounce,
        probes,
        |position| entries.get(position).map(|entry| entry.light),
        |position| {
            entries
                .get(position)
                .and_then(|entry| light_source_labels.get(entry.source_index))
                .map(String::as_str)
        },
    );
}

/// Ordered markers the pipeline's stage boundaries leave on the compiling
/// thread, so a test can see the patch cache freed before the next stage.
#[cfg(test)]
pub(crate) mod stage_events {
    use std::cell::RefCell;

    pub(crate) const PATCH_CACHE_DROPPED: &str = "surface radiance patch cache dropped";
    pub(crate) const MOVER_BOUNCE_DONE: &str = "mover probe channel bounce done";

    thread_local! {
        static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn record(event: impl Into<String>) {
        EVENTS.with(|events| events.borrow_mut().push(event.into()));
    }

    pub(crate) fn take() -> Vec<String> {
        EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }
}
