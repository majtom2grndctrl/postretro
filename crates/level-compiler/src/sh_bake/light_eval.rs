// Per-light radiance evaluation: falloff, spot cone, Lambert term, bounce guard.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use crate::map_data::{FalloffModel, LightType, MapLight};

/// Normal-free incident radiance reaching `point` from `light`, plus the unit
/// incident direction (point→light). This is the surface-cosine-removed core of
/// `light_contribution_lambert`: falloff + spot-cone + directional intensity, but
/// no `n·l` term. A probe is a point in air with no surface normal at bake time,
/// so the direct-at-probe SH bake (D3) projects THIS radiance along the incident
/// direction and lets the runtime fragment's normal apply the cosine.
///
/// Returns `None` when the light reaches the point with zero radiance — too close
/// to the origin (degenerate direction), outside falloff range, or fully outside
/// the spot cone. The unit direction for Point/Spot is geometric (point→light);
/// for Directional it is `-cone_direction` (the photon travel vector negated).
///
/// `light_contribution_lambert` is exactly `radiance × max(n·l, 0)`, so the two
/// stay in lockstep by construction — `light_contribution_lambert` calls this and
/// applies the cosine. The Task 0 harness pins that factoring for all three light
/// types.
pub(crate) fn incident_radiance_at_point(light: &MapLight, point: Vec3) -> Option<(Vec3, Vec3)> {
    match light.light_type {
        LightType::Point => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return None;
            }
            let l = to_light / dist;
            let radiance = Vec3::from(light.color) * (light.intensity * falloff(light, dist));
            Some((radiance, l))
        }
        LightType::Spot => {
            let to_light = Vec3::new(
                light.origin.x as f32 - point.x,
                light.origin.y as f32 - point.y,
                light.origin.z as f32 - point.z,
            );
            let dist = to_light.length();
            if dist < 1.0e-4 {
                return None;
            }
            let l = to_light / dist;
            let cone = spot_cone_attenuation(light, -l);
            let radiance =
                Vec3::from(light.color) * (light.intensity * falloff(light, dist) * cone);
            Some((radiance, l))
        }
        LightType::Directional => {
            let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0]));
            // cone_direction is the photon travel vector; negate to get the surface-to-light vector.
            let l = (-dir).normalize_or_zero();
            if l.length_squared() < 1.0e-8 {
                return None;
            }
            let radiance = Vec3::from(light.color) * light.intensity;
            Some((radiance, l))
        }
        // No single direction carries a polygon's light. A probe treated as
        // a receiver facing the light's net direction Φ̂ (the normalised
        // irradiance vector of the polygons it faces) takes the windowed
        // polygon term, which is I/d² in the far limit and never more than a
        // card facing the light receives: billboard scatter's value. Direct
        // SH projects over the polygons instead (`area_probe`), and the
        // Lambert term clips to the receiver's own horizon.
        LightType::Area => area_facing_radiance(light, point),
    }
}

/// An area light's windowed polygon term at `point` as a receiver facing
/// the light's probe direction receives it, times colour × radiance, and
/// that direction: Φ̂, or a faced polygon's reversed normal where Φ cancels
/// ([`AreaEmitter::probe_direction`](crate::area_light::AreaEmitter::probe_direction)),
/// the lift direct SH takes. `None` where no polygon faces the point or the
/// term is zero.
fn area_facing_radiance(light: &MapLight, point: Vec3) -> Option<(Vec3, Vec3)> {
    let emitter = light.area.as_ref()?;
    let p = point.as_dvec3();
    let facing = emitter.probe_direction(p)?;
    let (irradiance, _) = emitter.windowed_irradiance(p, facing, f64::from(light.falloff_range));
    if irradiance <= 0.0 {
        return None;
    }
    let radiance = Vec3::from(light.color) * (f64::from(light.intensity) * irradiance) as f32;
    Some((radiance, facing.as_vec3()))
}

/// Lambert contribution without visibility — caller handles shadow testing.
/// Exactly `incident_radiance_at_point × max(n·l, 0)`, the surface-cosine the
/// probe bake omits, for every light but an area light, whose term is the
/// windowed polygon irradiance clipped to the receiver's own horizon.
pub(super) fn light_contribution_lambert(
    light: &MapLight,
    surface_point: Vec3,
    surface_normal: Vec3,
) -> Vec3 {
    if light.light_type == LightType::Area {
        return area_lambert(light, surface_point, surface_normal);
    }
    let Some((radiance, l)) = incident_radiance_at_point(light, surface_point) else {
        return Vec3::ZERO;
    };
    let n_dot_l = surface_normal.dot(l).max(0.0);
    radiance * n_dot_l
}

/// An area light's windowed polygon term at a receiver `(point, normal)`,
/// clipped to its horizon, times colour × radiance: the lightmap texel's
/// unshadowed term (`lightmap_bake::light_contribution_and_direction`).
fn area_lambert(light: &MapLight, point: Vec3, normal: Vec3) -> Vec3 {
    let Some(emitter) = light.area.as_ref() else {
        return Vec3::ZERO;
    };
    let (irradiance, _) = emitter.windowed_irradiance(
        point.as_dvec3(),
        normal.as_dvec3(),
        f64::from(light.falloff_range),
    );
    if irradiance <= 0.0 {
        return Vec3::ZERO;
    }
    Vec3::from(light.color) * (f64::from(light.intensity) * irradiance) as f32
}

/// Must match `falloff` in `forward.wgsl` exactly — divergence produces "ghost glow"
/// or missing bounce light. InverseDistance/InverseSquared have no upper clamp past 1.0,
/// matching the shader convention.
pub(super) fn falloff(light: &MapLight, distance: f32) -> f32 {
    let range = light.falloff_range.max(1.0e-4);
    match light.falloff_model {
        FalloffModel::Linear => (1.0 - distance / range).clamp(0.0, 1.0),
        FalloffModel::InverseDistance => {
            if distance > range {
                return 0.0;
            }
            1.0 / distance.max(1.0e-4)
        }
        FalloffModel::InverseSquared => {
            if distance > range {
                return 0.0;
            }
            let d2 = (distance * distance).max(1.0e-4);
            1.0 / d2
        }
    }
}

/// Whether a light can contribute bounced radiance at `point` before visibility
/// is traced. Point and spot lights have zero falloff strictly beyond their
/// clamped range; directional lights are position-independent.
pub(crate) fn light_reaches_point(light: &MapLight, point: Vec3) -> bool {
    match light.light_type {
        LightType::Directional => true,
        LightType::Point | LightType::Spot => {
            let light_origin = Vec3::new(
                light.origin.x as f32,
                light.origin.y as f32,
                light.origin.z as f32,
            );
            point.distance(light_origin) <= light.falloff_range.max(1.0e-4)
        }
        LightType::Area => {
            let (centre, radius) = light.reach_sphere();
            point.as_dvec3().distance(centre) <= radius.max(1.0e-4)
        }
    }
}

/// The SH bounce's pre-ray guard: a light's Lambert term at a bounce hit, or
/// `None` when its shadow ray is provably wasted because the term is exactly
/// zero — beyond range, past a spot's outer cone, or behind the surface. Exact
/// zero, not an epsilon: a near-zero term still accumulates. The range test
/// runs first because a non-finite light origin yields a NaN term rather than
/// zero; skipping it on range keeps this bit-identical to the range-only guard
/// it extends for every input.
pub(crate) fn bounce_contribution(
    light: &MapLight,
    hit_point: Vec3,
    hit_normal: Vec3,
) -> Option<Vec3> {
    if !light_reaches_point(light, hit_point) {
        return None;
    }
    let contribution = light_contribution_lambert(light, hit_point, hit_normal);
    (contribution != Vec3::ZERO).then_some(contribution)
}

/// Must match `cone_attenuation` in `forward.wgsl` — Hermite cubic smoothstep
/// so direct and indirect agree along the cone fringe.
pub(crate) fn spot_cone_parameters(light: &MapLight) -> (Vec3, f32, f32) {
    let dir = Vec3::from(light.cone_direction.unwrap_or([0.0, -1.0, 0.0])).normalize_or_zero();
    let inner = light.cone_angle_inner.unwrap_or(0.0);
    let outer = light.cone_angle_outer.unwrap_or(inner + 0.01);
    (dir, inner.cos(), outer.cos())
}

pub(super) fn spot_cone_attenuation(light: &MapLight, light_to_surface: Vec3) -> f32 {
    let (dir, cos_inner, cos_outer) = spot_cone_parameters(light);
    let cos_theta = dir.dot(light_to_surface.normalize_or_zero());
    smoothstep(cos_outer, cos_inner, cos_theta)
}

/// `t² × (3 - 2t)` — matches WGSL's `smoothstep(edge0, edge1, x)`.
pub(super) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1.0e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
