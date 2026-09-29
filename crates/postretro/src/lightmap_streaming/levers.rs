//! Runtime lightmap residency levers: pool cap and lead L (dev-tools sliders).
//! See: context/lib/rendering_pipeline.md §4 · context/lib/experimental_spikes.md

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// Default pool cap in 2048² layers. At the default lead it never grows on
/// the measured maps, and band retention repacks on 2.54% of tour steps.
pub(crate) const DEFAULT_POOL_CAP_LAYERS: u32 = 15;
/// Default lead L in metres.
pub(crate) const DEFAULT_LEAD_METRES: u32 = 16;
/// Id-51 leads are in id-46 fixed-point distance units.
pub(crate) const LEAD_UNITS_PER_METRE: u32 = CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;

/// Developer levers, not a player setting. Changing either takes effect at
/// the next frame: a lead change recomputes demand from the baked set, and
/// every drain batch carries the current cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LightmapLevers {
    pool_cap_layers: u32,
    /// Lead L, fixed point; never past `max_lead`.
    lead: u32,
    /// The level's baked maximum lead (id-51 header).
    max_lead: u32,
}

impl LightmapLevers {
    pub(crate) fn new(max_lead: u32) -> Self {
        Self {
            pool_cap_layers: DEFAULT_POOL_CAP_LAYERS,
            lead: (DEFAULT_LEAD_METRES * LEAD_UNITS_PER_METRE).min(max_lead),
            max_lead,
        }
    }

    pub(crate) fn pool_cap_layers(&self) -> u32 {
        self.pool_cap_layers
    }

    /// Lead L in id-46 fixed-point units.
    pub(crate) fn lead(&self) -> u32 {
        self.lead
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the Task 11 lead slider spans 0..=max")
    )]
    pub(crate) fn max_lead(&self) -> u32 {
        self.max_lead
    }

    /// At least one layer: a zero cap would refuse every band block while
    /// mandatory work grows the pool anyway.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the Task 11 pool-cap slider writes layers")
    )]
    pub(crate) fn set_pool_cap_layers(&mut self, layers: u32) {
        self.pool_cap_layers = layers.max(1);
    }

    /// Clamped to the baked maximum: the set holds no entry past it.
    pub(crate) fn set_lead(&mut self, lead: u32) {
        self.lead = lead.min(self.max_lead);
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the Task 11 lead slider writes metres")
    )]
    pub(crate) fn set_lead_metres(&mut self, metres: f32) {
        let units = (metres.max(0.0) * LEAD_UNITS_PER_METRE as f32).round();
        self.set_lead(if units >= u32::MAX as f32 {
            u32::MAX
        } else {
            units as u32
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levers_default_to_sixteen_metres_and_fifteen_layers_within_the_baked_maximum() {
        let levers = LightmapLevers::new(32 * LEAD_UNITS_PER_METRE);
        assert_eq!(levers.pool_cap_layers(), 15);
        assert_eq!(levers.lead(), 16 * LEAD_UNITS_PER_METRE);

        let short = LightmapLevers::new(8 * LEAD_UNITS_PER_METRE);
        assert_eq!(short.lead(), 8 * LEAD_UNITS_PER_METRE, "clamped to max");
    }

    #[test]
    fn lead_lever_clamps_to_the_baked_maximum_and_cap_to_one_layer() {
        let mut levers = LightmapLevers::new(32 * LEAD_UNITS_PER_METRE);
        levers.set_lead_metres(40.0);
        assert_eq!(levers.lead(), levers.max_lead());
        levers.set_lead_metres(-3.0);
        assert_eq!(levers.lead(), 0);
        levers.set_lead_metres(2.5);
        assert_eq!(levers.lead(), 2560);
        levers.set_pool_cap_layers(0);
        assert_eq!(levers.pool_cap_layers(), 1);
    }
}
