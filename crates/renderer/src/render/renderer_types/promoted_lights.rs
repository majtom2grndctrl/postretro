// Baked-light promotion records: promoted shadow-pool slot budgets, record
// and per-light state types, and the animated-baked runtime-share seam.
// See: context/lib/rendering_pipeline.md §4

/// Fixed number of section-45 animated-baked lights that may crossfade into
/// the forward shadow-pool path. Pass B carries the matching `(1 - w)` values
/// in binding 26's fixed-size uniform; entries at and above this cap retain
/// their full baked delta and are never promotion candidates.
pub(crate) const MAX_ANIMATED_BAKED_LIGHTS: usize = 256;

/// First-guess promoted-slot budgets (cache VRAM ≈ 32 MiB spot + 12 MiB cube),
/// tuned on §10 target hardware. See the static-light-entity-shadows plan.
pub(crate) const MAX_PROMOTED_SPOT: usize = 8;
pub(crate) const MAX_PROMOTED_CUBE: usize = 2;

/// Which spot/cube static-depth cache and entity-only live pool a promoted
/// static light uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromotedShadowPoolKind {
    Spot,
    Cube,
}

/// The raw index namespace a promoted depth-cache record belongs to. The two
/// namespaces intentionally remain distinct: selected-static records key the
/// shadowmask/`EntityShadowLights` arrays, while section-45 animated records
/// key the `AnimatedBakedLights` roster and its compose-weight state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromotedBakedLightSource {
    SelectedStatic { selection_index: u32 },
    AnimatedBaked { animated_baked_index: u32 },
}

impl PromotedBakedLightSource {
    pub(crate) fn selected_static_index(self) -> Option<usize> {
        match self {
            Self::SelectedStatic { selection_index } => Some(selection_index as usize),
            Self::AnimatedBaked { .. } => None,
        }
    }

    pub(crate) fn animated_baked_index(self) -> Option<usize> {
        match self {
            Self::SelectedStatic { .. } => None,
            Self::AnimatedBaked {
                animated_baked_index,
            } => Some(animated_baked_index as usize),
        }
    }
}

/// One baked light promoted into a shadow pool slot this frame. Selected-static
/// records append a forward light record; animated-baked records instead point
/// at their already-reserved section-45 forward tail. Both use the same static
/// world-depth cache because their light projections are fixed for the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PromotedBakedLightRecord {
    /// Index into the level's full light array.
    pub global_light_index: u32,
    /// Which shadow pool (spot or cube) the light is promoted into.
    pub pool_kind: PromotedShadowPoolKind,
    /// Slot index within that pool.
    pub slot: u32,
    /// Promotion crossfade weight w ∈ [0,1] — 0 is fully baked SH, 1 is fully
    /// the runtime pool term (see rendering_pipeline.md §4 "Promoted static lights").
    pub weight: f32,
    /// Source variant carries the correctly typed raw roster index.
    pub source: PromotedBakedLightSource,
}

/// Per-candidate-light promotion tracking across frames: current weight,
/// sticky hold-over time, and which pool slot (if any) the light occupies.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PromotedBakedLightState {
    pub weight: f32,
    pub sticky_remaining: f32,
    pub pool_kind: Option<PromotedShadowPoolKind>,
    pub slot: u32,
    pub last_score: f32,
}

/// The one CPU seam for an animated-baked promotion's runtime share. Both the
/// forward tail encoder and binding-26's complementary compose array call this
/// against the same raw `AnimatedBakedLights` state entry. A detached state has
/// no shadow resource and therefore contributes no runtime share. Capped
/// overflow is never promoted and stays fully baked in Pass B.
pub(in crate::render) fn animated_baked_promotion_weight(
    animated_baked_index: usize,
    state: Option<&PromotedBakedLightState>,
) -> f32 {
    if animated_baked_index >= MAX_ANIMATED_BAKED_LIGHTS {
        return 0.0;
    }
    state
        .filter(|state| state.pool_kind.is_some())
        .map(|state| state.weight)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0)
}
