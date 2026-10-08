// Projectile travel, splash, and presentation validation.
use super::*;

pub(super) fn validate_splash_descriptor(splash: &SplashDescriptor) -> Result<(), DescriptorError> {
    if let Some(knockback) = &splash.knockback {
        knockback.validate("components.weapon.splash.knockback")?;
    }
    if !splash.radius.is_finite() || splash.radius <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.weapon.splash.radius` must be a finite value > 0.0, got {}",
                splash.radius
            ),
        });
    }
    if !splash.min_fraction.is_finite() || !(0.0..=1.0).contains(&splash.min_fraction) {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.weapon.splash.minFraction` must be a finite value in 0.0..=1.0, got {}",
                splash.min_fraction
            ),
        });
    }
    Ok(())
}

pub(super) fn validate_projectile_descriptor(
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
