// SH base stage over the patch cache: shade, sum and gather, then free the cache.
// See: context/lib/build_pipeline.md §Compiler pipeline

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::gather::{PatchGather, PatchLookup};
use super::layer::layer_progress_units;
use super::patches::PatchSet;
use super::sum::PatchRadiance;
use super::working_set::ShBaseHandoff;
use crate::bake_control::BakeControl;
use crate::light_namespaces::StaticBakedLights;
use crate::sh_bake::{
    RadianceSource, ShBakeCtx, ShConfig, bake_sh_volume_after, probe_grid_dims, static_light_refs,
};
use crate::texel_footprint::EmbeddedEmitters;

/// The bake-only cache one SH base bake builds: the gate's patches and
/// every static light's summed single-bounce radiance over them. It lives
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

/// Bake id 34 through the surface radiance cache: each static light
/// shades its layer over the gate's patch set (seed axis and embedded flag
/// keyed by its global static index), the layers sum in that order, and
/// the probe gather reads the sum by bilinear lookup. `embedded` is the
/// bounce family's set ([`crate::direct_sh_bake::bounce_embedded_emitters`]).
/// Progress counts one unit per light per patch chunk, then one per probe.
pub(crate) fn bake_sh_base(
    ctx: &ShBakeCtx<'_>,
    config: &ShConfig,
    handoff: ShBaseHandoff,
    embedded: &EmbeddedEmitters,
    control: &BakeControl,
) -> OctahedralShVolumeSection {
    let ShBaseHandoff {
        patches,
        light_reach,
    } = handoff;
    let lights = static_light_refs(ctx);
    debug_assert_eq!(
        light_reach.len(),
        lights.len(),
        "the gate priced every static light"
    );
    let shading_units = lights.len() * layer_progress_units(patches.len());
    let probes: usize = probe_grid_dims(ctx.geometry, config.probe_spacing)
        .iter()
        .map(|&d| d as usize)
        .product();
    control.publish_total(shading_units + probes);
    let priced_patches = patches.len();
    let radiance =
        PatchRadiance::single_bounce(&ctx.ray_ctx(), &patches, &lights, embedded, control);
    // AC 31: the bake shades exactly the set the gate priced, as the delta
    // bakes' CSRs equal their plans.
    debug_assert_eq!(radiance.patch_count(), priced_patches);
    let cache = PatchCache { patches, radiance };
    let gather = PatchGather::new(&cache.patches, &cache.radiance, PatchLookup::Bilinear);
    bake_sh_volume_after(
        ctx,
        config,
        control,
        RadianceSource::Patches(&gather),
        shading_units,
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
