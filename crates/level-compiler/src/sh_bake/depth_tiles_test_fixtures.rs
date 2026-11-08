// Test fixture: a dev map's production SH base bake, depth tiles included, with its probe layout.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::fixture_pipeline::{FixturePipeline, load_fixture};
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::sh_bake::{ProbeGridLayout, ShBakeCtx, ShConfig, probe_grid_layout, static_light_refs};
use crate::surface_radiance::{DEFAULT_BOUNCE_FILL, ShBase, bake_sh_base, plan_sh_base};
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

/// A dev map through the compile pipeline up to its BVH, ready to bake SH
/// base at 1 m spacing with grey bounce, as a map without textures does.
pub(crate) struct DepthTileFixture {
    pub(crate) pipeline: FixturePipeline,
}

impl DepthTileFixture {
    pub(crate) fn load(name: &str) -> Self {
        Self {
            pipeline: load_fixture(name),
        }
    }

    /// Run `f` over the map's SH bake context.
    pub(crate) fn with_ctx<R>(&self, f: impl FnOnce(&ShBakeCtx<'_>) -> R) -> R {
        let p = &self.pipeline;
        let static_lights = StaticBakedLights::from_lights(&p.lights);
        let animated_lights = AnimatedBakedLights::from_lights(&p.lights);
        f(&ShBakeCtx {
            bounce_materials: None,
            bvh: &p.bvh,
            primitives: &p.primitives,
            geometry: &p.geometry,
            tree: &p.tree,
            exterior_leaves: &p.exterior_leaves,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: p.lights.len(),
        })
    }

    pub(crate) fn config() -> ShConfig {
        ShConfig { probe_spacing: 1.0 }
    }

    pub(crate) fn layout(&self) -> ProbeGridLayout {
        self.with_ctx(|ctx| probe_grid_layout(ctx, &Self::config()))
    }

    /// The production SH base stage, as the pipeline calls it, keeping at
    /// most `tile_cap` tiles. Runs on the calling thread, so its log lines
    /// reach a capture started there.
    pub(crate) fn bake(&self, tile_cap: u32, cache: Option<&StageCache>) -> ShBase {
        self.with_ctx(|ctx| {
            let (handoff, _) = plan_sh_base(
                &self.pipeline.geometry,
                &static_light_refs(ctx),
                0,
                Self::config().probe_spacing,
                DEFAULT_BOUNCE_FILL,
                u64::MAX,
            )
            .expect("the SH base gate admits the fixture");
            bake_sh_base(
                ctx,
                &Self::config(),
                handoff,
                &NO_EMBEDDED_EMITTERS,
                cache,
                &BakeControl::unrestricted(),
                tile_cap,
            )
        })
    }
}
