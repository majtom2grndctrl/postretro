// Impact dispatch vocabulary: the IR inputs and command-target tokens every
// damage chokepoint fire publishes to impact policies.
//
// See: context/lib/scripting.md §11 (Impact policies)

use glam::Vec3;

use super::health::DamageProducer;
use crate::registry::EntityId;
use postretro_foundation::{IrType, IrValue};

/// Exact numeric IR vocabulary published by every impact dispatch.
///
/// The command-target tokens are intentionally absent: the IR only carries
/// numbers and booleans, while entity references travel on a separate channel.
pub const IMPACT_AMOUNT_INPUT: &str = "@impact.amount";
pub const IMPACT_HEALTH_BEFORE_INPUT: &str = "@impact.healthBefore";
pub const IMPACT_HEALTH_AFTER_INPUT: &str = "@impact.healthAfter";
pub const IMPACT_MAX_HEALTH_INPUT: &str = "@impact.maxHealth";

pub const IMPACT_DISPATCH_INPUTS: [(&str, IrType); 4] = [
    (IMPACT_AMOUNT_INPUT, IrType::Number),
    (IMPACT_HEALTH_BEFORE_INPUT, IrType::Number),
    (IMPACT_HEALTH_AFTER_INPUT, IrType::Number),
    (IMPACT_MAX_HEALTH_INPUT, IrType::Number),
];

/// Exact command-target token for the damaged entity in an impact dispatch.
pub const IMPACT_TARGET_TOKEN: &str = "@impact.target";

/// Exact command-target token for the attacker in an impact dispatch.
pub const IMPACT_SOURCE_TOKEN: &str = "@impact.source";

/// One observable fire of the damage chokepoint.
///
/// Numeric fields bind through [`IMPACT_DISPATCH_INPUTS`]. `target` and
/// `source` are opaque command-target values keyed by [`IMPACT_TARGET_TOKEN`]
/// and [`IMPACT_SOURCE_TOKEN`], respectively; they are never IR leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactDispatch {
    /// Points dealt: the total drained from layers and health after type
    /// multipliers and resistances. Health's share is unfloored.
    pub amount: f32,
    /// The incoming amount after hit zones, before type and layers.
    pub raw: f32,
    /// The layers' share of `amount`.
    pub absorbed: f32,
    /// This hit took some layer from above zero to zero.
    pub broke: bool,
    pub health_before: f32,
    /// The raw post-subtraction value before the stored health is floored.
    pub health_after: f32,
    pub max_health: f32,
    pub target: EntityId,
    pub source: Option<EntityId>,
    pub producer: DamageProducer,
    /// World-space contact point carried from [`DamageContext::point`].
    /// Evaluator-only: never an IR input, so [`Self::ir_values`] omits it.
    pub point: Option<Vec3>,
}

impl ImpactDispatch {
    /// Values for the numeric half of the impact dispatch scope, ordered by
    /// the fixed vocabulary so a later consumer can seed a bound scope directly.
    pub fn ir_values(&self) -> [(&'static str, IrValue); 4] {
        [
            (IMPACT_AMOUNT_INPUT, IrValue::Number(self.amount)),
            (
                IMPACT_HEALTH_BEFORE_INPUT,
                IrValue::Number(self.health_before),
            ),
            (
                IMPACT_HEALTH_AFTER_INPUT,
                IrValue::Number(self.health_after),
            ),
            (IMPACT_MAX_HEALTH_INPUT, IrValue::Number(self.max_health)),
        ]
    }
}
