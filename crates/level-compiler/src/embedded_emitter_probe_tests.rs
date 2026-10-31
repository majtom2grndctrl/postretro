// Embedded emitters at probe points: flagged lights trace their full set; each warns once.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use log::Level;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection as ScatterSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::octahedral::irradiance_array_tile_location;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_test_log_capture::{CapturedRecord, LogCapture};

use crate::animated_direct_sh_bake::{
    AnimatedDirectShBakeInputs, bake_animated_direct_sh_delta_volumes_controlled,
};
use crate::bake_control::BakeControl;
use crate::billboard_direct_scatter_bake::{
    BillboardDirectScatterBakeInputs,
    bake_animated_billboard_direct_scatter_delta_volumes_cached_controlled,
    bake_billboard_direct_scatter_volume_cached_controlled,
};
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::cache::StageCache;
use crate::delta_drop_policy::ScriptMutableDescriptorSlots;
use crate::direct_sh_bake::{
    DirectBakeInputs, bake_direct_sh_delta_volumes_controlled,
    bake_direct_sh_volume_cached_controlled,
};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::{
    DEFAULT_AREA_SAMPLE_COUNT, RAY_EPSILON, SOFT_PROBE_SAMPLES, area_sample_target, emitter_radius,
    probe_sample_direction, segment_clear, soft_visibility_early_out,
};
use crate::map_data::{
    DEFAULT_LIGHT_SIZE, FalloffModel, LightAnimation, LightType, MapLight, ShadowType,
};
use crate::partition::{Aabb, BspLeaf, BspTree};
use crate::sh_bake::{ShBakeCtx, ShConfig, incident_radiance_at_point};
use crate::texel_footprint::{EmbeddedEmitters, ProbeEarlyOut};

/// Face of the wall the lights sit beside: it faces `-X`, toward them.
const WALL_FACE_X: f32 = 0.7;
const LIGHT_HEIGHT: f64 = 1.0;

// --- scene ----------------------------------------------------------------

#[derive(Default)]
struct Scene {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    ranges: Vec<FaceIndexRange>,
}

impl Scene {
    /// A planar quad whose winding faces `cross(c1 - c0, c2 - c0)`.
    fn quad(&mut self, corners: [Vec3; 4]) {
        let base = self.vertices.len() as u32;
        for corner in corners {
            self.vertices.push(Vertex::new(
                corner.to_array(),
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        let index_offset = self.indices.len() as u32;
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        self.ranges.push(FaceIndexRange {
            index_offset,
            index_count: 6,
        });
    }

    /// An axis-aligned box of six outward-facing quads.
    fn aabb(&mut self, a: Vec3, b: Vec3) {
        let corner = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { a.x } else { b.x },
                if i & 2 == 0 { a.y } else { b.y },
                if i & 4 == 0 { a.z } else { b.z },
            )
        };
        for face in [
            [0, 4, 6, 2],
            [1, 3, 7, 5],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 2, 3, 1],
            [4, 5, 7, 6],
        ] {
            self.quad(face.map(corner));
        }
    }

    fn build(self) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: self.vertices,
                indices: self.indices,
                faces: self
                    .ranges
                    .iter()
                    .map(|_| FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    })
                    .collect(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: self.ranges,
        }
    }
}

struct Fixture {
    geometry: GeometryResult,
    bvh: Bvh<f32, 3>,
    primitives: Vec<BvhPrimitive>,
}

impl Fixture {
    /// An up-facing floor and a slab wall whose `-X` face stands at
    /// [`WALL_FACE_X`]; `post` adds a thin pillar between the wall and the
    /// open floor, a partial occluder of a light on the wall.
    fn new(post: bool) -> Self {
        Self::with_boxes(post, &[])
    }

    /// [`Fixture::new`] with `boxes` added as `(min, max)` corners.
    fn with_boxes(post: bool, boxes: &[(Vec3, Vec3)]) -> Self {
        let mut scene = Scene::default();
        scene.quad([
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(-3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(3.0, 0.0, -3.0),
        ]);
        scene.aabb(
            Vec3::new(WALL_FACE_X, 0.05, -1.5),
            Vec3::new(WALL_FACE_X + 0.3, 2.5, 1.5),
        );
        if post {
            scene.aabb(Vec3::new(0.3, 0.05, -0.06), Vec3::new(0.36, 2.5, 0.06));
        }
        for &(min, max) in boxes {
            scene.aabb(min, max);
        }
        let geometry = scene.build();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH builds");
        Self {
            geometry,
            bvh,
            primitives,
        }
    }

    fn trace(&self) -> impl Fn(Vec3, Vec3) -> bool + '_ {
        |from, to| segment_clear(&self.bvh, &self.primitives, &self.geometry, from, to)
    }

    fn classify(&self, lights: &[MapLight]) -> EmbeddedEmitters {
        EmbeddedEmitters::classify(
            lights.iter().enumerate(),
            &self.bvh,
            &self.primitives,
            &self.geometry,
            &BakeControl::unrestricted(),
        )
    }

    /// One probe point's visibility of `light` the way the probe-point bakes
    /// take it: the incident direction as the offset normal.
    fn probe_visibility(
        &self,
        light: &MapLight,
        probe: Vec3,
        seed: u64,
        early_out: ProbeEarlyOut,
    ) -> Option<f32> {
        let (_, l) = incident_radiance_at_point(light, probe)?;
        Some(soft_visibility_early_out(
            probe,
            l,
            light,
            seed,
            DEFAULT_AREA_SAMPLE_COUNT,
            early_out,
            self.trace(),
        ))
    }

    /// The reference: every sample of the full lattice traced, written out
    /// independently of the sampler's early-out logic.
    fn full_set(&self, light: &MapLight, probe: Vec3, seed: u64) -> Option<f32> {
        let (_, l) = incident_radiance_at_point(light, probe)?;
        let origin = probe + l * RAY_EPSILON;
        let n = DEFAULT_AREA_SAMPLE_COUNT;
        let trace = self.trace();
        let clear = (0..n)
            .filter(|&i| trace(origin, area_sample_target(probe, light, seed, i, n)))
            .count();
        Some(clear as f32 / n as f32)
    }

    /// The reference for an unflagged soft light, written out independently
    /// of the sampler: each of the four probes is the full-lattice index
    /// whose unrotated direction is nearest that of the matching sample of a
    /// four-point lattice, skipping indices already taken (ties to the lower
    /// index). All four clear or all four blocked store 1 or 0; otherwise
    /// every remaining index is traced and the set's clear fraction stored.
    fn four_probe_rule(&self, light: &MapLight, probe: Vec3, seed: u64) -> Option<FourProbe> {
        let (_, l) = incident_radiance_at_point(light, probe)?;
        assert!(
            emitter_radius(light) > 0.0,
            "the reference covers soft lights"
        );
        let n = DEFAULT_AREA_SAMPLE_COUNT;
        let mut probes: Vec<u32> = Vec::new();
        for k in 0..SOFT_PROBE_SAMPLES {
            let ideal = probe_sample_direction(light, k, SOFT_PROBE_SAMPLES);
            let mut best: Option<(u32, f32)> = None;
            for i in (0..n).filter(|i| !probes.contains(i)) {
                let dot = probe_sample_direction(light, i, n).dot(ideal);
                if best.is_none_or(|(_, best_dot)| dot > best_dot) {
                    best = Some((i, dot));
                }
            }
            probes.push(best.expect("the lattice outnumbers the probes").0);
        }
        let origin = probe + l * RAY_EPSILON;
        let trace = self.trace();
        let clear_at = |i: u32| trace(origin, area_sample_target(probe, light, seed, i, n));
        let probe_clear = probes.iter().filter(|&&i| clear_at(i)).count() as u32;
        if probe_clear == 0 || probe_clear == SOFT_PROBE_SAMPLES {
            return Some(FourProbe {
                visibility: probe_clear as f32 / SOFT_PROBE_SAMPLES as f32,
                escalated: false,
            });
        }
        let rest = (0..n)
            .filter(|i| !probes.contains(i))
            .filter(|&i| clear_at(i))
            .count() as u32;
        Some(FourProbe {
            visibility: (probe_clear + rest) as f32 / n as f32,
            escalated: true,
        })
    }
}

/// [`Fixture::four_probe_rule`]'s visibility, and whether the probes split.
struct FourProbe {
    visibility: f32,
    escalated: bool,
}

fn point_light(origin: [f64; 3]) -> MapLight {
    MapLight {
        origin: DVec3::from_array(origin),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 2.0,
        color: [1.0, 0.9, 0.8],
        falloff_model: FalloffModel::Linear,
        falloff_range: 6.0,
        light_size: DEFAULT_LIGHT_SIZE,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// A default-size light `gap` metres in front of the wall's face.
fn beside_wall(gap: f64) -> MapLight {
    point_light([f64::from(WALL_FACE_X) - gap, LIGHT_HEIGHT, 0.3])
}

fn animated(mut light: MapLight) -> MapLight {
    light.animation = Some(LightAnimation {
        period: 1.0,
        phase: 0.0,
        brightness: Some(vec![0.0, 1.0]),
        color: None,
        direction: None,
        start_active: true,
    });
    light
}

/// Probe points in the open air in front of the wall, each with its own seed.
fn open_probes() -> Vec<(Vec3, u64)> {
    let mut probes = Vec::new();
    for ix in 0..12 {
        for iy in 0..8 {
            for iz in 0..12 {
                let p = Vec3::new(
                    -2.6 + ix as f32 * 0.24,
                    0.15 + iy as f32 * 0.27,
                    -2.4 + iz as f32 * 0.4,
                );
                let seed = (probes.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x51;
                probes.push((p, seed));
            }
        }
    }
    probes
}

// --- the rule -------------------------------------------------------------

#[test]
fn probe_point_bakes_trace_a_flagged_light_without_early_out() {
    assert_eq!(ProbeEarlyOut::for_probe_point(true), ProbeEarlyOut::Off);
    assert_eq!(ProbeEarlyOut::for_probe_point(false), ProbeEarlyOut::Agreed);
    // Neither agreement ends a flagged light's trace.
    for clear in 0..=4 {
        assert!(!ProbeEarlyOut::Off.ends(clear, 4));
    }
}

/// The regression: a default-size light 0.2 m from a wall has about a tenth
/// of its sphere inside it. Probes in the open look at that cap through the
/// sphere, where few of the four probes aim, so the early-out stored 1.0
/// where the full set reads about 0.9. Flagged, every probe stores its
/// full-set visibility exactly.
#[test]
fn an_embedded_emitter_bakes_every_probe_at_its_full_set_visibility() {
    let fixture = Fixture::new(false);
    let light = beside_wall(0.2);
    let flags = fixture.classify(std::slice::from_ref(&light));
    assert!(flags.is_embedded(0), "the fixture embeds the emitter");
    let early_out = ProbeEarlyOut::for_probe_point(flags.is_embedded(0));

    let (mut lit, mut snapped, mut changed) = (0usize, 0usize, 0usize);
    let (mut sum_full, mut sum_old, mut max_gap) = (0.0f64, 0.0f64, 0.0f32);
    for (probe, seed) in open_probes() {
        let Some(full) = fixture.full_set(&light, probe, seed) else {
            continue;
        };
        let stored = fixture
            .probe_visibility(&light, probe, seed, early_out)
            .unwrap();
        assert_eq!(
            stored.to_bits(),
            full.to_bits(),
            "probe {probe}: stores {stored}, full set {full}"
        );
        let old = fixture
            .probe_visibility(&light, probe, seed, ProbeEarlyOut::Agreed)
            .unwrap();
        if full > 0.0 {
            lit += 1;
            sum_full += f64::from(full);
            sum_old += f64::from(old);
        }
        if old == 1.0 && full < 1.0 {
            snapped += 1;
        }
        if old != full {
            changed += 1;
            max_gap = max_gap.max(old - full);
        }
    }
    let (mean_full, mean_old) = (sum_full / lit as f64, sum_old / lit as f64);
    eprintln!(
        "embedded 0.2 m: {} of {lit} lit probes changed, {snapped} snapped to 1.0; mean \
         visibility early-out {mean_old:.4} vs full set {mean_full:.4} (+{:.1}%), worst +{max_gap:.3}",
        changed,
        (mean_old / mean_full - 1.0) * 100.0,
    );
    assert!(lit > 500, "the light reaches the probes: {lit}");
    assert!(
        snapped > lit / 4,
        "the fixture must show the defect: the early-out stores 1.0 at {snapped} of {lit} \
         probes whose full set reads less"
    );
    assert!(mean_old > mean_full + 0.02, "the early-out reads bright");
}

/// The four centre-point probes have no probe at the emitter's centre, so
/// four blocked probes need not mean an occluder: two can sit in the
/// embedded cap and two behind a partial occluder. The footprint's
/// all-blocked early-out ([`ProbeEarlyOut::Blocked`]) would store 0 there;
/// the probe-point rule stores the full set.
#[test]
fn four_blocked_probes_of_an_embedded_emitter_can_still_see_it() {
    let fixture = Fixture::new(true);
    let light = beside_wall(0.0);
    let flags = fixture.classify(std::slice::from_ref(&light));
    assert!(
        flags.is_embedded(0),
        "a light on the face embeds half its sphere"
    );
    let early_out = ProbeEarlyOut::for_probe_point(true);

    let mut cases = 0usize;
    let mut worst = 0.0f32;
    for (probe, seed) in open_probes() {
        let Some(full) = fixture.full_set(&light, probe, seed) else {
            continue;
        };
        let blocked = fixture
            .probe_visibility(&light, probe, seed, ProbeEarlyOut::Blocked)
            .unwrap();
        let stored = fixture
            .probe_visibility(&light, probe, seed, early_out)
            .unwrap();
        assert_eq!(stored.to_bits(), full.to_bits(), "probe {probe}");
        // `Blocked` stores 0 only when all four probes were blocked: one
        // clear probe would have escalated to at least 1/N.
        if blocked == 0.0 && full > 0.0 {
            cases += 1;
            worst = worst.max(full);
        }
    }
    eprintln!("all four probes blocked yet lit: {cases} probes, full set up to {worst:.3}");
    assert!(
        cases > 0,
        "the fixture must put all four probes in the cap or behind the post at some lit probe"
    );
}

/// A contract check, not proof of the rule: in true umbra (behind the wall)
/// a flagged light stores exactly 0. Every early-out stores 0 there too, so
/// this guards against the full set leaking light through the wall, not
/// against a lost flag; the tests above hold the rule itself.
#[test]
fn a_flagged_light_stores_zero_in_umbra() {
    let fixture = Fixture::new(false);
    let light = beside_wall(0.2);
    let early_out = ProbeEarlyOut::for_probe_point(true);
    let mut umbra = 0;
    for ix in 0..6 {
        for iy in 0..6 {
            for iz in 0..5 {
                let probe = Vec3::new(
                    1.2 + ix as f32 * 0.25,
                    0.3 + iy as f32 * 0.3,
                    -0.8 + iz as f32 * 0.4,
                );
                let Some(stored) = fixture.probe_visibility(&light, probe, 7, early_out) else {
                    continue;
                };
                assert_eq!(stored.to_bits(), 0.0f32.to_bits(), "probe {probe}");
                umbra += 1;
            }
        }
    }
    assert!(
        umbra > 100,
        "the wall shadows the probes behind it: {umbra}"
    );
}

/// A light whose sphere stands clear is unflagged and keeps the 4-probe
/// early-out: every probe stores exactly what an independent trace of that
/// rule stores (four spread probes, their agreement taken as 0 or 1, else the
/// whole lattice).
#[test]
fn a_clear_emitter_keeps_the_probe_early_out_bit_for_bit() {
    let fixture = Fixture::new(true);
    let light = beside_wall(0.3);
    let flags = fixture.classify(std::slice::from_ref(&light));
    assert!(!flags.is_embedded(0));
    let early_out = ProbeEarlyOut::for_probe_point(flags.is_embedded(0));
    let (mut lit, mut agreed, mut escalated) = (0, 0, 0);
    for (probe, seed) in open_probes() {
        let Some(reference) = fixture.four_probe_rule(&light, probe, seed) else {
            continue;
        };
        let stored = fixture
            .probe_visibility(&light, probe, seed, early_out)
            .unwrap();
        assert_eq!(
            stored.to_bits(),
            reference.visibility.to_bits(),
            "probe {probe}"
        );
        lit += usize::from(stored > 0.0);
        agreed += usize::from(!reference.escalated);
        escalated += usize::from(reference.escalated);
    }
    assert!(lit > 500, "{lit}");
    // The post splits some probes and not others, so both branches of the
    // rule are held to the reference.
    assert!(
        agreed > 0 && escalated > 0,
        "{agreed} agreed, {escalated} escalated"
    );
}

// --- the stages -------------------------------------------------------------

fn tree() -> BspTree {
    BspTree {
        nodes: Vec::new(),
        leaves: vec![BspLeaf {
            face_indices: Vec::new(),
            bounds: Aabb {
                min: DVec3::splat(-1000.0),
                max: DVec3::splat(1000.0),
            },
            is_solid: false,
            defining_planes: Vec::new(),
        }],
    }
}

/// Every probe-point stage's bytes for one set of lights.
#[derive(Debug, PartialEq)]
struct ProbeStages {
    direct: Vec<u8>,
    direct_deltas: Vec<u8>,
    /// Per selection slot, the concatenated sub-blocks of its entries.
    direct_delta_slots: Vec<Vec<u16>>,
    animated_direct: Vec<u8>,
    /// Per animated light, the concatenated sub-blocks of its id-45 entries.
    animated_direct_slots: Vec<Vec<u16>>,
    scatter: Vec<u8>,
    animated_scatter: Vec<u8>,
    /// Summed red of the dense direct atlas and the static scatter.
    direct_red: f64,
    scatter_red: f64,
}

/// The static probe-point flags as the pipeline computes them: the bounce
/// family's set, filtered.
fn static_probe_flags(
    sh_ctx: &ShBakeCtx<'_>,
    penumbra_floor: bool,
    control: &BakeControl,
) -> EmbeddedEmitters {
    let bounce = crate::direct_sh_bake::bounce_embedded_emitters(
        &crate::direct_sh_bake::EmitterScene::of(sh_ctx),
        penumbra_floor,
        control,
    );
    crate::direct_sh_bake::probe_point_emitters(&bounce, sh_ctx.static_lights)
}

/// Bake every probe-point stage over the wall fixture for `lights`, with
/// the emitter flags production computes (`flagged`) or none.
fn bake_stages(lights: &[MapLight], flagged: bool, cache: Option<&StageCache>) -> ProbeStages {
    let fixture = Fixture::new(false);
    let tree = tree();
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let alpha_lights = AlphaLightsNs::from_lights(lights);
    let sh_ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let control = BakeControl::unrestricted();
    let static_flags = static_probe_flags(&sh_ctx, flagged, &control);
    let animated_flags = crate::animated_direct_sh_bake::embedded_emitters(
        &sh_ctx,
        &animated_lights,
        flagged,
        &control,
    );
    if flagged {
        // Keyed by static position and by animated position: the flagged
        // static light sits at static 1 and the flagged animated light at
        // animated 1 (`stage_lights`).
        assert!(
            !static_flags.is_embedded(0)
                && static_flags.is_embedded(1)
                && !static_flags.is_embedded(2),
            "the fixture flags static light 1 alone: {static_flags:?}"
        );
        assert!(
            !animated_flags.is_embedded(0) && animated_flags.is_embedded(1),
            "the fixture flags animated light 1 alone: {animated_flags:?}"
        );
    }
    let config = ShConfig { probe_spacing: 0.5 };
    let direct_inputs = DirectBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
        embedded_emitters: &static_flags,
    };
    let direct = bake_direct_sh_volume_cached_controlled(&direct_inputs, &config, cache, &control);
    let selected = EntityShadowLightsSection {
        light_indices: (0..alpha_lights.entries().len() as u32).collect(),
    };
    let (deltas, _) = bake_direct_sh_delta_volumes_controlled(
        &direct_inputs,
        &config,
        &alpha_lights,
        &selected,
        &control,
    )
    .expect("the selected static lights reach probes");
    let stride = deltas.delta_subblocks.len() / deltas.affinity_lights.len();
    let mut direct_delta_slots = vec![Vec::new(); static_lights.entries().len()];
    for (entry, &slot) in deltas.affinity_lights.iter().enumerate() {
        direct_delta_slots[slot as usize]
            .extend_from_slice(&deltas.delta_subblocks[entry * stride..(entry + 1) * stride]);
    }
    let mutable = ScriptMutableDescriptorSlots::empty(animated_lights.len());
    let animated_direct = bake_animated_direct_sh_delta_volumes_controlled(
        &AnimatedDirectShBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            animated_lights: &animated_lights,
            mutable_descriptors: &mutable,
            embedded_emitters: &animated_flags,
        },
        &config,
        &control,
    )
    .expect("the animated lights reach probes");
    let animated_stride =
        animated_direct.delta_subblocks.len() / animated_direct.affinity_lights.len();
    let mut animated_direct_slots = vec![Vec::new(); animated_lights.len()];
    for (entry, &light) in animated_direct.affinity_lights.iter().enumerate() {
        animated_direct_slots[light as usize].extend_from_slice(
            &animated_direct.delta_subblocks
                [entry * animated_stride..(entry + 1) * animated_stride],
        );
    }
    let scatter_inputs = BillboardDirectScatterBakeInputs {
        sh_ctx: &sh_ctx,
        portals: &[],
        animated_lights: &animated_lights,
        static_embedded_emitters: &static_flags,
        animated_embedded_emitters: &animated_flags,
    };
    let scatter = bake_billboard_direct_scatter_volume_cached_controlled(
        &scatter_inputs,
        &config,
        true,
        cache,
        &control,
    )
    .expect("static lights produce a scatter base");
    let animated_scatter = bake_animated_billboard_direct_scatter_delta_volumes_cached_controlled(
        &scatter_inputs,
        &config,
        &animated_direct,
        cache,
        &control,
    )
    .expect("the animated scatter bakes");

    let direct_red = direct
        .atlas
        .as_chunks::<8>()
        .0
        .iter()
        .map(|texel| f64::from(f16_to_f32(u16::from_le_bytes([texel[0], texel[1]]))))
        .sum();
    let scatter_red = scatter
        .scatter_rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|rgba| f64::from(f16_to_f32(rgba[0])))
        .sum();
    ProbeStages {
        direct: direct.to_bytes(),
        direct_deltas: deltas.to_bytes(),
        direct_delta_slots,
        animated_direct: animated_direct.to_bytes(),
        animated_direct_slots,
        scatter: scatter.to_bytes(),
        animated_scatter: animated_scatter.to_bytes(),
        direct_red,
        scatter_red,
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Lights whose key spaces all differ, so a stage that reads a flag under
/// the wrong index moves the wrong light:
///
/// | light            | source | static | direct | animated |
/// |------------------|--------|--------|--------|----------|
/// | `sdf` static     | 0      | 0      | –      | –        |
/// | clear animated   | 1      | –      | –      | 0        |
/// | flagged static   | 2      | 1      | 0      | –        |
/// | flagged animated | 3      | –      | –      | 1        |
/// | clear static     | 4      | 2      | 1      | –        |
///
/// The direct-delta selection slots follow the direct column.
fn stage_lights() -> Vec<MapLight> {
    let mut sdf = point_light([-2.0, 1.5, -2.0]);
    sdf.shadow_type = ShadowType::Sdf;
    let mut clear_animated = point_light([-1.8, 1.2, 1.6]);
    clear_animated.color = [1.0, 0.4, 0.3];
    let mut clear = point_light([-1.5, 1.4, -1.2]);
    clear.color = [0.3, 0.6, 1.0];
    vec![
        sdf,
        animated(clear_animated),
        beside_wall(0.2),
        animated(point_light([f64::from(WALL_FACE_X) - 0.15, 0.8, -0.6])),
        clear,
    ]
}

/// Every probe-point stage moves with the flag, and only the flagged
/// lights' bytes do: the clear static and animated lights' delta sub-blocks
/// are identical with and without the flags. Flagged probes read dimmer.
#[test]
fn every_probe_point_stage_traces_flagged_lights_and_keeps_the_rest() {
    let lights = stage_lights();
    let flagged = bake_stages(&lights, true, None);
    let unflagged = bake_stages(&lights, false, None);

    assert_ne!(flagged.direct, unflagged.direct, "direct SH");
    assert_ne!(
        flagged.direct_deltas, unflagged.direct_deltas,
        "direct SH deltas"
    );
    assert_ne!(
        flagged.animated_direct, unflagged.animated_direct,
        "animated direct SH"
    );
    assert_ne!(flagged.scatter, unflagged.scatter, "billboard scatter");
    assert_ne!(
        flagged.animated_scatter, unflagged.animated_scatter,
        "animated billboard scatter"
    );
    assert_ne!(
        flagged.direct_delta_slots[0], unflagged.direct_delta_slots[0],
        "the flagged light's deltas move"
    );
    assert!(!flagged.direct_delta_slots[1].is_empty());
    assert_eq!(
        flagged.direct_delta_slots[1], unflagged.direct_delta_slots[1],
        "the clear light's deltas keep their bytes"
    );
    assert_ne!(
        flagged.animated_direct_slots[1], unflagged.animated_direct_slots[1],
        "the flagged animated light's deltas move"
    );
    assert!(!flagged.animated_direct_slots[0].is_empty());
    assert_eq!(
        flagged.animated_direct_slots[0], unflagged.animated_direct_slots[0],
        "the clear animated light's deltas keep their bytes"
    );
    assert!(
        flagged.direct_red < unflagged.direct_red && flagged.scatter_red < unflagged.scatter_red,
        "flagged probes read dimmer: direct {} vs {}, scatter {} vs {}",
        flagged.direct_red,
        unflagged.direct_red,
        flagged.scatter_red,
        unflagged.scatter_red,
    );
    eprintln!(
        "stage energy (red), flagged / unflagged: direct {:.5}, scatter {:.5}",
        flagged.direct_red / unflagged.direct_red,
        flagged.scatter_red / unflagged.scatter_red,
    );
}

#[test]
fn flagged_probe_point_stages_are_byte_identical_across_worker_counts() {
    let lights = stage_lights();
    let bake = |workers: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap()
            .install(|| bake_stages(&lights, true, None))
    };
    let single = bake(1);
    assert_eq!(single, bake(4), "1 vs 4 workers");
    assert_eq!(single, bake(7), "1 vs 7 workers");
}

/// Warm equals cold with the flags, and the cached stages key the flagged
/// set: a cache warmed without flags (as `--no-penumbra-floor` bakes) serves
/// no flagged bake, and the reverse.
#[test]
fn flagged_probe_point_stages_warm_equal_cold_and_key_the_flags() {
    let lights = stage_lights();
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");

    let cold = bake_stages(&lights, true, None);
    let cold_unflagged = bake_stages(&lights, false, None);
    let warm_unflagged = bake_stages(&lights, false, Some(&cache));
    assert_eq!(warm_unflagged, cold_unflagged, "unflagged miss");
    let miss = bake_stages(&lights, true, Some(&cache));
    assert_eq!(miss, cold, "a flagged bake misses the unflagged entries");
    let hit = bake_stages(&lights, true, Some(&cache));
    assert_eq!(hit, cold, "flagged hit");
    assert_eq!(
        bake_stages(&lights, false, Some(&cache)),
        cold_unflagged,
        "unflagged hit"
    );
}

// --- culled reach -----------------------------------------------------------

/// Thin posts across the open floor, far from the wall: each hides a sliver
/// of the clear light's sphere from the probes behind it, where the four
/// probes can agree on clear while the full set does not.
fn far_posts() -> Vec<(Vec3, Vec3)> {
    (0..5)
        .map(|i| {
            let x = -2.8 + 0.35 * i as f32;
            (Vec3::new(x, 0.05, -2.05), Vec3::new(x + 0.05, 2.5, -2.0))
        })
        .collect()
}

/// Direct SH (id 35) and static scatter (id 47) over the far-post fixture,
/// with `flags` as the static emitter flags.
fn bake_direct_and_scatter(
    sh_ctx: &ShBakeCtx<'_>,
    config: &ShConfig,
    flags: &EmbeddedEmitters,
) -> (DirectShVolumeSection, ScatterSection) {
    let control = BakeControl::unrestricted();
    let direct = bake_direct_sh_volume_cached_controlled(
        &DirectBakeInputs {
            sh_ctx,
            portals: &[],
            embedded_emitters: flags,
        },
        config,
        None,
        &control,
    );
    let scatter = bake_billboard_direct_scatter_volume_cached_controlled(
        &BillboardDirectScatterBakeInputs {
            sh_ctx,
            portals: &[],
            animated_lights: sh_ctx.animated_lights,
            static_embedded_emitters: flags,
            animated_embedded_emitters: &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
        },
        config,
        false,
        None,
        &control,
    )
    .expect("static lights produce a scatter base");
    (direct, scatter)
}

/// Probe `probe`'s whole octahedral tile in the uncompressed id-35 atlas.
fn direct_tile(section: &DirectShVolumeSection, probe: usize) -> Vec<u8> {
    const TEXEL_BYTES: usize = 8;
    let [layer, tile_x, tile_y] =
        irradiance_array_tile_location(probe, section.tiles_per_layer, section.atlas_tiles_per_row);
    let (width, height) = (
        section.atlas_dimensions[0] as usize,
        section.atlas_dimensions[1] as usize,
    );
    let dim = section.tile_dimension as usize;
    let layer_base = layer as usize * width * height;
    let mut tile = Vec::with_capacity(dim * dim * TEXEL_BYTES);
    for row in 0..dim {
        let texel = layer_base + (tile_y as usize * dim + row) * width + tile_x as usize * dim;
        tile.extend_from_slice(&section.atlas[texel * TEXEL_BYTES..(texel + dim) * TEXEL_BYTES]);
    }
    tile
}

/// A probe's light list is the reach-culled subset of the direct lights,
/// while each light's early-out is looked up by its direct index. A flagged
/// light whose short range leaves some affinity cells reaching only the clear
/// light must leave those cells' id-35 tiles and id-47 scatter bit-identical
/// to an unflagged bake. Flagging the clear light instead moves them, so the
/// equality would catch a flag read under the wrong index.
#[test]
fn cells_only_the_clear_light_reaches_keep_their_bytes() {
    use crate::direct_sh_bake::{build_reach_index, static_direct_lights};
    use crate::sh_bake::{probe_grid_layout, static_light_refs};

    let mut short = beside_wall(0.2);
    short.falloff_range = 1.0;
    let mut clear = point_light([-1.5, 1.4, -1.2]);
    clear.color = [0.3, 0.6, 1.0];
    let lights = vec![short, clear];

    let fixture = Fixture::with_boxes(false, &far_posts());
    let tree = tree();
    let exterior = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    let sh_ctx = ShBakeCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let config = ShConfig { probe_spacing: 0.5 };
    let flags = static_probe_flags(&sh_ctx, true, &BakeControl::unrestricted());
    assert!(flags.is_embedded(0) && !flags.is_embedded(1), "{flags:?}");

    // The probes whose affinity cell the clear light reaches and the short
    // light does not.
    let layout = probe_grid_layout(&sh_ctx, &config);
    let static_refs = static_light_refs(&sh_ctx);
    let (direct_lights, global_indices) = static_direct_lights(&static_refs);
    assert_eq!(global_indices, [0, 1]);
    let reach = build_reach_index(
        &DirectBakeInputs {
            sh_ctx: &sh_ctx,
            portals: &[],
            embedded_emitters: &flags,
        },
        &direct_lights,
        config.probe_spacing,
    );
    let [nx, ny, _] = layout.dims.map(|d| d as usize);
    let clear_only: Vec<usize> = (0..layout.total_probes())
        .filter(|&probe| layout.validity[probe] != 0)
        .filter(|&probe| {
            let (x, y, z) = (probe % nx, probe / nx % ny, probe / (nx * ny));
            reach
                .cell_lights
                .get(reach.cell_for_probe(x as u32, y as u32, z as u32))
                .is_some_and(|cell| cell.as_slice() == [1])
        })
        .collect();
    assert!(
        clear_only.len() > 50,
        "the short range leaves cells to the clear light: {}",
        clear_only.len()
    );

    let (flagged_direct, flagged_scatter) = bake_direct_and_scatter(&sh_ctx, &config, &flags);
    let (plain_direct, plain_scatter) =
        bake_direct_and_scatter(&sh_ctx, &config, &EmbeddedEmitters::default());
    let misread = EmbeddedEmitters::from_flags([(1, flags.fraction(0))]);
    let (misread_direct, misread_scatter) = bake_direct_and_scatter(&sh_ctx, &config, &misread);
    assert_eq!(flagged_scatter.grid_dimensions, layout.dims);

    let scatter_at = |section: &ScatterSection, probe: usize| {
        section.scatter_rgba[probe * 4..probe * 4 + 4].to_vec()
    };
    let (mut direct_moved, mut scatter_moved) = (0, 0);
    for &probe in &clear_only {
        assert_eq!(
            direct_tile(&flagged_direct, probe),
            direct_tile(&plain_direct, probe),
            "id-35 probe {probe}"
        );
        assert_eq!(
            scatter_at(&flagged_scatter, probe),
            scatter_at(&plain_scatter, probe),
            "id-47 probe {probe}"
        );
        direct_moved +=
            usize::from(direct_tile(&misread_direct, probe) != direct_tile(&plain_direct, probe));
        scatter_moved +=
            usize::from(scatter_at(&misread_scatter, probe) != scatter_at(&plain_scatter, probe));
    }
    assert!(
        direct_moved > 0 && scatter_moved > 0,
        "the clear light's own flag moves its cells (id 35: {direct_moved}, id 47: \
         {scatter_moved} of {} probes)",
        clear_only.len()
    );
    // And the flag still moves the short light's own cells.
    assert_ne!(flagged_direct.atlas, plain_direct.atlas);
}

// --- warnings through the pipeline ------------------------------------------

/// An axis-aligned brush from `min` to `max`, Quake map units.
pub(crate) fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    const TEXTURE: &str = "concrete_pavement_036";
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// A sealed 256 u room whose one light (entity 2) stands 6 u off the inside
/// face of the west wall, so about a fifth of its default 0.25 m sphere lies
/// in the wall. `light_keys` are extra lines on the light.
fn wall_light_map(light_keys: &str) -> String {
    let brushes = [
        box_brush([-16, -16, -16], [272, 272, 0]),
        box_brush([-16, -16, 128], [272, 272, 144]),
        box_brush([-16, -16, 0], [0, 272, 128]),
        box_brush([256, -16, 0], [272, 272, 128]),
        box_brush([0, -16, 0], [256, 0, 128]),
        box_brush([0, 256, 0], [256, 272, 128]),
    ];
    [
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"128 128 24\"\n}".to_owned(),
        format!(
            "{{\n\"classname\" \"light\"\n\"origin\" \"6 128 64\"\n\"light\" \"300\"\n\
             \"_falloff_range\" \"256\"\n{light_keys}}}"
        ),
    ]
    .join("\n")
        + "\n"
}

/// One in-process compile of `map_text` through the production pipeline,
/// with its stage cache under `root`, and every record it logged on this
/// thread: the caller thread, where the emitter warnings must land.
fn compile_in_process(root: &std::path::Path, map_text: &str) -> Vec<CapturedRecord> {
    compile_in_process_with(root, map_text, true)
}

/// [`compile_in_process`], with the stage cache under `root` or, when
/// `cached` is false, none (`--no-cache`: the cold SH base bake).
fn compile_in_process_with(
    root: &std::path::Path,
    map_text: &str,
    cached: bool,
) -> Vec<CapturedRecord> {
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).expect("maps directory");
    std::fs::create_dir_all(root.join("baked").join("materials")).expect("baked root");
    let map = maps.join("embedded.map");
    std::fs::write(&map, map_text).expect("write the map");
    let path = |name: &str| root.join(name).to_string_lossy().into_owned();
    let cache_args = if cached {
        vec!["--cache-dir".to_owned(), path("cache")]
    } else {
        vec!["--no-cache".to_owned()]
    };
    let args = crate::parse_args_from(
        [map.to_string_lossy().into_owned()]
            .into_iter()
            .chain(cache_args)
            .chain([
                "--baked-root".to_owned(),
                path("baked"),
                "-o".to_owned(),
                path("embedded.prl"),
                "--sh-probe-spacing".to_owned(),
                "4".to_owned(),
                "--lightmap-density".to_owned(),
                "0.25".to_owned(),
            ]),
    )
    .expect("arguments parse");
    let cache = crate::construct_stage_cache(&args);
    assert_eq!(cache.is_some(), cached);
    let capture = LogCapture::start();
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        cache,
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("the map compiles");
    capture.records()
}

/// The embedded-emitter warnings among `records`.
fn emitter_warnings(records: &[CapturedRecord]) -> Vec<String> {
    records
        .iter()
        .filter(|record| {
            record.level == Level::Warn && record.message.contains("emitter inside geometry")
        })
        .map(|record| record.message.clone())
        .collect()
}

// A static light both the probe stages and the lightmap stage flag warns
// once per compile, not twice, naming both bake families, cold and with
// every memo warm.
#[test]
fn a_light_both_stage_families_flag_warns_once_per_compile() {
    let root = tempfile::tempdir().expect("temp root");
    let map = wall_light_map("");
    for build in ["cold", "warm"] {
        let records = compile_in_process(root.path(), &map);
        let warnings = emitter_warnings(&records);
        assert_eq!(warnings.len(), 1, "{build}: {warnings:?}");
        for part in [
            "static light entity 2 'light'",
            "0.250 m emitter sphere",
            "makes its lit lightmap texels and the SH and billboard probes it lights trace",
        ] {
            assert!(
                warnings[0].contains(part),
                "{build}: missing {part:?}: {}",
                warnings[0]
            );
        }
    }
}

// An animated-only map has no lightmap placement, so the weight-map stage
// has no chunk and never runs: the light only the probe stages flag still
// warns once per compile, from the pipeline, naming only the probes.
#[test]
fn a_light_only_the_probe_stages_flag_warns_once_per_compile() {
    let root = tempfile::tempdir().expect("temp root");
    let map = wall_light_map("\"style\" \"2\"\n");
    for build in ["cold", "warm"] {
        let records = compile_in_process(root.path(), &map);
        assert!(
            !records
                .iter()
                .any(|record| record.message.contains("[cache] animated_lm_weight_maps")),
            "{build}: the fixture must leave the weight-map stage nothing to bake"
        );
        let warnings = emitter_warnings(&records);
        assert_eq!(warnings.len(), 1, "{build}: {warnings:?}");
        for part in [
            "animated light entity 2 'light'",
            "makes the SH and billboard probes it lights trace",
        ] {
            assert!(
                warnings[0].contains(part),
                "{build}: missing {part:?}: {}",
                warnings[0]
            );
        }
    }
}

// --- the SH bounce patches ----------------------------------------------------

fn sdf(mut light: MapLight) -> MapLight {
    light.shadow_type = ShadowType::Sdf;
    light
}

/// The bounce fixture: the floor and wall, a point light and an
/// `sdf`-shadowed light whose emitter spheres both reach into the wall.
pub(crate) fn embedded_bounce_fixture() -> (GeometryResult, Vec<MapLight>) {
    let shadowed = sdf(point_light([
        f64::from(WALL_FACE_X) - 0.2,
        LIGHT_HEIGHT,
        -0.9,
    ]));
    (
        Fixture::new(false).geometry,
        vec![beside_wall(0.2), shadowed],
    )
}

// The pipeline classifies static lights once, under the bounce family's
// rule, and filters the probe-point set from it. That set must be the one
// testing the probe-point lights alone gives (`StaticLightMap` lights,
// classified under the probe rule): the same keys, fractions and order, so
// the same `hash_keys` bytes, or ids 35, 41, 47 and 48 and their cache keys
// would move.
#[test]
fn the_probe_point_set_filtered_from_the_bounce_set_matches_its_own_classification() {
    use crate::direct_sh_bake::{EmitterScene, bounce_embedded_emitters, probe_point_emitters};

    let fixture = Fixture::new(false);
    let (_, mut lights) = embedded_bounce_fixture();
    // After the `sdf` light, so a kept key follows a dropped one: a
    // flagged point light, a clear one and a hard one.
    lights.push(point_light([
        f64::from(WALL_FACE_X) - 0.2,
        LIGHT_HEIGHT,
        0.9,
    ]));
    lights.push(point_light([-1.5, LIGHT_HEIGHT, 0.3]));
    let mut hard = beside_wall(0.1);
    hard.light_size = 0.0;
    lights.push(hard);
    let static_lights = StaticBakedLights::from_lights(&lights);
    let scene = EmitterScene {
        static_lights: &static_lights,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
    };
    let control = BakeControl::unrestricted();
    let key_bytes = |set: &EmbeddedEmitters| {
        let mut hasher = blake3::Hasher::new();
        set.hash_keys(&mut hasher);
        hasher.finalize()
    };

    let bounce = bounce_embedded_emitters(&scene, true, &control);
    let derived = probe_point_emitters(&bounce, &static_lights);
    let alone = EmbeddedEmitters::classify(
        static_lights
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.light.shadow_type == ShadowType::StaticLightMap)
            .map(|(index, entry)| (index, entry.light)),
        &fixture.bvh,
        &fixture.primitives,
        &fixture.geometry,
        &control,
    );
    assert!(alone.is_embedded(0), "{alone:?}");
    assert!(
        bounce.is_embedded(1) && !derived.is_embedded(1),
        "the filter must drop the flagged sdf light: {bounce:?} {derived:?}"
    );
    assert_eq!(derived, alone, "keys, fractions and order");
    assert_eq!(key_bytes(&derived), key_bytes(&alone));

    // `--no-penumbra-floor`: neither set tests a light.
    let off = bounce_embedded_emitters(&scene, false, &control);
    assert_eq!(off, EmbeddedEmitters::default());
    assert_eq!(
        probe_point_emitters(&off, &static_lights),
        EmbeddedEmitters::default()
    );
}

// A flagged point light and a flagged `sdf`-shadowed
// light shade their bounce patches at the full lattice's visibility, the
// no-early-out reference written out here; the probe-point set still flags
// only the point light, so the `sdf` light reaches no probe-point key
// (ids 35, 41, 47, 48 fold only that set).
#[test]
fn flagged_point_and_sdf_lights_shade_their_patches_at_full_set_visibility() {
    use crate::direct_sh_bake::{EmitterScene, bounce_embedded_emitters, probe_point_emitters};
    use crate::sh_bake::{
        RaytracingCtx, bounce_contribution, reflected_radiance, soft_visibility_seed,
    };
    use crate::surface_radiance::{PatchSet, Side, shade_light_layer};

    let fixture = Fixture::new(false);
    let (_, lights) = embedded_bounce_fixture();
    let point = lights[0].clone();
    let control = BakeControl::unrestricted();
    let classify = |lights: &[MapLight]| {
        let static_lights = StaticBakedLights::from_lights(lights);
        let scene = EmitterScene {
            static_lights: &static_lights,
            bvh: &fixture.bvh,
            primitives: &fixture.primitives,
            geometry: &fixture.geometry,
        };
        let bounce = bounce_embedded_emitters(&scene, true, &control);
        (probe_point_emitters(&bounce, &static_lights), bounce)
    };
    let (probes, bounce) = classify(&lights);
    assert!(
        probes.is_embedded(0) && !probes.is_embedded(1),
        "{probes:?}"
    );
    assert!(bounce.is_embedded(0) && bounce.is_embedded(1), "{bounce:?}");
    let (point_only, _) = classify(std::slice::from_ref(&point));
    assert_eq!(
        probes, point_only,
        "the sdf light leaves the probe-point flags, so their keys"
    );

    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
    };
    let patches = PatchSet::build(&fixture.geometry);
    let reach = crate::surface_radiance::PatchReachIndex::new(patches.patches());
    let trace = |from, to| crate::sh_bake::segment_clear(&ctx, from, to);
    let n = DEFAULT_AREA_SAMPLE_COUNT;
    for (index, light) in lights.iter().enumerate() {
        let layer = shade_light_layer(
            &ctx,
            &patches,
            &reach.in_reach(light),
            light,
            index as u64,
            bounce.is_embedded(index),
            &control,
        );
        let mut expected = Vec::new();
        let mut penumbra = 0;
        for (at, patch) in patches.patches().iter().enumerate() {
            for side in Side::BOTH {
                let normal = side.normal(patch.normal);
                let Some(contribution) = bounce_contribution(light, patch.centroid, normal) else {
                    continue;
                };
                let seed = soft_visibility_seed(patch.seed_id(), side as u64, index as u64);
                let origin = patch.centroid + normal * RAY_EPSILON;
                let clear = (0..n)
                    .filter(|&i| {
                        trace(
                            origin,
                            area_sample_target(patch.centroid, light, seed, i, n),
                        )
                    })
                    .count() as u32;
                if clear == 0 {
                    continue;
                }
                penumbra += usize::from(clear < n);
                let visibility = clear as f32 / n as f32;
                let radiance =
                    reflected_radiance(None, patch.texture_index, contribution * visibility);
                expected.push((at as u32, side, radiance.to_array()));
            }
        }
        assert!(
            penumbra > 0,
            "light {index}: some patch sees part of the emitter"
        );
        let actual: Vec<_> = layer
            .entries
            .iter()
            .map(|entry| (entry.patch, entry.side, entry.radiance))
            .collect();
        assert_eq!(actual, expected, "light {index}");
    }
}

// An `sdf`-shadowed light only the bounce family flags warns once per
// compile, cold and warm, naming the SH bounce patches; the point light the
// probe-point and lightmap stages flag keeps its own line, word for word,
// and neither warns twice.
#[test]
fn an_sdf_light_only_the_bounce_flags_warns_once_naming_the_bounce_bake() {
    const SDF_LIGHT: &str = "{\n\"classname\" \"light\"\n\"origin\" \"250 128 64\"\n\
        \"light\" \"300\"\n\"_falloff_range\" \"256\"\n\"_shadow_type\" \"sdf\"\n}\n";
    let root = tempfile::tempdir().expect("temp root");
    let point_only = emitter_warnings(&compile_in_process_with(
        &root.path().join("point"),
        &wall_light_map(""),
        false,
    ));
    assert_eq!(point_only.len(), 1, "{point_only:?}");
    let map = wall_light_map("") + SDF_LIGHT;
    let both = root.path().join("both");
    for (build, cached) in [("cold", false), ("warm-first", true), ("warm", true)] {
        let warnings = emitter_warnings(&compile_in_process_with(&both, &map, cached));
        assert_eq!(warnings.len(), 2, "{build}: {warnings:?}");
        assert!(warnings.contains(&point_only[0]), "{build}: {warnings:?}");
        let bounce: Vec<&String> = warnings
            .iter()
            .filter(|line| line.contains("static light entity 3 'light'"))
            .collect();
        assert_eq!(bounce.len(), 1, "{build}: {warnings:?}");
        for part in [
            "0.250 m emitter sphere",
            "makes the SH bounce patches it lights trace every shadow sample",
        ] {
            assert!(
                bounce[0].contains(part),
                "{build}: missing {part:?}: {}",
                bounce[0]
            );
        }
    }
}
