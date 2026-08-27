// Compiler selection for baked-tier static lights that may cast runtime
// shadows onto moving entities.
// See: context/lib/rendering_pipeline.md §4 (Promoted static lights)

use std::collections::HashMap;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::lightmap_bake;
use crate::map_data::{EntityShadowParams, LightType, MapLight, ShadowType};

const DECORATIVE_SPOT_GRID: usize = 16;
const DECORATIVE_SPOT_RAY_COUNT: usize = DECORATIVE_SPOT_GRID * DECORATIVE_SPOT_GRID;
const DECORATIVE_SPOT_BLOCKED_RATIO: f32 = 0.75;
const DECORATIVE_SPOT_MAX_DISTANCE: f32 = 1.5;
const DECORATIVE_SPOT_RANGE_FRACTION: f32 = 0.25;

pub struct EntityShadowSelectionInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub static_lights: &'a StaticBakedLights<'a>,
    pub alpha_lights: &'a AlphaLightsNs<'a>,
    pub params: EntityShadowParams,
}

pub fn select_entity_shadow_lights(
    inputs: &EntityShadowSelectionInputs<'_>,
) -> EntityShadowLightsSection {
    let alpha_indices_by_source = inputs
        .alpha_lights
        .entries()
        .iter()
        .enumerate()
        .map(|(alpha_index, entry)| (entry.source_index, alpha_index as u32))
        .collect::<HashMap<_, _>>();

    let max_static_intensity = inputs
        .static_lights
        .entries()
        .iter()
        .filter(|entry| is_promotable_base_light(entry.light))
        .map(|entry| entry.light.intensity.max(0.0))
        .fold(0.0, f32::max);
    let min_intensity = inputs.params.min_intensity_ratio * max_static_intensity;

    let mut light_indices = inputs
        .static_lights
        .entries()
        .iter()
        .filter_map(|entry| {
            let alpha_index = *alpha_indices_by_source.get(&entry.source_index)?;
            is_eligible(
                entry.light,
                min_intensity,
                inputs.params.min_range,
                inputs.bvh,
                inputs.primitives,
                inputs.geometry,
            )
            .then_some(alpha_index)
        })
        .collect::<Vec<_>>();

    light_indices.sort_unstable();
    EntityShadowLightsSection { light_indices }
}

fn is_eligible(
    light: &MapLight,
    min_intensity: f32,
    min_range: f32,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> bool {
    if !is_promotable_base_light(light) {
        return false;
    }
    if light.intensity < min_intensity {
        return false;
    }
    if light.falloff_range < min_range {
        return false;
    }
    if light.light_type == LightType::Spot
        && decorative_spot_fixture(light, bvh, primitives, geometry)
    {
        return false;
    }
    true
}

fn is_promotable_base_light(light: &MapLight) -> bool {
    !light.is_dynamic
        && !light.is_animated
        && light.animation.is_none()
        && !light.bake_only
        && matches!(light.light_type, LightType::Point | LightType::Spot)
        && light.shadow_type == ShadowType::StaticLightMap
}

// Detects a spot fixture aimed into its own mounting surface (e.g. a wall
// sconce facing the wall) so it can be excluded from promotion — such a
// light casts no useful entity shadow and would waste a pool slot. Casts a
// solid-angle-uniform grid of rays over the light's cone cap (cos-θ-uniform
// sampling avoids polar oversampling) and flags the fixture as decorative
// once most rays are blocked within a short probe distance.
fn decorative_spot_fixture(
    light: &MapLight,
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
) -> bool {
    let Some(direction) = light.cone_direction.map(Vec3::from) else {
        return false;
    };
    let Some(outer_angle) = light.cone_angle_outer else {
        return false;
    };
    let axis = direction.normalize_or_zero();
    if axis.length_squared() == 0.0 {
        return false;
    }

    let distance =
        DECORATIVE_SPOT_MAX_DISTANCE.min(DECORATIVE_SPOT_RANGE_FRACTION * light.falloff_range);
    let origin = dvec3_to_vec3(light.origin);
    let cos_outer = outer_angle.cos();
    let (tangent, bitangent) = orthonormal_basis(axis);

    let mut blocked = 0usize;
    for y in 0..DECORATIVE_SPOT_GRID {
        for x in 0..DECORATIVE_SPOT_GRID {
            let u = (x as f32 + 0.5) / DECORATIVE_SPOT_GRID as f32;
            let v = (y as f32 + 0.5) / DECORATIVE_SPOT_GRID as f32;
            let cos_theta = 1.0 - u * (1.0 - cos_outer);
            let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
            let phi = std::f32::consts::TAU * v;
            let ray_dir = (tangent * (phi.cos() * sin_theta)
                + bitangent * (phi.sin() * sin_theta)
                + axis * cos_theta)
                .normalize_or_zero();
            if !lightmap_bake::segment_clear(
                bvh,
                primitives,
                geometry,
                origin,
                origin + ray_dir * distance,
            ) {
                blocked += 1;
            }
        }
    }

    blocked as f32 / DECORATIVE_SPOT_RAY_COUNT as f32 >= DECORATIVE_SPOT_BLOCKED_RATIO
}

fn dvec3_to_vec3(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

fn orthonormal_basis(axis: Vec3) -> (Vec3, Vec3) {
    let helper = if axis.z.abs() < 0.999 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let tangent = helper.cross(axis).normalize_or_zero();
    let bitangent = axis.cross(tangent);
    (tangent, bitangent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build;
    use crate::geometry::FaceIndexRange;
    use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
    use crate::map_data::{FalloffModel, LightAnimation};
    use crate::script_light_membership;
    use glam::DVec3;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::light_membership::{
        LightMembershipManifest, LightMembershipRecord,
    };
    use postretro_level_format::texture_names::TextureNamesSection;

    fn point_light(intensity: f32, range: f32) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            carrier: String::new(),
            light_type: LightType::Point,
            intensity,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: range,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn spot_light(origin: DVec3, direction: [f32; 3]) -> MapLight {
        MapLight {
            origin,
            carrier: String::new(),
            light_type: LightType::Spot,
            cone_angle_inner: Some(0.2),
            cone_angle_outer: Some(0.35),
            cone_direction: Some(direction),
            ..point_light(1.0, 10.0)
        }
    }

    fn empty_geometry() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![],
                indices: vec![],
                faces: vec![],
            },
            texture_names: TextureNamesSection { names: vec![] },
            face_index_ranges: vec![],
        }
    }

    fn vertex(pos: [f32; 3]) -> Vertex {
        Vertex::new(
            pos,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn wall_geometry() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    vertex([-2.0, -2.0, -0.5]),
                    vertex([2.0, -2.0, -0.5]),
                    vertex([0.0, 2.0, -0.5]),
                ],
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: vec![] },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn select(lights: &[MapLight], geometry: &GeometryResult) -> EntityShadowLightsSection {
        let (bvh, primitives, _) = bvh_build::build_bvh(geometry).unwrap();
        let static_lights = StaticBakedLights::from_lights(lights);
        let alpha_lights = AlphaLightsNs::from_lights(lights);
        select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry,
            static_lights: &static_lights,
            alpha_lights: &alpha_lights,
            params: EntityShadowParams::default(),
        })
    }

    #[test]
    fn selector_marks_eligible_static_point_light() {
        let lights = vec![point_light(1.0, 8.0)];

        let selected = select(&lights, &empty_geometry());

        assert_eq!(selected.light_indices, vec![0]);
    }

    #[test]
    fn selector_excludes_low_intensity_lights() {
        let lights = vec![point_light(1.0, 8.0), point_light(0.49, 8.0)];

        let selected = select(&lights, &empty_geometry());

        assert_eq!(selected.light_indices, vec![0]);
    }

    #[test]
    fn selector_intensity_baseline_ignores_non_promotable_static_lights() {
        let eligible = point_light(1.0, 8.0);
        let mut bright_sdf = point_light(10.0, 8.0);
        bright_sdf.shadow_type = ShadowType::Sdf;

        let selected = select(&[eligible, bright_sdf], &empty_geometry());

        assert_eq!(selected.light_indices, vec![0]);
    }

    #[test]
    fn selector_excludes_low_range_lights() {
        let lights = vec![point_light(1.0, 8.0), point_light(1.0, 3.99)];

        let selected = select(&lights, &empty_geometry());

        assert_eq!(selected.light_indices, vec![0]);
    }

    #[test]
    fn selector_excludes_non_runtime_or_non_static_lightmap_lights() {
        let mut bake_only = point_light(1.0, 8.0);
        bake_only.bake_only = true;
        let mut sdf = point_light(1.0, 8.0);
        sdf.shadow_type = ShadowType::Sdf;
        let mut directional = point_light(1.0, 8.0);
        directional.light_type = LightType::Directional;
        let mut animated = point_light(1.0, 8.0);
        animated.animation = Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![1.0, 0.5]),
            color: None,
            direction: None,
            start_active: true,
        });
        let lights = vec![point_light(1.0, 8.0), bake_only, sdf, directional, animated];

        let selected = select(&lights, &empty_geometry());

        assert_eq!(selected.light_indices, vec![0]);
    }

    #[test]
    fn selector_excludes_script_derived_animated_membership() {
        let mut lights = vec![point_light(1.0, 8.0)];
        let manifest = LightMembershipManifest::new(
            vec![LightMembershipRecord {
                index: 0,
                is_dynamic: false,
                start_active: None,
                start_active_conflict: false,
            }],
            Vec::new(),
        );
        script_light_membership::apply_manifest(&mut lights, &[true], &manifest)
            .expect("script target becomes an animated baked light");

        let selected = select(&lights, &empty_geometry());

        assert!(
            selected.light_indices.is_empty(),
            "derived animated membership must not promote the light to entity-shadow selection"
        );
    }

    #[test]
    fn selector_excludes_decorative_spot_aimed_into_near_surface() {
        let lights = vec![spot_light(DVec3::ZERO, [0.0, 0.0, -1.0])];

        let selected = select(&lights, &wall_geometry());

        assert!(selected.light_indices.is_empty());
    }
}
