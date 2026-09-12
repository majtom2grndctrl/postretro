// VM-free PODs slated for the foundation layer.
// See: context/lib/scripting.md §12 (Crate Architecture)

/// Hit effects resolved by combat: health damage and an independent velocity
/// change in metres per second. Attribution travels beside these effects.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DamagePayload {
    pub amount: f32,
    pub impulse: glam::Vec3,
}

/// Resolve authored knockback along a shot or blast direction. At coincident
/// centers (or a cancelling downward/upward blend), launch upward consistently.
pub fn knockback_impulse(speed: f32, upward_bias: f32, direction: glam::Vec3) -> glam::Vec3 {
    use glam::Vec3;
    if !speed.is_finite()
        || !(0.0..=1000.0).contains(&speed)
        || !upward_bias.is_finite()
        || !(0.0..=1.0).contains(&upward_bias)
        || !direction.is_finite()
    {
        return Vec3::ZERO;
    }
    let direction = direction.try_normalize().unwrap_or(Vec3::Y);
    direction
        .lerp(Vec3::Y, upward_bias)
        .try_normalize()
        .unwrap_or(Vec3::Y)
        * speed
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
pub struct ModMapEntry {
    pub id: String,
    pub path: String,
    pub name: String,
    pub tags: Vec<String>,
}
