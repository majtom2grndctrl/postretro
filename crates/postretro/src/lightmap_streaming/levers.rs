//! Runtime lightmap residency lever: the pool cap (a dev-tools slider). Lead L
//! is the level's, on the cell-demand stage.
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md

// The default pool cap in 2048² layers is the renderer's, which sizes the
// first pool generation with it.
use postretro_renderer::DEFAULT_LIGHTMAP_POOL_CAP_LAYERS as DEFAULT_POOL_CAP_LAYERS;

/// Developer lever, not a player setting. Every drain batch carries the
/// current cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LightmapLevers {
    pool_cap_layers: u32,
}

impl LightmapLevers {
    pub(crate) fn new() -> Self {
        Self {
            pool_cap_layers: DEFAULT_POOL_CAP_LAYERS,
        }
    }

    pub(crate) fn pool_cap_layers(&self) -> u32 {
        self.pool_cap_layers
    }

    /// At least one layer: a zero cap would refuse every band block while
    /// mandatory work grows the pool anyway.
    #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
    pub(crate) fn set_pool_cap_layers(&mut self, layers: u32) {
        self.pool_cap_layers = layers.max(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_cap_defaults_to_fifteen_layers_and_clamps_to_one() {
        let mut levers = LightmapLevers::new();
        assert_eq!(levers.pool_cap_layers(), 15);
        levers.set_pool_cap_layers(0);
        assert_eq!(levers.pool_cap_layers(), 1);
    }
}
