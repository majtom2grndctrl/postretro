// The lights the mover probe channel's direct half gates, and the one name
// each takes in id 57: every static lightmap light id 35 bakes and every
// animated-baked light id 45 bakes. SDF-shadow lights are not gated.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::HashMap;

use postretro_level_format::mover_probe_channel::MoverLightNamespace;

use super::direct::ChannelLight;
use crate::affinity_grid::AffinityReachPolicy;
use crate::animated_direct_sh_bake::animated_direct_reach_policies;
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::direct_sh_bake::static_direct_lights;
use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights};
use crate::map_data::ShadowType;
use crate::sh_bake::{ShBakeCtx, static_light_refs};
use crate::texel_footprint::{EmbeddedEmitters, ProbeEarlyOut};

/// Where the gated lights come from.
pub(crate) struct ChannelLightSources<'a> {
    pub(crate) sh_ctx: &'a ShBakeCtx<'a>,
    pub(crate) alpha_lights: &'a AlphaLightsNs<'a>,
    pub(crate) animated_lights: &'a AnimatedBakedLights<'a>,
    /// Keyed by position in `sh_ctx.static_lights`, as id 35 reads it.
    pub(crate) static_embedded: &'a EmbeddedEmitters,
    /// Keyed by position in `animated_lights`, as id 45 reads it.
    pub(crate) animated_embedded: &'a EmbeddedEmitters,
    pub(crate) mutable_descriptors: &'a ScriptMutableDescriptorSlots,
}

/// The gated lights, in id 35's then id 45's order. A static light with an
/// AlphaLights record is named by that index (namespace 0); one without, a
/// `_bake_only` light, by its static-baked index, id 35's seed axis
/// (namespace 2), which section assembly ranks into the section's bake-only
/// roster. So no light has two names.
pub(crate) fn channel_lights<'a>(sources: &ChannelLightSources<'a>) -> Vec<ChannelLight<'a>> {
    let static_refs = static_light_refs(sources.sh_ctx);
    let (direct_lights, global_indices) = static_direct_lights(&static_refs);
    let static_entries = sources.sh_ctx.static_lights.entries();
    let alpha_by_source: HashMap<usize, u32> = sources
        .alpha_lights
        .entries()
        .iter()
        .enumerate()
        .map(|(alpha, entry)| (entry.source_index, alpha as u32))
        .collect();
    let mut lights = Vec::new();
    for (light, &global) in direct_lights.iter().zip(&global_indices) {
        let source = static_entries[global as usize].source_index;
        let (namespace, light_index) = match alpha_by_source.get(&source) {
            Some(&alpha) => (MoverLightNamespace::StaticLight, alpha),
            None => (
                MoverLightNamespace::BakeOnlyStaticLight,
                u32::try_from(global).expect("static-baked light count fits u32"),
            ),
        };
        lights.push(ChannelLight {
            namespace,
            light_index,
            baked: (*light).clone(),
            authored: light,
            seed_axis: global,
            early_out: ProbeEarlyOut::for_probe_point(
                sources.static_embedded.is_embedded(global as usize),
            ),
            policy: AffinityReachPolicy::DIRECT,
        });
    }

    let animated = sources.animated_lights.entries();
    let policies = animated_direct_reach_policies(animated.len(), sources.mutable_descriptors);
    for (roster, (entry, policy)) in animated.iter().zip(policies).enumerate() {
        if entry.light.shadow_type == ShadowType::Sdf {
            continue;
        }
        // Id 45 bakes unit radiance; the animation descriptor applies the
        // authored colour and intensity at runtime.
        let mut baked = entry.light.clone();
        baked.intensity = 1.0;
        baked.color = [1.0; 3];
        lights.push(ChannelLight {
            namespace: MoverLightNamespace::AnimatedBaked,
            light_index: roster as u32,
            baked,
            authored: entry.light,
            seed_axis: entry.source_index as u64,
            early_out: ProbeEarlyOut::for_probe_point(
                sources.animated_embedded.is_embedded(roster),
            ),
            policy,
        });
    }
    lights
}
