// Shadow tiers: the spot pool and the promoted and dynamic world-depth cache
// textures coupled to one spot shadow resolution, shared by every level set
// built at it, and the registry from resolution to a live tier.
// See: context/plans/done/level-load-pipeline/index.md (Per-load renderer resources) · context/lib/rendering_pipeline.md §4

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use super::dynamic_depth_cache::{
    DynamicCacheAllocation, DynamicCacheTexture, DynamicCacheTextures,
};
use super::promoted_depth_cache::PromotedDepthTextures;
use crate::lighting::spot_shadow::SpotShadowTextures;

/// Every GPU texture coupled to one spot shadow resolution: the live spot
/// pool's, and the promoted and dynamic world-depth caches', each cache
/// allocated the first time a set at this tier needs it. A named exception
/// to per-load ownership: level sets at one tier share it through `Arc`, it
/// outlives a release while any set (or the active pool) still holds it, and
/// a set at another tier builds its own, so a resident level's lights keep
/// sampling theirs.
pub(in crate::render) struct ShadowTier {
    pub(in crate::render) spot: Arc<SpotShadowTextures>,
    promoted: OnceLock<Arc<PromotedDepthTextures>>,
    dynamic_spot: OnceLock<Arc<DynamicCacheTexture>>,
    dynamic_cube: OnceLock<Arc<DynamicCacheTexture>>,
}

impl ShadowTier {
    pub(in crate::render) fn new(device: &wgpu::Device, resolution: u32) -> Self {
        Self {
            spot: Arc::new(SpotShadowTextures::new(device, resolution)),
            promoted: OnceLock::new(),
            dynamic_spot: OnceLock::new(),
            dynamic_cube: OnceLock::new(),
        }
    }

    pub(in crate::render) fn resolution(&self) -> u32 {
        self.spot.resolution
    }

    /// The tier's promoted world-depth cache textures, created on first use.
    pub(in crate::render) fn promoted(
        &self,
        device: &wgpu::Device,
        cube_array_supported: bool,
    ) -> Arc<PromotedDepthTextures> {
        self.promoted
            .get_or_init(|| {
                Arc::new(PromotedDepthTextures::new(
                    device,
                    cube_array_supported,
                    self.resolution(),
                ))
            })
            .clone()
    }

    /// The dynamic world-depth cache textures `allocation` asks for, each
    /// created on first use at this tier.
    pub(in crate::render) fn dynamic(
        &self,
        device: &wgpu::Device,
        allocation: DynamicCacheAllocation,
    ) -> DynamicCacheTextures {
        DynamicCacheTextures {
            spot: allocation.spot.then(|| {
                self.dynamic_spot
                    .get_or_init(|| Arc::new(DynamicCacheTexture::spot(device, self.resolution())))
                    .clone()
            }),
            cube: allocation.cube.then(|| {
                self.dynamic_cube
                    .get_or_init(|| Arc::new(DynamicCacheTexture::cube(device)))
                    .clone()
            }),
        }
    }
}

/// Resolution → weak handle to the tier built at it. A lookup at a
/// resolution whose tier is still held anywhere reuses it; otherwise it
/// builds a fresh one. Holds no tier alive itself.
pub(in crate::render) struct TierRegistry<T> {
    live: HashMap<u32, Weak<T>>,
}

impl<T> Default for TierRegistry<T> {
    fn default() -> Self {
        Self {
            live: HashMap::new(),
        }
    }
}

impl<T> TierRegistry<T> {
    /// The live tier at `resolution`, else a new one from `build`.
    pub(in crate::render) fn get_or_build(
        &mut self,
        resolution: u32,
        build: impl FnOnce() -> T,
    ) -> Arc<T> {
        self.live.retain(|_, tier| tier.strong_count() > 0);
        if let Some(tier) = self.live.get(&resolution).and_then(Weak::upgrade) {
            return tier;
        }
        let tier = Arc::new(build());
        self.live.insert(resolution, Arc::downgrade(&tier));
        tier
    }

    /// Record a tier built outside [`Self::get_or_build`] (the full-init pool).
    pub(in crate::render) fn register(&mut self, resolution: u32, tier: &Arc<T>) {
        self.live.insert(resolution, Arc::downgrade(tier));
    }

    /// The live tier at `resolution`, if anything still holds one.
    pub(in crate::render) fn live(&self, resolution: u32) -> Option<Arc<T>> {
        self.live.get(&resolution).and_then(Weak::upgrade)
    }

    /// Record a tier an off-main build created, unless a live tier at its
    /// resolution is already registered: that one stays, and `tier` stays
    /// private to the set that built it.
    pub(in crate::render) fn register_if_vacant(&mut self, resolution: u32, tier: &Arc<T>) {
        if self.live(resolution).is_none() {
            self.register(resolution, tier);
        }
    }
}
