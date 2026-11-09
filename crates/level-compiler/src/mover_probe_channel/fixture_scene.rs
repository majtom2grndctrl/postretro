// A dev map's tracing scene and probe grid for the direct half's
// fixture tests: the map's lights gated as static lights and baked by the
// production direct bake.

use super::direct::{ChannelLight, DirectScene, MoverDirectBake, ProbeCells, bake_direct};
use crate::affinity_grid::{AffinityReachInputs, AffinityReachPolicy};
use crate::bake_control::BakeControl;
use crate::fixture_pipeline::FixturePipeline;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::mover_light_gate::MoverPoses;
use crate::sh_bake::{
    ProbeGridLayout, RaytracingCtx, ShBakeCtx, ShConfig, bake_probe_direct_rgb_posed,
    probe_grid_layout, vec3_from,
};
use crate::texel_footprint::ProbeEarlyOut;
use postretro_level_format::mover_probe_channel::MoverLightNamespace;

/// One build's tracing scene and probe grid.
pub(super) struct Scene {
    pub(super) fixture: FixturePipeline,
    pub(super) layout: ProbeGridLayout,
}

impl Scene {
    pub(super) fn of(fixture: FixturePipeline) -> Self {
        let static_lights = StaticBakedLights::from_lights(&fixture.lights);
        let animated_lights = AnimatedBakedLights::from_lights(&fixture.lights);
        let layout = probe_grid_layout(
            &ShBakeCtx {
                bounce_materials: None,
                bvh: &fixture.bvh,
                primitives: &fixture.primitives,
                geometry: &fixture.geometry,
                tree: &fixture.tree,
                exterior_leaves: &fixture.exterior_leaves,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: fixture.lights.len(),
            },
            &ShConfig { probe_spacing: 1.0 },
        );
        Self { fixture, layout }
    }

    pub(super) fn ctx(&self) -> RaytracingCtx<'_> {
        RaytracingCtx {
            bounce_materials: None,
            bvh: &self.fixture.bvh,
            primitives: &self.fixture.primitives,
            geometry: &self.fixture.geometry,
        }
    }

    /// `light`'s static term at `probe`, under seed axis `seed_axis`.
    pub(super) fn term(&self, light: &ChannelLight<'_>, probe: usize) -> [f32; 27] {
        bake_probe_direct_rgb_posed(
            &self.ctx(),
            vec3_from(self.layout.probe_positions[probe]),
            &[&light.baked],
            &[light.seed_axis],
            &[light.early_out],
            probe as u64,
            None,
        )
    }

    pub(super) fn poses(&self) -> MoverPoses {
        MoverPoses::resolve(
            &self.fixture.kinematic_movers,
            &self.fixture.kinematic_waypoints,
        )
    }

    /// Every light of the map gated as a static light, its index its seed
    /// axis: the channel's q and a term traced here share one sample set.
    pub(super) fn static_channel_lights(&self) -> Vec<ChannelLight<'_>> {
        self.fixture
            .lights
            .iter()
            .enumerate()
            .map(|(index, light)| ChannelLight {
                namespace: MoverLightNamespace::StaticLight,
                light_index: index as u32,
                baked: light.clone(),
                authored: light,
                seed_axis: index as u64,
                early_out: ProbeEarlyOut::Agreed,
                policy: AffinityReachPolicy::DIRECT,
            })
            .collect()
    }

    pub(super) fn bake(&self, lights: &[ChannelLight<'_>], cells: &ProbeCells) -> MoverDirectBake {
        let vertices: Vec<[f32; 3]> = self
            .fixture
            .geometry
            .geometry
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .collect();
        let poses = self.poses();
        let scene = DirectScene {
            ctx: self.ctx(),
            layout: &self.layout,
            cells,
            reach: AffinityReachInputs {
                geometry_vertices: &vertices,
                tree: &self.fixture.tree,
                exterior_leaves: &self.fixture.exterior_leaves,
                portals: &self.fixture.portals,
                probe_spacing: 1.0,
            },
        };
        bake_direct(&poses, lights, &scene, None, &BakeControl::unrestricted())
            .expect("direct bake")
    }
}
