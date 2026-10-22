// Ghost transparency mode: how every ghost draw is made translucent,
// mod-wide (`render.ghosts`). Renderer vocabulary; the app maps the mod
// profile onto it at one chokepoint.
// See: context/lib/rendering_pipeline.md §7.9

/// The author's ghost transparency, `render.ghosts`. The ghost passes build a
/// pipeline per mode and batch ghosts by it; a projectile billboard ghost
/// blends under both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum GhostTransparency {
    /// Alpha blend at the presented opacity, no depth write. The default, and
    /// frame capture's unless its scene says otherwise.
    #[default]
    Blend,
    /// Screen-door: blend off, a fragment discard against a screen-space
    /// ordered pattern keeps about the presented opacity's share of pixels,
    /// still with no depth write.
    Dither,
}

impl GhostTransparency {
    /// Every mode.
    pub const ALL: [Self; 2] = [Self::Blend, Self::Dither];
}
