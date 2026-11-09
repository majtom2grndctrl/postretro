//! The mover probe channel at its pipeline seam: the direct half's q bake
//! over the gated light set, behind its per-light memo, and id 57's assembly
//! once ids 41 and 45 are final.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::mover_probe_channel::{MoverLightNamespace, MoverProbeChannelSection};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::affinity_grid::AffinityReachInputs;
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::delta_sections::PostBakeDeltaSections;
use crate::governor::Governor;
use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights};
use crate::map_data::MapKinematicMover;
use crate::mover_light_gate::MoverPoses;
use crate::mover_probe_channel::{
    ChannelLightSources, DirectScene, EmittedDirectEntries, MoverDirectBake, MoverRamps,
    ProbeCells, bake_direct, channel_lights, channel_section,
};
use crate::portals::Portal;
use crate::reporter::StageProgress;
use crate::sh_bake::{self, RaytracingCtx, ShBakeCtx, ShConfig};
use crate::texel_footprint::EmbeddedEmitters;

/// What the direct half reads from the pipeline.
pub(super) struct DirectHalfInputs<'a> {
    pub(super) poses: &'a MoverPoses,
    pub(super) movers: &'a [MapKinematicMover],
    pub(super) sh_ctx: &'a ShBakeCtx<'a>,
    pub(super) sh_config: &'a ShConfig,
    pub(super) portals: &'a [Portal],
    pub(super) alpha_lights: &'a AlphaLightsNs<'a>,
    pub(super) animated_lights: &'a AnimatedBakedLights<'a>,
    /// Keyed by position in `sh_ctx.static_lights`, as id 35 reads it.
    pub(super) static_embedded: &'a EmbeddedEmitters,
    /// Keyed by position in `animated_lights`, as id 45 reads it.
    pub(super) animated_embedded: &'a EmbeddedEmitters,
    pub(super) mutable_descriptors: &'a ScriptMutableDescriptorSlots,
    pub(super) cache: Option<&'a StageCache>,
    pub(super) governor: &'a Arc<Governor>,
}

/// The direct half's bake and the grid its section is laid on.
pub(super) struct DirectHalf {
    bake: MoverDirectBake,
    cells: ProbeCells,
    ramps: MoverRamps,
}

/// Bake the direct half: per gated light, the entries of every analysed
/// mover its samples meet. `None` when no mover is analysed or the probe
/// grid is empty: then no light needs gating, and no memo is read.
pub(super) fn bake_direct_half(inputs: DirectHalfInputs<'_>) -> anyhow::Result<Option<DirectHalf>> {
    if inputs.poses.analysed.is_empty() {
        return Ok(None);
    }
    let layout = sh_bake::probe_grid_layout(inputs.sh_ctx, inputs.sh_config);
    if layout.is_empty() {
        return Ok(None);
    }
    let started = Instant::now();
    let lights = channel_lights(&ChannelLightSources {
        sh_ctx: inputs.sh_ctx,
        alpha_lights: inputs.alpha_lights,
        animated_lights: inputs.animated_lights,
        static_embedded: inputs.static_embedded,
        animated_embedded: inputs.animated_embedded,
        mutable_descriptors: inputs.mutable_descriptors,
    });
    let cells = ProbeCells::of_layout(&layout);
    let geometry_vertices: Vec<[f32; 3]> = inputs
        .sh_ctx
        .geometry
        .geometry
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect();
    let scene = DirectScene {
        ctx: RaytracingCtx {
            // Direct visibility rays never bounce, so they stay colour-blind.
            bounce_materials: None,
            bvh: inputs.sh_ctx.bvh,
            primitives: inputs.sh_ctx.primitives,
            geometry: inputs.sh_ctx.geometry,
        },
        layout: &layout,
        cells: &cells,
        reach: AffinityReachInputs {
            geometry_vertices: &geometry_vertices,
            tree: inputs.sh_ctx.tree,
            exterior_leaves: inputs.sh_ctx.exterior_leaves,
            portals: inputs.portals,
            probe_spacing: inputs.sh_config.probe_spacing,
        },
    };
    let control = BakeControl::new(Arc::clone(inputs.governor), &StageProgress::indeterminate());
    let bake = bake_direct(inputs.poses, &lights, &scene, inputs.cache, &control)?;
    let bake_only =
        |namespace: MoverLightNamespace| namespace == MoverLightNamespace::BakeOnlyStaticLight;
    log::info!(
        "[MoverLight] probe channel direct half: {} entr(y/ies) over {} of {} gated light(s) \
         ({} of {} bake-only), {:.1} ms",
        bake.entry_count(),
        bake.lights.len(),
        lights.len(),
        bake.lights
            .iter()
            .filter(|light| bake_only(light.namespace))
            .count(),
        lights
            .iter()
            .filter(|light| bake_only(light.namespace))
            .count(),
        started.elapsed().as_secs_f64() * 1000.0,
    );
    Ok(Some(DirectHalf {
        bake,
        cells,
        ramps: MoverRamps::new(inputs.movers),
    }))
}

impl DirectHalf {
    /// Id 57 with every optional own-tile flag clear (bake-only entries
    /// always carry theirs): what the storage classifiers read, before ids
    /// 41 and 45 are final. `None` with no entry. The
    /// grid must be id 34's.
    pub(super) fn provisional_section(
        &self,
        base: &OctahedralShVolumeSection,
    ) -> anyhow::Result<Option<MoverProbeChannelSection>> {
        self.check_grid(base)?;
        channel_section(&self.bake, &self.cells, &self.ramps, |_, _, _| false)
    }

    /// Settle id 57's own-tile flags against ids 41 and 45 as emitted:
    /// after the exact-zero drop, id 41's classification under the channel's
    /// cap, and valid-probe compaction (P22).
    pub(super) fn finish_section(
        &self,
        sections: &mut PostBakeDeltaSections,
    ) -> anyhow::Result<()> {
        let emitted = EmittedDirectEntries {
            selection: sections.entity_shadow_lights.as_ref(),
            direct: sections.direct.as_ref(),
            animated: sections.animated_direct.as_ref(),
        };
        let section = channel_section(&self.bake, &self.cells, &self.ramps, |cell, ns, light| {
            !emitted.holds_per_probe(cell, ns, light)
        })?;
        if let Some(section) = &section {
            let own = section
                .direct_entries
                .iter()
                .filter(|entry| entry.carries_own_tile)
                .count();
            let bake_only = section
                .direct_entries
                .iter()
                .filter(|entry| entry.namespace == MoverLightNamespace::BakeOnlyStaticLight)
                .count();
            log::info!(
                "[MoverLight] MoverProbeChannel (id 57): {} direct entr(y/ies), {own} carrying \
                 their own tile, {bake_only} naming {} bake-only light(s), over {} mover(s); \
                 direct block {} B: {} B of entry records, {} B of visibility, {} B of own tiles",
                section.direct_entries.len(),
                section.bake_only_light_count(),
                section.ramps.len(),
                direct_block_bytes(section),
                section.direct_entries.len() * DIRECT_ENTRY_BYTES,
                section.visibility.len(),
                section.own_tiles.len() * 2,
            );
        }
        sections.mover_channel = section;
        Ok(())
    }

    fn check_grid(&self, base: &OctahedralShVolumeSection) -> anyhow::Result<()> {
        let base_cells = ProbeCells::new(base.grid_dimensions, |probe| {
            base.probes[probe].validity != 0
        });
        anyhow::ensure!(
            base_cells == self.cells,
            "mover probe channel: the direct bake's probe grid disagrees with id 34's"
        );
        Ok(())
    }
}

/// One direct entry record's wire bytes (`level-format` pins the layout).
const DIRECT_ENTRY_BYTES: usize = 12;

/// The direct block's wire bytes: entry records, visibility and own tiles.
fn direct_block_bytes(section: &MoverProbeChannelSection) -> usize {
    section.direct_entries.len() * DIRECT_ENTRY_BYTES
        + section.visibility.len()
        + section.own_tiles.len() * 2
}
