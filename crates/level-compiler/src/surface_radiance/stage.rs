// SH base stage over the patch cache: layers cached or shaded, summed, gathered, then freed.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{add_later_bounces, later_bounce_progress_units};
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
use crate::sh_bake::{
    RadianceSource, ShBakeCtx, ShConfig, animation_tables, bake_sh_volume_over, probe_grid_layout,
    static_light_refs,
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
pub(crate) fn bake_sh_base(
    ctx: &ShBakeCtx<'_>,
    config: &ShConfig,
    handoff: ShBaseHandoff,
    embedded: &EmbeddedEmitters,
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> OctahedralShVolumeSection {
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
        let keys = LayerKeys::new(ctx.geometry, ctx.bounce_materials);
        let volume = VolumeCache {
            cache,
            key: volume_key(&keys, &plan, &layout, config.probe_spacing, bounce_fill),
        };
        (LayerCache { cache, keys }, volume)
    });
    if let Some((layers, volume)) = &caches
        && let Some(mut section) = volume.load(&layout)
    {
        for planned in &plan.layers {
            layers
                .cache
                .mark_used(&layers.keys.key(planned.digest, planned.embedded));
        }
        (section.animation_descriptors, section.slot_for_map_light) = animation_tables(ctx);
        #[cfg(test)]
        let _section = crate::live_bytes::hold(
            "id-34 section",
            crate::sh_bake::section_heap_bytes(&section),
        );
        #[cfg(test)]
        crate::live_bytes::phase("volume load");
        control.advance(total_units);
        return section;
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
    add_later_bounces(
        &ctx.ray_ctx(),
        &patches,
        &mut radiance,
        bounce_fill,
        control,
    );
    #[cfg(test)]
    crate::live_bytes::phase("later bounces");
    let patch_cache = PatchCache { patches, radiance };
    let mut section = {
        let gather = PatchGather::new(
            &patch_cache.patches,
            &patch_cache.radiance,
            PatchLookup::PRODUCTION,
        );
        bake_sh_volume_over(ctx, &layout, control, RadianceSource::Patches(&gather))
    };
    #[cfg(test)]
    let _section = crate::live_bytes::hold(
        "id-34 section",
        crate::sh_bake::section_heap_bytes(&section),
    );
    #[cfg(test)]
    crate::live_bytes::phase("probe gather");
    drop(patch_cache);
    if let Some((_, volume)) = &caches {
        volume.store(&mut section);
    }
    #[cfg(test)]
    crate::live_bytes::phase("volume store");
    section
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
