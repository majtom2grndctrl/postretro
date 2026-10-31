// PNG-backed, pre-encoding colour and base/delta estimator contracts.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};

use super::bounce::*;
use super::light_eval::*;
use super::sh_math::*;
use super::trace::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::coloured_bounce_test_fixtures::{Appearance, room_map, write_textures};
use crate::delta_sh_bake::{indirect_light_axis, unit_radiance_light};
use crate::fixture_pipeline::FixturePipeline;
use crate::lightmap_bake::soft_visibility_one_sample;
use crate::surface_materials::stats::BounceMaterials;
use crate::texture_mips::{StageTextureBytes, bake_materials_into};
use postretro_level_format::geometry::NO_TEXTURE;
use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

// Map (-112, 0, 48), just inside the red west wall. Quake X becomes engine -Z.
const PROBE: Vec3 = Vec3::new(0.0, 48.0 * 0.0254, 112.0 * 0.0254);
const PROBE_INDEX: u64 = 17;

struct Room {
    dir: tempfile::TempDir,
    pipeline: FixturePipeline,
}

impl Room {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_textures(dir.path());
        let path = dir.path().join("room.map");
        std::fs::write(&path, room_map(Appearance::RedWall, true, true)).unwrap();
        let pipeline = crate::fixture_pipeline::load_map(&path);
        let room = Self { dir, pipeline };
        assert!(
            probe_is_valid_pub(
                &room.pipeline.tree,
                &room.pipeline.exterior_leaves,
                PROBE.as_dvec3()
            ),
            "probe must be inside the playable room"
        );
        room.assert_bvh_texture_indices();
        room
    }

    fn materials(&self, saturation: f32) -> BounceMaterials {
        let (_, stats) = bake_materials_into(
            &self.pipeline.geometry.texture_names.names,
            &self.dir.path().join("textures"),
            &self.dir.path().join("materials"),
            &mut StageTextureBytes::new(),
            true,
        )
        .unwrap();
        BounceMaterials::new(stats, saturation).unwrap()
    }

    fn static_light(&self) -> &MapLight {
        self.pipeline
            .lights
            .iter()
            .find(|light| light.animation.is_none())
            .unwrap()
    }

    fn ray_ctx<'a>(&'a self, materials: Option<&'a BounceMaterials>) -> RaytracingCtx<'a> {
        RaytracingCtx {
            bvh: &self.pipeline.bvh,
            primitives: &self.pipeline.primitives,
            geometry: &self.pipeline.geometry,
            bounce_materials: materials,
        }
    }

    fn static_coefficients(
        &self,
        light: &MapLight,
        materials: Option<&BounceMaterials>,
    ) -> [f32; 27] {
        let lights = std::slice::from_ref(light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bvh: &self.pipeline.bvh,
            primitives: &self.pipeline.primitives,
            geometry: &self.pipeline.geometry,
            tree: &self.pipeline.tree,
            exterior_leaves: &self.pipeline.exterior_leaves,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
            bounce_materials: materials,
        };
        bake_probe(&inputs, PROBE, &[light], None, 100.0, true, PROBE_INDEX).coefficients
    }

    fn assert_bvh_texture_indices(&self) {
        let geometry = &self.pipeline.geometry;
        assert!(!self.pipeline.primitives.is_empty());
        for primitive in &self.pipeline.primitives {
            let face_index = geometry
                .face_index_ranges
                .iter()
                .position(|range| {
                    range.index_offset == primitive.index_offset
                        && range.index_count == primitive.index_count
                })
                .expect("every BVH primitive belongs to a static face");
            assert_eq!(
                primitive.material_bucket_id, geometry.geometry.faces[face_index].texture_index,
                "the SH hit's bucket must name the face's texture"
            );
        }
    }
}

fn rgb(coefficients: &[f32; 27], basis: usize) -> Vec3 {
    Vec3::from_slice(&coefficients[basis * 3..basis * 3 + 3])
}

fn band(basis: usize) -> std::ops::Range<usize> {
    match basis {
        0 => 0..1,
        1..=3 => 1..4,
        _ => 4..9,
    }
}

fn band_tolerance(coefficients: &[[f32; 27]], basis: usize) -> f32 {
    let maximum = coefficients
        .iter()
        .flat_map(|values| {
            band(basis).flat_map(move |index| values[index * 3..index * 3 + 3].iter().copied())
        })
        .map(f32::abs)
        .fold(0.0, f32::max);
    maximum * 0.001
}

#[test]
fn png_backed_red_wall_spills_red_while_empty_materials_keep_neutral_bounce() {
    let room = Room::new();
    let materials = room.materials(1.0);
    let coloured = room.static_coefficients(room.static_light(), Some(&materials));
    let empty = BounceMaterials::new(Vec::new(), 1.0).unwrap();
    let grey = room.static_coefficients(room.static_light(), Some(&empty));
    let legacy = room.static_coefficients(room.static_light(), None);
    let coloured_l0 = rgb(&coloured, 0);
    assert!(
        coloured_l0.x > coloured_l0.y && coloured_l0.x > coloured_l0.z,
        "red wall must tint the probe's indirect term: {coloured_l0}"
    );
    for basis in 0..9 {
        let value = rgb(&grey, basis);
        let epsilon = band_tolerance(&[grey], basis);
        assert!(
            (value.x - value.y).abs() <= epsilon && (value.x - value.z).abs() <= epsilon,
            "neutral fallback basis {basis}: {value}"
        );
    }
    assert_eq!(
        grey.map(f32::to_bits),
        legacy.map(f32::to_bits),
        "empty table preserves the legacy estimator exactly"
    );
}

#[test]
fn png_backed_probe_saturation_preserves_every_coefficient_luminance_and_interpolates() {
    let room = Room::new();
    let coefficients = [0.0, 0.5, 1.0].map(|saturation| {
        let materials = room.materials(saturation);
        room.static_coefficients(room.static_light(), Some(&materials))
    });
    let weights = Vec3::new(0.2126, 0.7152, 0.0722);
    for basis in 0..9 {
        let [neutral, half, full] = coefficients.map(|values| rgb(&values, basis));
        let epsilon = band_tolerance(&coefficients, basis);
        assert!(
            (neutral.x - neutral.y).abs() <= epsilon && (neutral.x - neutral.z).abs() <= epsilon,
            "basis {basis} not neutral: {neutral}"
        );
        assert!(
            (neutral.dot(weights) - full.dot(weights)).abs() <= epsilon,
            "basis {basis} luminance moved: {neutral} vs {full}"
        );
        assert!(
            (half - (neutral + full) * 0.5).abs().max_element() <= epsilon,
            "basis {basis} does not interpolate: {neutral}, {half}, {full}"
        );
    }
    let half_spread = (rgb(&coefficients[1], 0).x - rgb(&coefficients[1], 0).z).abs();
    let full_spread = (rgb(&coefficients[2], 0).x - rgb(&coefficients[2], 0).z).abs();
    assert!(
        half_spread > 0.0 && half_spread < full_spread,
        "half saturation must strictly reduce the red/blue spread"
    );
}

#[test]
fn animated_unit_bounce_composed_at_runtime_peak_matches_equivalent_static_bounce() {
    let room = Room::new();
    let materials = room.materials(1.0);
    let animated = room
        .pipeline
        .lights
        .iter()
        .find(|light| light.animation.is_some())
        .unwrap();
    let descriptor = animation_descriptor_for_light(animated);
    let scale_at = |time| {
        animated_light_scale(
            Some(AnimatedLightScaleDescriptor {
                period: descriptor.period,
                phase: descriptor.phase,
                base_color: descriptor.base_color,
                brightness: &descriptor.brightness,
                color: &descriptor.color,
                is_active: true,
            }),
            time,
        )
    };
    // Select the runtime curve's actual sampled peak, including interpolation;
    // comparing only the authored samples would miss Catmull-Rom overshoot.
    let (peak_time, peak) = (0..4096)
        .map(|i| {
            let time = descriptor.period.abs() * i as f32 / 4096.0;
            (time, scale_at(time))
        })
        .max_by(|a, b| a.1[0].total_cmp(&b.1[0]))
        .unwrap();
    assert!(
        peak[0] > 0.0 && peak[0] > scale_at(0.0)[0],
        "fixture animation must have a meaningful changing peak"
    );
    assert!((peak[0] - peak[1]).abs() < f32::EPSILON && (peak[0] - peak[2]).abs() < f32::EPSILON);
    let unit = unit_radiance_light(animated);
    assert!((unit.intensity - 1.0).abs() <= f32::EPSILON);
    assert!(
        (animated.intensity - unit.intensity).abs() > 1.0,
        "delta must genuinely bake unit radiance"
    );
    let mut static_peak = room.static_light().clone();
    assert!(
        static_peak.origin.distance(animated.origin) <= f64::EPSILON,
        "both lights share the same emitter position"
    );
    static_peak.intensity = peak[0];
    static_peak.color = [1.0; 3];
    for table in [Some(&materials), None] {
        // Different production entry points: static integrates authored peak
        // radiance; delta integrates unit transport, then runtime scale composes it.
        let static_coefficients = room.static_coefficients(&static_peak, table);
        let delta = bake_probe_indirect_rgb(
            &room.ray_ctx(table),
            PROBE,
            &unit,
            indirect_light_axis(&unit),
            PROBE_INDEX,
            BounceVisibility::OneSample,
        );
        let scale = scale_at(peak_time);
        let composed: [f32; 27] = std::array::from_fn(|index| delta[index] * scale[index % 3]);
        for index in 0..27 {
            assert!(
                (composed[index] - static_coefficients[index]).abs() <= 0.02,
                "coefficient {index}: composed {} vs static {}",
                composed[index],
                static_coefficients[index]
            );
        }
        let static_irradiance = evaluate_sh_rgb(&static_coefficients, Vec3::Z);
        let composed_irradiance = evaluate_sh_rgb(&composed, Vec3::Z);
        assert!(
            static_irradiance.max_element() >= 0.2 && composed_irradiance.max_element() >= 0.2,
            "parity fixture must have substantial bounce: {static_irradiance}, {composed_irradiance}"
        );
        assert!(
            (static_irradiance - composed_irradiance)
                .abs()
                .max_element()
                <= 0.02,
            "runtime-composed irradiance differs: {static_irradiance}, {composed_irradiance}"
        );
    }
}

#[test]
fn mixed_resolved_and_unresolved_hits_keep_exact_legacy_grey_at_every_saturation() {
    let mut room = Room::new();
    let names = &mut room.pipeline.geometry.texture_names.names;
    let red = names.iter().position(|name| name == "bounce/red").unwrap() as u32;
    let missing = names.len() as u32;
    names.extend(
        [
            "bounce/missing",
            "bounce/normal_only",
            "bounce/specular_only",
        ]
        .map(str::to_owned),
    );
    let cases = [
        NO_TEXTURE,
        names.len() as u32,
        missing,
        missing + 1,
        missing + 2,
    ];
    let mut next = 0;
    for face in &mut room.pipeline.geometry.geometry.faces {
        if face.texture_index != red {
            face.texture_index = cases[next % cases.len()];
            next += 1;
        }
    }
    assert!(
        next >= cases.len(),
        "room must expose every unresolved face case"
    );
    let (bvh, primitives, _) = build_bvh(&room.pipeline.geometry).unwrap();
    room.pipeline.bvh = bvh;
    room.pipeline.primitives = primitives;
    room.assert_bvh_texture_indices();
    let origin = Vec3::new(0.0, 1.0, 0.0);
    for saturation in [0.0, 0.5, 1.0] {
        let materials = room.materials(saturation);
        let ctx = room.ray_ctx(Some(&materials));
        let light = room.static_light();
        let mut covered = std::collections::BTreeSet::new();
        for primitive in &room.pipeline.primitives {
            let indices = &ctx.geometry.geometry.indices[primitive.index_offset as usize
                ..(primitive.index_offset + primitive.index_count) as usize];
            let target = indices
                .iter()
                .map(|&index| {
                    Vec3::from_array(ctx.geometry.geometry.vertices[index as usize].position)
                })
                .sum::<Vec3>()
                / indices.len() as f32;
            let direction = (target - origin).normalize();
            let hit = closest_hit(
                &ctx,
                origin + direction * RAY_EPSILON,
                direction,
                f32::INFINITY,
            )
            .expect("room ray reaches a face");
            let index = hit.texture_index;
            let contribution = bounce_contribution(light, hit.point, hit.normal)
                .expect("hard light illuminates the hit");
            let visibility = soft_visibility_one_sample(
                hit.point,
                hit.normal,
                light,
                0,
                |_| 0,
                DEFAULT_AREA_SAMPLE_COUNT,
                |from, to| segment_clear(&ctx, from, to),
            );
            assert!(visibility > 0.0, "fixture reference ray must be lit");
            let radiance = contribution * visibility;
            let expected = if index == red {
                radiance * materials.albedo(red) / std::f32::consts::PI
            } else {
                radiance * 0.45 / std::f32::consts::PI
            };
            let actual = sample_radiance_rgb(
                &ctx,
                origin,
                direction,
                &[light],
                None,
                100.0,
                PROBE_INDEX,
                0,
                BounceVisibility::OneSample,
            )
            .0;
            assert_eq!(
                actual.to_array().map(f32::to_bits),
                expected.to_array().map(f32::to_bits),
                "hit texture {index}, saturation {saturation}"
            );
            covered.insert(index);
        }
        assert!(
            covered.contains(&red),
            "mixed map must sample its resolved red wall"
        );
        for index in cases {
            assert!(
                covered.contains(&index),
                "missing hit coverage for texture {index}"
            );
        }
    }
}

#[path = "colour_key_tests.rs"]
mod colour_key_tests;
