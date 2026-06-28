// VM-free PODs slated for the foundation layer.
// See: context/lib/scripting.md §12 (Crate Architecture)

/// A unit of damage applied to an entity's `Health` component through
/// `apply_damage`. Consumers must not assume it stays amount-only — damage
/// types, resistances, and falloff are future fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DamagePayload {
    pub(crate) amount: f32,
}

/// The four canonical-agent parameters the navmesh was baked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavAgentParams {
    pub radius: f32,
    pub height: f32,
    pub step_height: f32,
    pub max_slope_deg: f32,
}

/// Validated map catalog entry exported by a mod manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModMapEntry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) tags: Vec<String>,
}
