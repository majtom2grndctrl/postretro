// SH base stage over the patch cache: layers cached or shaded, summed, gathered, then freed.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{add_later_bounces, later_bounce_progress_units};
use super::gather::{PatchGather, PatchLookup};
use super::layer::LayerKeys;
use super::layer_set::{LayerCache, LayerPlan, resolve_layers};
use super::patches::PatchSet;
use super::sum::PatchRadiance;
use super::working_set::ShBaseHandoff;
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::light_namespaces::StaticBakedLights;
use crate::sh_bake::{
    RadianceSource, ShBakeCtx, ShConfig, bake_sh_volume_after, probe_grid_dims, static_light_refs,
};
use crate::texel_footprint::EmbeddedEmitters;

/// The bake-only cache one SH base bake builds: the gate's patches and
/// the radiance the probe gather reads over them, every static light's
/// summed bounce one plus the map's fill of later bounces. It lives
/// only inside [`bake_sh_base`], so it is freed before the delta SH stage
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
/// its content digest ([`LayerPlan`]); with `cache`, a layer is first read
/// from the build cache and, on a miss, written back the moment it is
/// shaded. The layers sum in content order into bounce one; the later
/// bounces gather patch to patch at the gate's fill
/// ([`ShBaseHandoff::bounce_fill`]), and the probe gather reads the result
/// by bilinear lookup. No layer keys on the fill. `embedded` is the bounce
/// family's set ([`crate::direct_sh_bake::bounce_embedded_emitters`]),
/// keyed by static-light position. Cold (`cache == None`) and warm bake the
/// same bytes. Progress counts one unit per patch chunk of each layer's
/// reach, then one per patch chunk of each later bounce, then one per probe.
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
    let shading_units = plan.progress_units();
    let bounce_units = later_bounce_progress_units(patches.len(), bounce_fill);
    let probes: usize = probe_grid_dims(ctx.geometry, config.probe_spacing)
        .iter()
        .map(|&d| d as usize)
        .product();
    control.publish_total(shading_units + bounce_units + probes);
    let layer_cache = cache.map(|cache| LayerCache {
        cache,
        keys: LayerKeys::new(ctx.geometry, ctx.bounce_materials),
    });
    let layers = resolve_layers(
        &ctx.ray_ctx(),
        &patches,
        &lights,
        &plan,
        layer_cache.as_ref(),
        control,
    );
    let mut radiance = PatchRadiance::sum(patches.len(), &plan, &layers);
    drop(layers);
    add_later_bounces(
        &ctx.ray_ctx(),
        &patches,
        &mut radiance,
        bounce_fill,
        control,
    );
    let patch_cache = PatchCache { patches, radiance };
    let gather = PatchGather::new(
        &patch_cache.patches,
        &patch_cache.radiance,
        PatchLookup::Bilinear,
    );
    bake_sh_volume_after(
        ctx,
        config,
        control,
        RadianceSource::Patches(&gather),
        shading_units + bounce_units,
    )
}

/// Warn once for each static light only the bounce family's test flags:
/// an `sdf`-shadowed light, whose direct shadow bakes no texel or probe, so
/// the lightmap stage never names it. Lights the probe-point set flags keep
/// that stage's line. Both sets are keyed by position in `static_lights`.
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
