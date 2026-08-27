// Data-context descriptors: weapon/health/ai descriptors.
// See: context/lib/scripting.md

use std::collections::HashMap;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::data_descriptors::types::light::FalloffKind;
use crate::data_descriptors::{
    DescriptorError, is_portable_content_relative_asset_path, validate_ascii_identifier,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FireMode {
    Semi,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResolutionMode {
    Hitscan,
    Projectile,
}

/// Descriptor-owned tuning for a straight-line direct-impact projectile.
///
/// This stays in the foundation descriptor because it is pure authored data;
/// the entities crate materializes the flight state and the game layer owns the
/// collision and damage resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileDescriptor {
    /// Straight-line launch speed in metres per second.
    pub speed: f32,
    /// Swept-sphere radius in metres.
    pub radius: f32,
    /// Maximum flight time in milliseconds. Weapon range remains a second cap.
    #[serde(rename = "lifetimeMs")]
    pub lifetime_ms: f32,
    pub visual: ProjectileVisual,
}

/// Presentation attached to a projectile at spawn. It is descriptor data only:
/// rendering never decides whether the projectile hits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileVisual {
    pub body: ProjectileBodyVisual,
    #[serde(default)]
    pub trail: Option<ProjectileTrailVisual>,
    /// Optional dynamic point light materialized with the projectile body.
    /// Descriptor content is shared between peers; this is never a wire field.
    #[serde(default)]
    pub light: Option<ProjectileLight>,
    /// Optional stationary light flash spawned when the projectile contacts a
    /// surface or target. This is shared descriptor content, never a wire field.
    #[serde(default)]
    pub impact_light: Option<ProjectileImpactLight>,
}

/// Descriptor-owned dynamic point light attached to a travelling projectile.
/// The point shape is fixed by the projectile presentation path; only its
/// radiance and attenuation are author-configurable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub falloff_range: f32,
    #[serde(default = "default_projectile_light_falloff_model")]
    pub falloff_model: FalloffKind,
}

/// Descriptor-owned transient point light spawned at a projectile contact.
/// A peak radius turns the fade into an expanding shockwave; omitting it keeps
/// the authored radius static for the whole flash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileImpactLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32,
    #[serde(default)]
    pub peak_radius: Option<f32>,
    pub fade_ms: f32,
}

const fn default_projectile_light_falloff_model() -> FalloffKind {
    FalloffKind::InverseSquared
}

/// The projectile's visible body. A mesh body is rigid; no animation state is
/// attached to a travelling projectile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectileBodyVisual {
    Sprite {
        sprite: String,
        #[serde(default = "default_projectile_sprite_size")]
        size: f32,
        #[serde(default = "default_projectile_sprite_opacity")]
        opacity: f32,
        #[serde(default)]
        rotation: f32,
        #[serde(default = "default_projectile_sprite_tint")]
        tint: [f32; 3],
        /// Additive self-lit strength for this sprite collection. Zero keeps
        /// the billboard output on its existing scene-lit path.
        #[serde(default)]
        emissive: f32,
        /// Uniform hold duration for each numbered collection frame. Omission
        /// keeps the body pinned to frame zero, even for a multi-frame source.
        #[serde(default, rename = "frameDurationMs")]
        frame_duration_ms: Option<f32>,
    },
    Model {
        model: String,
    },
}

/// Optional descriptor-owned billboard-emitter trail. Defaults make the short
/// form `{ sprite: "sprites/trail.png" }` useful while retaining the existing
/// emitter component's complete presentation controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileTrailVisual {
    pub sprite: String,
    #[serde(default = "default_projectile_trail_rate")]
    pub rate: f32,
    #[serde(default = "default_projectile_trail_lifetime")]
    pub lifetime: f32,
    #[serde(default)]
    pub burst: Option<u32>,
    #[serde(default)]
    pub spread: f32,
    #[serde(default)]
    pub velocity: [f32; 3],
    #[serde(default)]
    pub buoyancy: f32,
    #[serde(default)]
    pub drag: f32,
    #[serde(default = "default_projectile_trail_size_curve")]
    pub size_over_lifetime: Vec<f32>,
    #[serde(default = "default_projectile_trail_opacity_curve")]
    pub opacity_over_lifetime: Vec<f32>,
    #[serde(default = "default_projectile_sprite_tint")]
    pub color: [f32; 3],
    #[serde(default)]
    pub spin_rate: f32,
    #[serde(default)]
    pub spin_animation: Option<ProjectileTrailSpinAnimation>,
}

/// Optional spin-rate tween for a projectile trail emitter. Projectile
/// descriptors use camelCase authoring while the stored emitter component uses
/// the equivalent engine-side shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectileTrailSpinAnimation {
    pub duration: f32,
    pub rate_curve: Vec<f32>,
}

const fn default_projectile_sprite_size() -> f32 {
    0.35
}

const fn default_projectile_sprite_opacity() -> f32 {
    1.0
}

const fn default_projectile_sprite_tint() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

const fn default_projectile_trail_rate() -> f32 {
    30.0
}

const fn default_projectile_trail_lifetime() -> f32 {
    0.4
}

fn default_projectile_trail_size_curve() -> Vec<f32> {
    vec![0.2, 0.12, 0.0]
}

fn default_projectile_trail_opacity_curve() -> Vec<f32> {
    vec![0.8, 0.45, 0.0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReloadStyle {
    Magazine,
    PerShell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WeaponResource {
    Ammo(AmmoResource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmmoResource {
    #[serde(rename = "type")]
    pub ammo_type: String,
    pub magazine: u32,
    #[serde(default = "default_cost_per_shot", rename = "costPerShot")]
    pub cost_per_shot: u32,
    pub reserve: u32,
    #[serde(default = "default_reload_ms", rename = "reloadMs")]
    pub reload_ms: u32,
    #[serde(default = "default_reload_style", rename = "reloadStyle")]
    pub reload_style: ReloadStyle,
}

const fn default_cost_per_shot() -> u32 {
    1
}

const fn default_reload_ms() -> u32 {
    1000
}

const fn default_reload_style() -> ReloadStyle {
    ReloadStyle::Magazine
}

/// Hard upper bound for authored weapon pellets per shell.
pub const MAX_PELLET_COUNT: u32 = 32;

const fn default_pellet_count() -> u32 {
    1
}

/// Authored first-person placement relative to the camera's screen center.
///
/// The labels stay author-facing: the render seam maps `right`, `up`, and
/// `forward` to camera-space +X, +Y, and -Z respectively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeaponPlacementDescriptor {
    #[serde(default, rename = "positionFromCenter")]
    pub offset: PlacementOffset,
    #[serde(default)]
    pub rotation: PlacementRotation,
}

/// First-person position offset in metres from the screen center.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlacementOffset {
    #[serde(default)]
    pub right: f32,
    #[serde(default)]
    pub up: f32,
    #[serde(default)]
    pub forward: f32,
}

/// First-person orientation offset in degrees about the camera origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlacementRotation {
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub pitch: f32,
    #[serde(default)]
    pub roll: f32,
}

impl WeaponPlacementDescriptor {
    /// Convert author-facing placement into the viewmodel camera-space frame.
    /// Camera space is right-handed with -Z forward, matching the renderer.
    pub fn camera_space(&self) -> (Vec3, Quat) {
        let offset = Vec3::new(self.offset.right, self.offset.up, -self.offset.forward);
        let rotation = Quat::from_rotation_y(self.rotation.yaw.to_radians())
            * Quat::from_rotation_x(self.rotation.pitch.to_radians())
            * Quat::from_rotation_z(self.rotation.roll.to_radians());
        (offset, rotation)
    }

    pub fn validate(&self) -> Result<(), DescriptorError> {
        for (field, value) in [
            ("positionFromCenter.right", self.offset.right),
            ("positionFromCenter.up", self.offset.up),
            ("positionFromCenter.forward", self.offset.forward),
            ("rotation.yaw", self.rotation.yaw),
            ("rotation.pitch", self.rotation.pitch),
            ("rotation.roll", self.rotation.roll),
        ] {
            if !value.is_finite() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.placement.{field}` must be a finite value, got {value}"
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Authored weapon component preset. This is descriptor-owned tuning data:
/// maps do not override these params, and the runtime materializes a separate
/// wieldable instance entity from the descriptor at player spawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponDescriptor {
    pub damage: f32,
    #[serde(default = "default_pellet_count")]
    pub pellet_count: u32,
    #[serde(default)]
    pub spread_degrees: f32,
    pub range: f32,
    #[serde(rename = "fireRateMs")]
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    /// Required when `resolution` is `Projectile`; omitted by all other
    /// resolution modes so existing hitscan descriptors remain unchanged.
    #[serde(default)]
    pub projectile: Option<ProjectileDescriptor>,
    #[serde(default, rename = "creditSource")]
    pub credit_source: Option<String>,
    /// Optional content-relative rigid prop model mounted at the pawn's third-person hand socket.
    /// Uses forward slashes and may not be absolute or contain parent traversal.
    #[serde(default, rename = "thirdPersonModel")]
    pub third_person_model: Option<String>,
    /// Optional content-relative model rendered by the first-person viewmodel pass.
    /// Uses forward slashes and may not be absolute or contain parent traversal.
    #[serde(default)]
    pub viewmodel: Option<String>,
    /// Optional per-weapon first-person placement. Whole-value resolution is
    /// per-instance (future), per-weapon, character (future), mod default, then
    /// the legacy `BASE_OFFSET` with zero rotation. v1 supplies `None` for the
    /// future character and per-instance tiers.
    #[serde(default)]
    pub placement: Option<WeaponPlacementDescriptor>,
    /// Optional model-local projectile origin in metres. This composes through
    /// the resolved steady first-person placement at fire time.
    #[serde(default)]
    pub muzzle_offset: Option<[f32; 3]>,
    #[serde(default)]
    pub resource: Option<WeaponResource>,
    #[serde(default, rename = "lowerMs")]
    pub lower_ms: u32,
    #[serde(default, rename = "raiseMs")]
    pub raise_ms: u32,
    /// Optional override of the mod-global reload-interrupt policy. Resolution
    /// belongs to the commit gate, so the component retains this unresolved.
    #[serde(default, rename = "blockDuringReload")]
    pub block_during_reload: Option<bool>,
}

impl WeaponDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.damage.is_finite() || self.damage < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.damage` must be a finite value >= 0.0, got {}",
                    self.damage
                ),
            });
        }
        if !(1..=MAX_PELLET_COUNT).contains(&self.pellet_count) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.pelletCount` must be in 1..={MAX_PELLET_COUNT}, got {}",
                    self.pellet_count
                ),
            });
        }
        if !self.spread_degrees.is_finite() || !(0.0..=45.0).contains(&self.spread_degrees) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.spreadDegrees` must be a finite value in 0.0..=45.0, got {}",
                    self.spread_degrees
                ),
            });
        }
        if !self.range.is_finite() || self.range <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.range` must be a finite value > 0.0, got {}",
                    self.range
                ),
            });
        }
        if !self.cooldown_ms.is_finite() || self.cooldown_ms <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.fireRateMs` must be a finite value > 0.0, got {}",
                    self.cooldown_ms
                ),
            });
        }
        match (self.resolution, self.projectile.as_ref()) {
            (ResolutionMode::Projectile, Some(projectile)) => {
                if self.pellet_count != 1 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.pelletCount` must be exactly 1 when `components.weapon.resolution` is `projectile`, got {}",
                            self.pellet_count
                        ),
                    });
                }
                validate_projectile_descriptor(projectile)?;
            }
            (ResolutionMode::Projectile, None) => {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.weapon.projectile` is required when `components.weapon.resolution` is `projectile`".to_string(),
                });
            }
            (ResolutionMode::Hitscan, Some(_)) => {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.weapon.projectile` must be omitted when `components.weapon.resolution` is `hitscan`".to_string(),
                });
            }
            (ResolutionMode::Hitscan, None) => {}
        }
        if let Some(credit_source) = self.credit_source.as_deref() {
            validate_credit_source(credit_source)?;
        }
        if let Some(placement) = self.placement.as_ref() {
            placement.validate()?;
        }
        if let Some(muzzle_offset) = self.muzzle_offset {
            for (index, component) in muzzle_offset.into_iter().enumerate() {
                if !component.is_finite() {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.muzzleOffset[{index}]` must be a finite value, got {component}"
                        ),
                    });
                }
            }
        }
        for (field, path) in [
            ("thirdPersonModel", self.third_person_model.as_deref()),
            ("viewmodel", self.viewmodel.as_deref()),
        ] {
            if let Some(path) = path
                && !is_portable_content_relative_asset_path(path)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.{field}` must be a non-empty, content-relative model path using forward slashes with no parent traversal"
                    ),
                });
            }
        }
        if let Some(WeaponResource::Ammo(ammo)) = self.resource.as_ref() {
            validate_ascii_identifier("components.weapon.resource.type", &ammo.ammo_type)?;
            for (field, value) in [
                ("magazine", ammo.magazine),
                ("costPerShot", ammo.cost_per_shot),
                ("reloadMs", ammo.reload_ms),
            ] {
                if value < 1 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.resource.{field}` must be >= 1, got {value}"
                        ),
                    });
                }
            }
        }
        Ok(self)
    }
}

fn validate_projectile_descriptor(
    projectile: &ProjectileDescriptor,
) -> Result<(), DescriptorError> {
    for (field, value, valid) in [
        (
            "speed",
            projectile.speed,
            projectile.speed.is_finite() && projectile.speed > 0.0,
        ),
        (
            "radius",
            projectile.radius,
            projectile.radius.is_finite() && projectile.radius >= 0.0,
        ),
        (
            "lifetimeMs",
            projectile.lifetime_ms,
            projectile.lifetime_ms.is_finite() && projectile.lifetime_ms > 0.0,
        ),
    ] {
        if !valid {
            let constraint = if field == "radius" { ">= 0.0" } else { "> 0.0" };
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.projectile.{field}` must be a finite value {constraint}, got {value}"
                ),
            });
        }
    }

    match &projectile.visual.body {
        ProjectileBodyVisual::Sprite {
            sprite,
            size,
            opacity,
            rotation,
            tint,
            emissive,
            frame_duration_ms,
        } => {
            validate_projectile_asset_path("body.sprite", sprite)?;
            for (field, value) in [
                ("body.size", *size),
                ("body.opacity", *opacity),
                ("body.rotation", *rotation),
            ] {
                if !value.is_finite() || (field == "body.size" && value <= 0.0) {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.weapon.projectile.visual.{field}` must be finite{}",
                            if field == "body.size" {
                                " and > 0.0"
                            } else {
                                ""
                            }
                        ),
                    });
                }
            }
            if !tint.iter().all(|value| value.is_finite()) {
                return Err(DescriptorError::InvalidShape {
                    reason:
                        "`components.weapon.projectile.visual.body.tint` must contain finite values"
                            .to_string(),
                });
            }
            if !emissive.is_finite() || *emissive < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.body.emissive` must be finite and >= 0.0, got {emissive}"
                    ),
                });
            }
            if let Some(frame_duration_ms) = frame_duration_ms
                && (!frame_duration_ms.is_finite() || *frame_duration_ms <= 0.0)
            {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.body.frameDurationMs` must be finite and > 0.0, got {frame_duration_ms}"
                    ),
                });
            }
        }
        ProjectileBodyVisual::Model { model } => {
            validate_projectile_asset_path("body.model", model)?
        }
    }

    if let Some(trail) = projectile.visual.trail.as_ref() {
        validate_projectile_asset_path("trail.sprite", &trail.sprite)?;
        // Keep the shared trail controls aligned with
        // `BillboardEmitterComponentLit::validate_into`. This descriptor lives
        // in foundation, below entities, so it mirrors that public contract
        // rather than depending on the component type. In particular,
        // buoyancy and spin rate are signed controls.
        for (field, value, valid) in [
            (
                "trail.rate",
                trail.rate,
                trail.rate.is_finite() && trail.rate >= 0.0,
            ),
            (
                "trail.lifetime",
                trail.lifetime,
                trail.lifetime.is_finite() && trail.lifetime > 0.0,
            ),
            (
                "trail.spread",
                trail.spread,
                trail.spread.is_finite() && trail.spread >= 0.0,
            ),
            (
                "trail.drag",
                trail.drag,
                trail.drag.is_finite() && trail.drag >= 0.0,
            ),
            ("trail.buoyancy", trail.buoyancy, trail.buoyancy.is_finite()),
            (
                "trail.spinRate",
                trail.spin_rate,
                trail.spin_rate.is_finite(),
            ),
        ] {
            if !valid {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.{field}` has an invalid value {value}"
                    ),
                });
            }
        }
        for (field, values) in [
            ("trail.velocity", trail.velocity.as_slice()),
            ("trail.color", trail.color.as_slice()),
            (
                "trail.sizeOverLifetime",
                trail.size_over_lifetime.as_slice(),
            ),
            (
                "trail.opacityOverLifetime",
                trail.opacity_over_lifetime.as_slice(),
            ),
        ] {
            if values.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.{field}` must be non-empty"
                    ),
                });
            }
            if !values.iter().all(|value| value.is_finite()) {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.{field}` must contain finite values"
                    ),
                });
            }
        }
        if let Some(animation) = trail.spin_animation.as_ref() {
            if !animation.duration.is_finite() || animation.duration <= 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.trail.spinAnimation.duration` must be a finite value > 0.0, got {}",
                        animation.duration
                    ),
                });
            }
            if animation.rate_curve.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: "`components.weapon.projectile.visual.trail.spinAnimation.rateCurve` must be non-empty".to_string(),
                });
            }
        }
    }

    if let Some(light) = projectile.visual.light.as_ref() {
        if !light.color.iter().all(|value| value.is_finite()) {
            return Err(DescriptorError::InvalidShape {
                reason:
                    "`components.weapon.projectile.visual.light.color` must contain finite values"
                        .to_string(),
            });
        }
        for (field, value, valid, constraint) in [
            (
                "intensity",
                light.intensity,
                light.intensity.is_finite() && light.intensity >= 0.0,
                ">= 0.0",
            ),
            (
                "falloffRange",
                light.falloff_range,
                light.falloff_range.is_finite() && light.falloff_range > 0.0,
                "> 0.0",
            ),
        ] {
            if !valid {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.light.{field}` must be a finite value {constraint}, got {value}"
                    ),
                });
            }
        }
    }

    if let Some(light) = projectile.visual.impact_light.as_ref() {
        if !light.color.iter().all(|value| value.is_finite()) {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.weapon.projectile.visual.impactLight.color` must contain finite values".to_string(),
            });
        }
        for (field, value, valid, constraint) in [
            (
                "intensity",
                light.intensity,
                light.intensity.is_finite() && light.intensity >= 0.0,
                ">= 0.0",
            ),
            (
                "radius",
                light.radius,
                light.radius.is_finite() && light.radius > 0.0,
                "> 0.0",
            ),
            (
                "fadeMs",
                light.fade_ms,
                light.fade_ms.is_finite() && light.fade_ms > 0.0,
                "> 0.0",
            ),
        ] {
            if !valid {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.weapon.projectile.visual.impactLight.{field}` must be a finite value {constraint}, got {value}"
                    ),
                });
            }
        }
        if let Some(peak_radius) = light.peak_radius
            && (!peak_radius.is_finite() || peak_radius < light.radius)
        {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.weapon.projectile.visual.impactLight.peakRadius` must be a finite value >= radius ({}), got {peak_radius}",
                    light.radius
                ),
            });
        }
    }
    Ok(())
}

fn validate_projectile_asset_path(field: &str, path: &str) -> Result<(), DescriptorError> {
    if is_portable_content_relative_asset_path(path) {
        return Ok(());
    }
    Err(DescriptorError::InvalidShape {
        reason: format!(
            "`components.weapon.projectile.visual.{field}` must be a non-empty, content-relative asset path using forward slashes with no parent traversal"
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TouchMode {
    Auto,
    Press,
}

const fn default_touch_mode() -> TouchMode {
    TouchMode::Auto
}

const fn default_touch_radius() -> f32 {
    40.0
}

/// Authored touch interaction preset for a world-placeable descriptor.
/// Both fields are descriptor-owned gameplay tuning; maps provide placement,
/// never interaction tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchableDescriptor {
    #[serde(default = "default_touch_mode")]
    pub mode: TouchMode,
    #[serde(default = "default_touch_radius")]
    pub radius: f32,
}

impl TouchableDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.radius.is_finite() || self.radius <= 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.touchable.radius` must be a finite value > 0.0, got {}",
                    self.radius
                ),
            });
        }
        Ok(self)
    }
}

fn validate_credit_source(value: &str) -> Result<(), DescriptorError> {
    validate_ascii_identifier("components.weapon.creditSource", value)
}

/// Authored health component preset attached to an entity type descriptor.
/// `max` is the entity's hit-point ceiling; the optional `hitbox` makes the
/// entity a hitscan and swept-projectile target (one world-aligned AABB, fixed
/// per archetype).
/// Wire keys are camelCase. Runtime data-archetype spawn materializes this into
/// a health component with `current == max`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDescriptor {
    pub max: f32,
    #[serde(default)]
    pub hitbox: Option<HitboxDescriptor>,
    /// Per-skeletal-zone damage multipliers, tag → factor (e.g. `"head" → 1.5`).
    /// A shot landing on a tagged zone scales the weapon's payload by this
    /// factor; an absent zone or an unlisted tag applies `1.0`. Each factor must
    /// be finite and `>= 0`. Defaults to empty (every zone applies `1.0`).
    #[serde(default, rename = "zoneMultipliers")]
    pub zone_multipliers: HashMap<String, f32>,
}

/// Authored hitbox sub-block: one world-aligned AABB. `half_extents` is the
/// box half-size on each axis; `offset` shifts the box center from the entity's
/// transform position (defaults to zero when absent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitboxDescriptor {
    pub half_extents: [f32; 3],
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
}

impl HealthDescriptor {
    /// Validate bounds serde cannot enforce (the `LightDescriptor::validate`
    /// precedent): `max` finite and `>= 1`; each `halfExtents` element finite and
    /// `> 0`; each `offset` element finite.
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.max.is_finite() || self.max < 1.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.health.max` must be a finite value >= 1.0, got {}",
                    self.max
                ),
            });
        }
        if let Some(hitbox) = self.hitbox.as_ref() {
            for (axis, value) in ["x", "y", "z"].iter().zip(hitbox.half_extents) {
                if !value.is_finite() || value <= 0.0 {
                    return Err(DescriptorError::InvalidShape {
                        reason: format!(
                            "`components.health.hitbox.halfExtents.{axis}` must be a finite value > 0.0, got {value}"
                        ),
                    });
                }
            }
            if let Some(offset) = hitbox.offset {
                for (axis, value) in ["x", "y", "z"].iter().zip(offset) {
                    if !value.is_finite() {
                        return Err(DescriptorError::InvalidShape {
                            reason: format!(
                                "`components.health.hitbox.offset.{axis}` must be a finite value, got {value}"
                            ),
                        });
                    }
                }
            }
        }
        for (tag, factor) in &self.zone_multipliers {
            if !factor.is_finite() || *factor < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.health.zoneMultipliers.{tag}` must be a finite value >= 0.0, got {factor}"
                    ),
                });
            }
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weapon_descriptor(credit_source: Option<&str>) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 180.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: credit_source.map(str::to_string),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        }
    }

    fn projectile_descriptor() -> ProjectileDescriptor {
        ProjectileDescriptor {
            speed: 24.0,
            radius: 0.1,
            lifetime_ms: 1_500.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/bolt.png".to_string(),
                    size: 0.35,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        }
    }

    #[test]
    fn projectile_resolution_requires_finite_travel_tuning_and_a_visual() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());
        assert!(descriptor.clone().validate().is_ok());

        for field in ["speed", "radius", "lifetimeMs"] {
            let mut invalid = descriptor.clone();
            let projectile = invalid.projectile.as_mut().unwrap();
            match field {
                "speed" => projectile.speed = f32::NAN,
                "radius" => projectile.radius = -0.01,
                "lifetimeMs" => projectile.lifetime_ms = 0.0,
                _ => unreachable!(),
            }
            let error = invalid.validate().unwrap_err();
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(reason.contains(field), "{reason}");
        }

        descriptor.projectile = None;
        let error = descriptor.validate().unwrap_err();
        let DescriptorError::InvalidShape { reason } = error else {
            panic!("expected InvalidShape");
        };
        assert!(reason.contains("components.weapon.projectile"), "{reason}");
    }

    #[test]
    fn muzzle_offset_requires_finite_components() {
        for muzzle_offset in [
            [f32::NAN, 0.0, 0.0],
            [0.0, f32::INFINITY, 0.0],
            [0.0, 0.0, f32::NEG_INFINITY],
        ] {
            let mut descriptor = weapon_descriptor(None);
            descriptor.muzzle_offset = Some(muzzle_offset);

            let error = descriptor
                .validate()
                .expect_err("non-finite muzzle rejects");
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(
                reason.contains("components.weapon.muzzleOffset"),
                "{reason}"
            );
        }
    }

    #[test]
    fn projectile_resolution_requires_exactly_one_pellet() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());
        descriptor.pellet_count = 2;

        let error = descriptor
            .validate()
            .expect_err("projectiles resolve one direct impact");
        let DescriptorError::InvalidShape { reason } = error else {
            panic!("expected InvalidShape");
        };
        assert!(reason.contains("pelletCount"), "{reason}");
        assert!(reason.contains("exactly 1"), "{reason}");
    }

    #[test]
    fn hitscan_resolution_rejects_projectile_settings() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.projectile = Some(projectile_descriptor());

        let error = descriptor
            .validate()
            .expect_err("hitscan must not retain projectile-only settings");
        let DescriptorError::InvalidShape { reason } = error else {
            panic!("expected InvalidShape");
        };
        assert!(reason.contains("components.weapon.projectile"), "{reason}");
        assert!(reason.contains("omitted"), "{reason}");
    }

    #[test]
    fn projectile_visual_and_trail_validation_name_the_invalid_field() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());

        let invalid_shapes: [(&str, fn(&mut ProjectileDescriptor)); 9] = [
            ("body.sprite", |projectile| {
                let ProjectileBodyVisual::Sprite { sprite, .. } = &mut projectile.visual.body
                else {
                    unreachable!();
                };
                *sprite = "../outside.png".to_string();
            }),
            ("body.model", |projectile| {
                projectile.visual.body = ProjectileBodyVisual::Model {
                    model: "C:/outside.gltf".to_string(),
                };
            }),
            ("trail.sprite", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    sprite: "../outside.png".to_string(),
                    ..valid_projectile_trail()
                });
            }),
            ("trail.rate", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    rate: -1.0,
                    ..valid_projectile_trail()
                });
            }),
            ("trail.lifetime", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    lifetime: 0.0,
                    ..valid_projectile_trail()
                });
            }),
            ("trail.sizeOverLifetime", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    size_over_lifetime: vec![],
                    ..valid_projectile_trail()
                });
            }),
            ("trail.velocity", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    velocity: [f32::NAN, 0.0, 0.0],
                    ..valid_projectile_trail()
                });
            }),
            ("spinAnimation.duration", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    spin_animation: Some(ProjectileTrailSpinAnimation {
                        duration: 0.0,
                        rate_curve: vec![0.0, 1.0],
                    }),
                    ..valid_projectile_trail()
                });
            }),
            ("spinAnimation.rateCurve", |projectile| {
                projectile.visual.trail = Some(ProjectileTrailVisual {
                    spin_animation: Some(ProjectileTrailSpinAnimation {
                        duration: 1.0,
                        rate_curve: Vec::new(),
                    }),
                    ..valid_projectile_trail()
                });
            }),
        ];

        for (field, mutate) in invalid_shapes {
            let mut invalid = descriptor.clone();
            mutate(invalid.projectile.as_mut().expect("projectile is present"));
            let error = invalid.validate().expect_err("projectile must be rejected");
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(reason.contains(field), "expected {field:?} in {reason:?}");
        }
    }

    #[test]
    fn projectile_impact_light_validation_names_every_invalid_field() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());

        let valid = ProjectileImpactLight {
            color: [0.5, 0.8, 1.0],
            intensity: 3.0,
            radius: 4.0,
            peak_radius: Some(8.0),
            fade_ms: 160.0,
        };
        descriptor
            .projectile
            .as_mut()
            .expect("projectile is present")
            .visual
            .impact_light = Some(valid.clone());
        assert!(descriptor.clone().validate().is_ok());

        for (field, impact_light) in [
            (
                "color",
                ProjectileImpactLight {
                    color: [f32::NAN, 0.8, 1.0],
                    ..valid.clone()
                },
            ),
            (
                "intensity",
                ProjectileImpactLight {
                    intensity: -0.01,
                    ..valid.clone()
                },
            ),
            (
                "radius",
                ProjectileImpactLight {
                    radius: 0.0,
                    ..valid.clone()
                },
            ),
            (
                "peakRadius",
                ProjectileImpactLight {
                    peak_radius: Some(3.0),
                    ..valid.clone()
                },
            ),
            (
                "fadeMs",
                ProjectileImpactLight {
                    fade_ms: 0.0,
                    ..valid.clone()
                },
            ),
        ] {
            let mut invalid = descriptor.clone();
            invalid
                .projectile
                .as_mut()
                .expect("projectile is present")
                .visual
                .impact_light = Some(impact_light);
            let error = invalid
                .validate()
                .expect_err("invalid impact light rejects");
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(reason.contains(&format!("impactLight.{field}")), "{reason}");
        }
    }

    #[test]
    fn projectile_sprite_emissive_requires_finite_nonnegative_strength() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());

        for emissive in [f32::NAN, -0.01] {
            let mut invalid = descriptor.clone();
            let ProjectileBodyVisual::Sprite {
                emissive: strength, ..
            } = &mut invalid
                .projectile
                .as_mut()
                .expect("projectile is present")
                .visual
                .body
            else {
                unreachable!();
            };
            *strength = emissive;

            let error = invalid
                .validate()
                .expect_err("invalid emissive must be rejected");
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(reason.contains("body.emissive"), "{reason}");
        }

        let ProjectileBodyVisual::Sprite { emissive, .. } = &mut descriptor
            .projectile
            .as_mut()
            .expect("projectile is present")
            .visual
            .body
        else {
            unreachable!();
        };
        *emissive = 3.0;
        assert!(
            descriptor.validate().is_ok(),
            "HDR emissive must be accepted"
        );
    }

    #[test]
    fn projectile_sprite_emissive_omission_defaults_to_zero() {
        let body: ProjectileBodyVisual = serde_json::from_value(serde_json::json!({
            "kind": "sprite",
            "sprite": "sprites/projectiles/bolt.png",
        }))
        .expect("sprite body should deserialize with defaults");
        let ProjectileBodyVisual::Sprite {
            emissive,
            frame_duration_ms,
            ..
        } = body
        else {
            unreachable!();
        };

        assert!(emissive.abs() < f32::EPSILON);
        assert!(frame_duration_ms.is_none());
    }

    #[test]
    fn projectile_sprite_frame_duration_deserializes_from_camel_case_authoring_key() {
        let body: ProjectileBodyVisual = serde_json::from_value(serde_json::json!({
            "kind": "sprite",
            "sprite": "sprites/projectiles/bolt.png",
            "frameDurationMs": 60.0,
        }))
        .expect("sprite body should deserialize authored cadence");
        let ProjectileBodyVisual::Sprite {
            frame_duration_ms, ..
        } = body
        else {
            unreachable!();
        };

        assert_eq!(frame_duration_ms, Some(60.0));
    }

    #[test]
    fn projectile_sprite_frame_duration_requires_finite_positive_value_when_present() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());

        for frame_duration_ms in [f32::NAN, 0.0, -0.01] {
            let mut invalid = descriptor.clone();
            let ProjectileBodyVisual::Sprite {
                frame_duration_ms: cadence,
                ..
            } = &mut invalid
                .projectile
                .as_mut()
                .expect("projectile is present")
                .visual
                .body
            else {
                unreachable!();
            };
            *cadence = Some(frame_duration_ms);

            let error = invalid
                .validate()
                .expect_err("invalid sprite cadence must be rejected");
            let DescriptorError::InvalidShape { reason } = error else {
                panic!("expected InvalidShape");
            };
            assert!(reason.contains("frameDurationMs"), "{reason}");
        }

        let ProjectileBodyVisual::Sprite {
            frame_duration_ms, ..
        } = &mut descriptor
            .projectile
            .as_mut()
            .expect("projectile is present")
            .visual
            .body
        else {
            unreachable!();
        };
        *frame_duration_ms = Some(60.0);
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn projectile_trail_keeps_signed_emitter_controls_valid() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());
        let projectile = descriptor
            .projectile
            .as_mut()
            .expect("projectile is present");
        projectile.visual.body = ProjectileBodyVisual::Model {
            model: "models/projectiles/rocket.gltf".to_string(),
        };
        projectile.visual.trail = Some(ProjectileTrailVisual {
            buoyancy: -1.0,
            spin_rate: -2.5,
            ..valid_projectile_trail()
        });

        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn projectile_trail_accepts_spin_animation_shape() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.projectile = Some(projectile_descriptor());
        descriptor.projectile.as_mut().unwrap().visual.trail = Some(ProjectileTrailVisual {
            spin_animation: Some(ProjectileTrailSpinAnimation {
                duration: 0.75,
                rate_curve: vec![0.0, 2.0, -1.0],
            }),
            ..valid_projectile_trail()
        });

        assert!(descriptor.validate().is_ok());
    }

    fn valid_projectile_trail() -> ProjectileTrailVisual {
        ProjectileTrailVisual {
            sprite: "sprites/projectiles/trail.png".to_string(),
            rate: 30.0,
            lifetime: 0.4,
            burst: None,
            spread: 0.0,
            velocity: [0.0, 0.0, 0.0],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![0.2, 0.0],
            opacity_over_lifetime: vec![0.8, 0.0],
            color: [1.0, 1.0, 1.0],
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    #[test]
    fn weapon_credit_source_accepts_allowed_ascii_identifier_and_omission() {
        let valid = "Alpha_09.source:primary-alt";

        let parsed = weapon_descriptor(Some(valid)).validate().unwrap();
        assert_eq!(parsed.credit_source.as_deref(), Some(valid));

        let omitted = weapon_descriptor(None).validate().unwrap();
        assert_eq!(omitted.credit_source, None);
    }

    #[test]
    fn weapon_credit_source_rejects_empty_overlength_and_disallowed_bytes() {
        for invalid in ["", "bad source", "rocket/primary", "plasma.\u{00e9}"] {
            let err = weapon_descriptor(Some(invalid)).validate().unwrap_err();
            assert!(
                err.to_string().contains("creditSource"),
                "unexpected error for {invalid:?}: {err}"
            );
        }

        let too_long = "a".repeat(65);
        let err = weapon_descriptor(Some(&too_long)).validate().unwrap_err();
        assert!(
            err.to_string().contains("64 bytes"),
            "unexpected overlength error: {err}"
        );
    }

    #[test]
    fn weapon_pellet_stats_validate_their_authored_bounds() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.pellet_count = MAX_PELLET_COUNT;
        descriptor.spread_degrees = 45.0;
        assert!(descriptor.clone().validate().is_ok());

        for pellet_count in [0, MAX_PELLET_COUNT + 1] {
            let mut invalid = descriptor.clone();
            invalid.pellet_count = pellet_count;
            let error = invalid.validate().unwrap_err();
            assert!(error.to_string().contains("pelletCount"), "{error}");
        }

        for spread_degrees in [-0.1, 45.1, f32::NAN, f32::INFINITY] {
            let mut invalid = descriptor.clone();
            invalid.spread_degrees = spread_degrees;
            let error = invalid.validate().unwrap_err();
            assert!(error.to_string().contains("spreadDegrees"), "{error}");
        }
    }

    #[test]
    fn weapon_placement_uses_authored_labels_defaults_and_finite_validation() {
        let placement: WeaponPlacementDescriptor = serde_json::from_value(serde_json::json!({
            "positionFromCenter": { "right": 0.32, "forward": 0.62 },
            "rotation": { "yaw": 15.0 },
        }))
        .expect("placement shape deserializes");
        assert_eq!(placement.offset.right, 0.32);
        assert_eq!(placement.offset.up, 0.0);
        assert_eq!(placement.offset.forward, 0.62);
        assert_eq!(placement.rotation.yaw, 15.0);
        assert_eq!(placement.rotation.pitch, 0.0);
        assert_eq!(placement.rotation.roll, 0.0);

        let mut descriptor = weapon_descriptor(None);
        descriptor.placement = Some(placement);
        assert!(descriptor.clone().validate().is_ok());

        for (field, placement) in [
            (
                "positionFromCenter.right",
                WeaponPlacementDescriptor {
                    offset: PlacementOffset {
                        right: f32::NAN,
                        ..PlacementOffset::default()
                    },
                    rotation: PlacementRotation::default(),
                },
            ),
            (
                "rotation.roll",
                WeaponPlacementDescriptor {
                    offset: PlacementOffset::default(),
                    rotation: PlacementRotation {
                        roll: f32::INFINITY,
                        ..PlacementRotation::default()
                    },
                },
            ),
        ] {
            let mut invalid = weapon_descriptor(None);
            invalid.placement = Some(placement);
            let error = invalid
                .validate()
                .expect_err("non-finite placement rejects");
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn weapon_ammo_resource_defaults_and_validates() {
        let mut descriptor = weapon_descriptor(None);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "shells.primary".to_string(),
            magazine: 8,
            cost_per_shot: 1,
            reserve: 0,
            reload_ms: 1000,
            reload_style: ReloadStyle::Magazine,
        }));
        assert!(descriptor.validate().is_ok());

        let parsed: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "fireRateMs": 180.0,
            "fireMode": "semi",
            "resolution": "hitscan",
            "resource": {
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "reserve": 32
            }
        }))
        .unwrap();
        let Some(WeaponResource::Ammo(ammo)) = parsed.resource else {
            panic!("expected ammo resource");
        };
        assert_eq!(ammo.cost_per_shot, 1);
        assert_eq!(ammo.reload_ms, 1000);
        assert_eq!(ammo.reload_style, ReloadStyle::Magazine);
    }

    #[test]
    fn weapon_ammo_resource_reload_style_serde_accepts_known_values_and_rejects_unknown() {
        for (value, expected) in [
            ("magazine", ReloadStyle::Magazine),
            ("perShell", ReloadStyle::PerShell),
        ] {
            let resource: WeaponResource = serde_json::from_value(serde_json::json!({
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "reserve": 32,
                "reloadStyle": value,
            }))
            .unwrap();
            let WeaponResource::Ammo(ammo) = resource;
            assert_eq!(ammo.reload_style, expected);
        }

        let error = serde_json::from_value::<WeaponResource>(serde_json::json!({
            "kind": "ammo",
            "type": "shells",
            "magazine": 8,
            "reserve": 32,
            "reloadStyle": "belt",
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn optional_weapon_model_paths_must_be_contained_content_relative_paths() {
        for invalid in [
            "",
            "/tmp/model.gltf",
            "../model.gltf",
            "models/../model.gltf",
            r"..\model.gltf",
            r"C:\models\model.gltf",
            "C:/models/model.gltf",
            "C:models/model.gltf",
            r"\\server\share\model.gltf",
        ] {
            for field in ["thirdPersonModel", "viewmodel"] {
                let mut descriptor = weapon_descriptor(None);
                if field == "thirdPersonModel" {
                    descriptor.third_person_model = Some(invalid.to_string());
                } else {
                    descriptor.viewmodel = Some(invalid.to_string());
                }
                let error = descriptor.validate().unwrap_err().to_string();
                assert!(
                    error.contains(field),
                    "unexpected error for {invalid:?}: {error}"
                );
                assert!(
                    error.contains("content-relative"),
                    "unexpected error for {invalid:?}: {error}"
                );
            }
        }

        let mut valid = weapon_descriptor(None);
        valid.third_person_model = Some("models/smg/model.gltf".to_string());
        valid.viewmodel = Some("./models/smg/view.gltf".to_string());
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn weapon_ammo_resource_rejects_semantically_invalid_values() {
        for (field, value) in [
            ("type", serde_json::json!("bad ammo")),
            ("magazine", serde_json::json!(0)),
            ("costPerShot", serde_json::json!(0)),
            ("reloadMs", serde_json::json!(0)),
        ] {
            let mut ammo = serde_json::json!({
                "kind": "ammo",
                "type": "shells",
                "magazine": 8,
                "costPerShot": 1,
                "reserve": 32,
                "reloadMs": 1000
            });
            ammo[field] = value;
            let mut descriptor = weapon_descriptor(None);
            descriptor.resource = Some(serde_json::from_value(ammo).unwrap());
            let err = descriptor.validate().unwrap_err();
            assert!(err.to_string().contains(field), "unexpected error: {err}");
        }

        for invalid_type in ["", "rocket/primary", "plasma.\u{00e9}"] {
            let mut descriptor = weapon_descriptor(None);
            descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: invalid_type.to_string(),
                magazine: 8,
                cost_per_shot: 1,
                reserve: 32,
                reload_ms: 1000,
                reload_style: ReloadStyle::Magazine,
            }));
            assert!(descriptor.validate().is_err(), "accepted {invalid_type:?}");
        }

        let mut descriptor = weapon_descriptor(None);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "a".repeat(65),
            magazine: 8,
            cost_per_shot: 1,
            reserve: 32,
            reload_ms: 1000,
            reload_style: ReloadStyle::Magazine,
        }));
        let err = descriptor.validate().unwrap_err();
        assert!(err.to_string().contains("64 bytes"));
    }

    #[test]
    fn weapon_ammo_resource_rejects_invalid_serde_shapes() {
        for resource in [
            serde_json::json!({"kind": "cell", "type": "cells", "magazine": 8, "reserve": 32}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": -1, "reserve": 32}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": 8, "reserve": -1}),
            serde_json::json!({"kind": "ammo", "type": "cells", "magazine": "8", "reserve": 32}),
        ] {
            assert!(serde_json::from_value::<WeaponResource>(resource).is_err());
        }
    }

    #[test]
    fn touchable_descriptor_defaults_and_rejects_nonpositive_or_nonfinite_radius() {
        let defaults: TouchableDescriptor = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(defaults.mode, TouchMode::Auto);
        assert!((defaults.radius - 40.0).abs() <= f32::EPSILON);

        let press_only: TouchableDescriptor =
            serde_json::from_value(serde_json::json!({ "mode": "press" })).unwrap();
        assert_eq!(press_only.mode, TouchMode::Press);
        assert!((press_only.radius - 40.0).abs() <= f32::EPSILON);

        for radius in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            let error = TouchableDescriptor {
                mode: TouchMode::Auto,
                radius,
            }
            .validate()
            .expect_err("non-positive and non-finite touch radii must reject");
            assert!(error.to_string().contains("components.touchable.radius"));
        }
    }
}
