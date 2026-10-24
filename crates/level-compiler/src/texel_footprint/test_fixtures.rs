// Footprint test fixtures: synthetic floors, plates, poles and walls baked through the layer path.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::cell::Cell;
use std::sync::OnceLock;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::{
    ClipBand, FootprintOutcome, ProbeEarlyOut, TexelFrame, TexelSite, footprint_for,
    footprint_visibility, lead_in_clear,
};
use crate::bake_control::BakeControl;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::chart_raster::{
    CHART_PADDING_TEXELS, ChartPlacement, chart_texel_seed, chart_texel_world_position,
};
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::{
    BlockLayout, Chart, light_texel_is_covered, segment_clear, segment_clear_remembering,
};
use crate::lightmap_layer::{SharedAtlas, bake_light_layer_chart_controlled};
use crate::map_data::{DEFAULT_LIGHT_SIZE, FalloffModel, LightType, MapLight, ShadowType};
use crate::texel_step_detector::VisibilityGrid;

/// The default atlas density, so fixtures measure ramps at the pitch
/// authors get.
pub(super) const PITCH: f32 = 0.04;

/// Gap under a "touching" occluder: above the ray lift, so the clip segment
/// runs beneath it, and so small that no emitter widens its edge (a 0.25 m
/// light 2 m up spreads it by 0.0005 m, 1/80 texel).
pub(super) const CONTACT_GAP: f32 = 0.002;

// --- geometry -------------------------------------------------------------

/// Faces accumulated into one `GeometryResult`.
#[derive(Default)]
pub(super) struct SceneBuilder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    ranges: Vec<FaceIndexRange>,
}

impl SceneBuilder {
    /// A planar quad, corners in winding order, facing `normal`.
    pub(super) fn quad(&mut self, corners: [Vec3; 4], normal: Vec3) {
        self.polygon(&corners, normal);
    }

    /// A planar convex polygon, corners in winding order, facing `normal`,
    /// fanned from its first corner as geometry extraction fans a face.
    pub(super) fn polygon(&mut self, corners: &[Vec3], normal: Vec3) {
        let base = self.vertices.len() as u32;
        let tangent = (corners[1] - corners[0]).normalize_or_zero();
        for corner in corners {
            self.vertices.push(Vertex::new(
                corner.to_array(),
                [0.0, 0.0],
                normal.to_array(),
                tangent.to_array(),
                true,
                [0.0, 0.0],
                0,
            ));
        }
        let index_offset = self.indices.len() as u32;
        for i in 1..corners.len() as u32 - 1 {
            self.indices.extend([base, base + i, base + i + 1]);
        }
        self.ranges.push(FaceIndexRange {
            index_offset,
            index_count: self.indices.len() as u32 - index_offset,
        });
    }

    /// A box spanning `local_min..local_max` in a frame turned `yaw` radians
    /// about +Y and moved to `anchor`: six outward-facing quads.
    pub(super) fn oriented_box(
        &mut self,
        anchor: Vec3,
        yaw: f32,
        local_min: Vec3,
        local_max: Vec3,
    ) {
        let (sin, cos) = yaw.sin_cos();
        let axis_x = Vec3::new(cos, 0.0, -sin);
        let axis_z = Vec3::new(sin, 0.0, cos);
        let at = |x: f32, y: f32, z: f32| anchor + axis_x * x + Vec3::Y * y + axis_z * z;
        let (a, b) = (local_min, local_max);
        let corner = |i: usize| {
            at(
                if i & 1 == 0 { a.x } else { b.x },
                if i & 2 == 0 { a.y } else { b.y },
                if i & 4 == 0 { a.z } else { b.z },
            )
        };
        let faces: [([usize; 4], Vec3); 6] = [
            ([0, 4, 6, 2], -axis_x),
            ([1, 3, 7, 5], axis_x),
            ([0, 1, 5, 4], -Vec3::Y),
            ([2, 6, 7, 3], Vec3::Y),
            ([0, 2, 3, 1], -axis_z),
            ([4, 5, 7, 6], axis_z),
        ];
        for (corners, normal) in faces {
            self.quad(corners.map(corner), normal);
        }
    }

    /// An axis-aligned box.
    pub(super) fn aabb(&mut self, min: Vec3, max: Vec3) {
        self.oriented_box(Vec3::ZERO, 0.0, min, max);
    }

    pub(super) fn build(self) -> GeometryResult {
        let faces = self
            .ranges
            .iter()
            .map(|_| FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            })
            .collect();
        GeometryResult {
            geometry: GeometrySection {
                vertices: self.vertices,
                indices: self.indices,
                faces,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: self.ranges,
        }
    }
}

// --- receiver ---------------------------------------------------------------

/// An up-facing floor receiver `width × height` texels at `PITCH`, its first
/// texel's corner at `(x0, 0, z0)`. Texel `(tx, ty)` is centred at texel
/// coordinates `(tx + 0.5, ty + 0.5)`; [`Receiver::texel_coords`] maps a
/// world point there.
#[derive(Debug, Clone, Copy)]
pub(super) struct Receiver {
    pub(super) x0: f32,
    pub(super) z0: f32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pitch: f32,
}

impl Receiver {
    pub(super) fn centred(width: u32, height: u32) -> Self {
        Self {
            x0: -(width as f32) * PITCH * 0.5,
            z0: -(height as f32) * PITCH * 0.5,
            width,
            height,
            pitch: PITCH,
        }
    }

    pub(super) fn chart(&self) -> Chart {
        let padding = 2 * CHART_PADDING_TEXELS;
        Chart {
            origin: Vec3::new(self.x0, 0.0, self.z0),
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [
                self.width as f32 * self.pitch,
                self.height as f32 * self.pitch,
            ],
            normal: Vec3::Y,
            width_texels: self.width + padding,
            height_texels: self.height + padding,
            leaf_index: 0,
            window: None,
        }
    }

    /// The floor quad under the chart, so the receiver is real geometry.
    pub(super) fn add_floor(&self, scene: &mut SceneBuilder) {
        let (x1, z1) = (
            self.x0 + self.width as f32 * self.pitch,
            self.z0 + self.height as f32 * self.pitch,
        );
        scene.quad(
            [
                Vec3::new(self.x0, 0.0, self.z0),
                Vec3::new(self.x0, 0.0, z1),
                Vec3::new(x1, 0.0, z1),
                Vec3::new(x1, 0.0, self.z0),
            ],
            Vec3::Y,
        );
    }

    /// Continuous texel coordinates of a world point on the floor.
    pub(super) fn texel_coords(&self, p: Vec3) -> [f64; 2] {
        [
            f64::from((p.x - self.x0) / self.pitch),
            f64::from((p.z - self.z0) / self.pitch),
        ]
    }
}

// --- lights ---------------------------------------------------------------

pub(super) fn point_light(origin: [f64; 3], light_size: f32) -> MapLight {
    MapLight {
        origin: DVec3::from_array(origin),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size,
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

pub(super) fn hard_point(origin: [f64; 3]) -> MapLight {
    point_light(origin, 0.0)
}

pub(super) fn default_point(origin: [f64; 3]) -> MapLight {
    point_light(origin, DEFAULT_LIGHT_SIZE)
}

/// A hard spot aimed straight down with a cone wide enough to light the
/// whole fixture.
pub(super) fn hard_spot(origin: [f64; 3]) -> MapLight {
    let mut light = point_light(origin, 0.0);
    light.light_type = LightType::Spot;
    light.cone_direction = Some([0.0, -1.0, 0.0]);
    light.cone_angle_inner = Some(1.2);
    light.cone_angle_outer = Some(1.4);
    light
}

/// A `_angular_diameter 0` sun shining along `aim`.
pub(super) fn hard_sun(aim: [f32; 3]) -> MapLight {
    let mut light = point_light([0.0, 50.0, 0.0], 0.0);
    light.light_type = LightType::Directional;
    light.cone_direction = Some(Vec3::from(aim).normalize().to_array());
    light
}

/// Unit vector from the floor toward `light`'s centre, seen from `at`.
pub(super) fn toward_light(light: &MapLight, at: Vec3) -> Vec3 {
    match light.light_type {
        LightType::Directional => -Vec3::from(light.cone_direction.unwrap()).normalize(),
        LightType::Point | LightType::Spot => (light.origin.as_vec3() - at).normalize(),
    }
}

// --- baking ---------------------------------------------------------------

/// A fixture's geometry, BVH and receiver.
pub(super) struct Fixture {
    pub(super) geometry: GeometryResult,
    pub(super) bvh: Bvh<f32, 3>,
    pub(super) primitives: Vec<BvhPrimitive>,
    pub(super) receiver: Receiver,
}

impl Fixture {
    pub(super) fn new(scene: SceneBuilder, receiver: Receiver) -> Self {
        let geometry = scene.build();
        let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture BVH builds");
        Self {
            geometry,
            bvh,
            primitives,
            receiver,
        }
    }

    /// Bake `light` over the receiver through the production per-light layer
    /// walk; absent texels had no record (no direct term).
    pub(super) fn bake(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        floor: bool,
    ) -> VisibilityGrid {
        let chart = self.receiver.chart();
        let charts = [chart];
        let placements = [ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let (atlas_width, atlas_height) = (charts[0].width_texels, charts[0].height_texels);
        let layout = BlockLayout::whole_layers(atlas_width, atlas_height, &placements, 2);
        let shared = SharedAtlas {
            charts: &charts,
            placements: &placements,
            atlas_width,
            atlas_height,
            layout: &layout,
        };
        // The emitter test the production stage runs once per light.
        let embedded = floor && self.has_embedded_emitter(light);
        let texels = bake_light_layer_chart_controlled(
            light,
            &shared,
            0,
            &self.bvh,
            &self.primitives,
            &self.geometry,
            area_sample_count,
            floor,
            embedded,
            &BakeControl::unrestricted(),
        );
        let mut grid = VisibilityGrid::new(self.receiver.width, self.receiver.height);
        for texel in texels {
            let x = texel.idx % atlas_width - CHART_PADDING_TEXELS;
            let y = texel.idx / atlas_width - CHART_PADDING_TEXELS;
            grid.set(x, y, texel.raw_visibility);
        }
        grid
    }

    /// [`super::embedded_emitter_fraction`] against this fixture.
    pub(super) fn embedded_emitter_fraction(&self, light: &MapLight) -> f32 {
        super::embedded_emitter_fraction(light, &self.bvh, &self.primitives, &self.geometry)
    }

    /// [`super::has_embedded_emitter`] against this fixture.
    pub(super) fn has_embedded_emitter(&self, light: &MapLight) -> bool {
        super::has_embedded_emitter(light, &self.bvh, &self.primitives, &self.geometry)
    }

    /// Bake `light` over the receiver as an animated light through the
    /// weight-map bake, one chunk spanning the receiver, and read each
    /// texel's visibility back from its stored weight over the unshadowed
    /// weight. A texel with no entry but a weight above the bake's epsilon
    /// is 0; one below it is absent, as the bake leaves it.
    pub(super) fn bake_weight_map(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        floor: bool,
    ) -> VisibilityGrid {
        use crate::animated_light_weight_maps::{
            WEIGHT_EPSILON, WeightMapInputs, bake_animated_light_weight_maps,
            contribution_to_weight,
        };
        use postretro_level_format::animated_light_chunks::{
            AnimatedLightChunk, AnimatedLightChunksSection,
        };

        let charts = [self.receiver.chart()];
        let chart = &charts[0];
        let placements = [ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        }];
        let (atlas_width, atlas_height) = (chart.width_texels, chart.height_texels);
        let layout = BlockLayout::whole_layers(atlas_width, atlas_height, &placements, 2);
        let chunks = AnimatedLightChunksSection {
            chunks: vec![AnimatedLightChunk {
                aabb_min: [0.0; 3],
                face_index: 0,
                aabb_max: [0.0; 3],
                index_offset: 0,
                uv_min: chart.uv_min,
                uv_max: [
                    chart.uv_min[0] + chart.uv_extent[0],
                    chart.uv_min[1] + chart.uv_extent[1],
                ],
                index_count: 1,
                _padding: 0,
            }],
            light_indices: vec![0],
        };
        let lights = [light.clone()];
        let section = bake_animated_light_weight_maps(&WeightMapInputs {
            bvh: &self.bvh,
            primitives: &self.primitives,
            geometry: &self.geometry,
            chunk_section: &chunks,
            lights: &lights,
            face_charts: &charts,
            face_placements: &placements,
            layout: &layout,
            atlas_width,
            atlas_height,
            static_atlas_layer_count: 1,
            area_sample_count,
            cut_parents: &[],
            penumbra_floor: floor,
            step_threshold: crate::texel_step_detector::TEXEL_STEP_THRESHOLD,
        })
        .expect("the receiver's chart fits its block")
        .section;
        let rect = section.chunk_rects[0];
        let mut grid = VisibilityGrid::new(self.receiver.width, self.receiver.height);
        for ty in 0..self.receiver.height {
            for tx in 0..self.receiver.width {
                let world_p = chart_texel_world_position(chart, tx as i32, ty as i32);
                let (contribution, _) = crate::lightmap_bake::light_contribution_and_direction(
                    light,
                    world_p,
                    chart.normal,
                );
                let unshadowed = contribution_to_weight(contribution, light.color, light.intensity);
                if unshadowed <= WEIGHT_EPSILON {
                    continue;
                }
                let x = tx + CHART_PADDING_TEXELS - rect.compact_x;
                let y = ty + CHART_PADDING_TEXELS - rect.compact_y;
                let entry =
                    section.offset_counts[(rect.texel_offset + y * rect.width + x) as usize];
                let visibility = match entry.count {
                    0 => 0.0,
                    _ => section.texel_lights[entry.offset as usize].weight / unshadowed,
                };
                grid.set(tx, ty, visibility);
            }
        }
        grid
    }

    /// The walk [`Fixture::bake`] runs for an unflagged light, replayed
    /// texel by texel through the footprint with each outcome kept: same
    /// order, same covered texels, same occluder caches, every clip segment
    /// traced (no pre-pass). `reach_override` replaces the wall clip and the
    /// lead-in alike.
    pub(super) fn footprint_outcomes(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        reach_override: Option<bool>,
    ) -> Vec<Option<FootprintOutcome>> {
        self.footprint_outcomes_with(
            light,
            area_sample_count,
            reach_override,
            ProbeEarlyOut::Agreed,
        )
    }

    /// [`Fixture::footprint_outcomes`] under the given early-out: a flagged
    /// light's walk takes [`ProbeEarlyOut::Blocked`].
    pub(super) fn footprint_outcomes_with(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        reach_override: Option<bool>,
        early_out: ProbeEarlyOut,
    ) -> Vec<Option<FootprintOutcome>> {
        self.replay(
            light,
            area_sample_count,
            reach_override,
            reach_override,
            early_out,
            |_, _| true,
        )
    }

    /// [`Fixture::footprint_outcomes`] with the wall clip traced and every
    /// lead-in taken as clear.
    pub(super) fn footprint_outcomes_without_lead_in(
        &self,
        light: &MapLight,
        area_sample_count: u32,
    ) -> Vec<Option<FootprintOutcome>> {
        self.replay(
            light,
            area_sample_count,
            None,
            Some(true),
            ProbeEarlyOut::Agreed,
            |_, _| true,
        )
    }

    /// The footprint's visibility at the texels `include` selects, every
    /// sample traced (no probe early-out): the reference an early-out bake
    /// must not snap to 0 or 1 where it is a fraction.
    pub(super) fn footprint_reference(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        include: impl Fn(u32, u32) -> bool,
    ) -> VisibilityGrid {
        let outcomes = self.replay(
            light,
            area_sample_count,
            None,
            None,
            ProbeEarlyOut::Off,
            include,
        );
        let mut grid = VisibilityGrid::new(self.receiver.width, self.receiver.height);
        for (i, outcome) in outcomes.iter().enumerate() {
            if let Some(outcome) = outcome {
                let i = i as u32;
                grid.set(
                    i % self.receiver.width,
                    i / self.receiver.width,
                    outcome.visibility,
                );
            }
        }
        grid
    }

    fn replay(
        &self,
        light: &MapLight,
        area_sample_count: u32,
        reach_override: Option<bool>,
        lead_in_override: Option<bool>,
        early_out: ProbeEarlyOut,
        include: impl Fn(u32, u32) -> bool,
    ) -> Vec<Option<FootprintOutcome>> {
        let chart = self.receiver.chart();
        let footprint = footprint_for(light, area_sample_count);
        let frame = TexelFrame::for_chart(&chart);
        let band = ClipBand::for_chart(&chart);
        let last_occluder = Cell::new(None);
        let clip_occluder = Cell::new(None);
        let mut outcomes = Vec::new();
        for ty in 0..self.receiver.height as i32 {
            for tx in 0..self.receiver.width as i32 {
                let world_p = chart_texel_world_position(&chart, tx, ty);
                if !include(tx as u32, ty as u32)
                    || !light_texel_is_covered(light, world_p, chart.normal)
                {
                    outcomes.push(None);
                    continue;
                }
                let site = TexelSite {
                    world_p,
                    normal: chart.normal,
                    seed: chart_texel_seed(&chart, tx, ty),
                    frame,
                    may_clip: true,
                };
                outcomes.push(Some(footprint_visibility(
                    light,
                    &site,
                    &footprint,
                    |from, to| self.clear_remembering(from, to, &last_occluder),
                    |from, to| {
                        reach_override
                            .unwrap_or_else(|| self.clear_remembering(from, to, &clip_occluder))
                    },
                    |from, to| {
                        lead_in_override.unwrap_or_else(|| {
                            lead_in_clear(
                                &self.bvh,
                                &self.primitives,
                                &self.geometry,
                                &band,
                                from,
                                to,
                            )
                        })
                    },
                    early_out,
                )));
            }
        }
        outcomes
    }

    fn clear_remembering(&self, from: Vec3, to: Vec3, cache: &Cell<Option<usize>>) -> bool {
        segment_clear_remembering(&self.bvh, &self.primitives, &self.geometry, from, to, cache)
    }

    /// Whether texel `(tx, ty)`'s single ray from its centre to `light`'s
    /// centre (its hard target) is blocked.
    pub(super) fn centre_ray_blocked(&self, light: &MapLight, tx: u32, ty: u32) -> bool {
        let chart = self.receiver.chart();
        let world_p = chart_texel_world_position(&chart, tx as i32, ty as i32);
        let target = crate::lightmap_bake::hard_ray_target(world_p, light);
        !segment_clear(
            &self.bvh,
            &self.primitives,
            &self.geometry,
            world_p + chart.normal * crate::lightmap_bake::RAY_EPSILON,
            target,
        )
    }
}

// --- analysis ---------------------------------------------------------------

/// The one-texel cone kernel's edge-spread function: the fraction of its
/// weight within signed distance `d` texels on the lit side of a straight
/// edge, so a texel centred `d` from a hard edge has exact filtered
/// visibility `cone_esf(d)`.
///
/// Integrates the kernel's exact line projection: with `a = √(1 − x²)`,
/// `∫ (1 − √(x² + y²)) dy` over the chord is `a − x²/2 · ln((1 + a)/(1 − a))`.
pub(super) fn cone_esf(d: f64) -> f64 {
    const STEPS: usize = 20_000;
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let projection = |x: f64| {
            let a = (1.0 - x * x).max(0.0).sqrt();
            if a <= 0.0 {
                0.0
            } else if x == 0.0 {
                1.0
            } else {
                a - 0.5 * x * x * ((1.0 + a) / (1.0 - a)).ln()
            }
        };
        // Cumulative trapezoid over [-1, 1] on a fine grid.
        let h = 2.0 / STEPS as f64;
        let mut cumulative = vec![0.0; STEPS + 1];
        for i in 1..=STEPS {
            let (x0, x1) = (-1.0 + (i - 1) as f64 * h, -1.0 + i as f64 * h);
            cumulative[i] = cumulative[i - 1] + 0.5 * h * (projection(x0) + projection(x1));
        }
        let total = cumulative[STEPS];
        assert!(
            (total - std::f64::consts::PI / 3.0).abs() < 1.0e-4,
            "the cone kernel integrates to π/3, got {total}"
        );
        cumulative.iter().map(|c| c / total).collect()
    });
    let t = ((d.clamp(-1.0, 1.0) + 1.0) / 2.0) * STEPS as f64;
    let i = (t.floor() as usize).min(STEPS - 1);
    let f = t - i as f64;
    table[i] * (1.0 - f) + table[i + 1] * f
}

/// Exact cone-kernel visibility at texel coordinates `centre` of a hard
/// shadow covering the convex polygons `shadow`, in texel coordinates: one
/// minus the share of the kernel's weight they cover, integrated on a polar
/// grid fine enough that it errs by well under 0.002.
pub(super) fn cone_visibility_of_shadow(centre: [f64; 2], shadow: &[Vec<[f64; 2]>]) -> f64 {
    const RINGS: usize = 96;
    const SPOKES: usize = 192;
    // Signed distance from `p` to a convex polygon's boundary, negative
    // inside, whichever way it winds.
    let signed_distance = |polygon: &[[f64; 2]], p: [f64; 2]| {
        let (mut nearest, mut inside, mut sign) = (f64::INFINITY, true, 0.0);
        for (i, &a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let (px, py) = (p[0] - a[0], p[1] - a[1]);
            let t = ((px * ex + py * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
            nearest = nearest.min((px - t * ex).hypot(py - t * ey));
            let cross = ex * py - ey * px;
            if cross != 0.0 {
                if sign == 0.0 {
                    sign = cross.signum();
                } else if cross.signum() != sign {
                    inside = false;
                }
            }
        }
        if inside { -nearest } else { nearest }
    };
    let inside = |polygon: &[[f64; 2]], p: [f64; 2]| {
        let mut sign = 0.0;
        for (i, &a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            if cross != 0.0 {
                if sign == 0.0 {
                    sign = cross.signum();
                } else if cross.signum() != sign {
                    return false;
                }
            }
        }
        true
    };
    let mut near = Vec::new();
    for polygon in shadow {
        let d = signed_distance(polygon, centre);
        if d <= -1.0 {
            return 0.0;
        }
        if d < 1.0 {
            near.push(polygon);
        }
    }
    if near.is_empty() {
        return 1.0;
    }
    let (mut covered, mut total) = (0.0, 0.0);
    for i in 0..RINGS {
        let r = (i as f64 + 0.5) / RINGS as f64;
        let weight = (1.0 - r) * r;
        for j in 0..SPOKES {
            let angle = (j as f64 + 0.5) / SPOKES as f64 * std::f64::consts::TAU;
            let p = [centre[0] + r * angle.cos(), centre[1] + r * angle.sin()];
            total += weight;
            if near.iter().any(|polygon| inside(polygon, p)) {
                covered += weight;
            }
        }
    }
    1.0 - covered / total
}

/// A straight shadow edge in receiver texel coordinates: a point on it, its
/// unit direction, and the unit normal pointing to the lit side.
#[derive(Debug, Clone, Copy)]
pub(super) struct Edge {
    pub(super) point: [f64; 2],
    pub(super) direction: [f64; 2],
    pub(super) lit_normal: [f64; 2],
}

impl Edge {
    /// Signed distance of texel coordinates `p` from the edge, positive on the
    /// lit side.
    pub(super) fn signed_distance(&self, p: [f64; 2]) -> f64 {
        (p[0] - self.point[0]) * self.lit_normal[0] + (p[1] - self.point[1]) * self.lit_normal[1]
    }

    fn at(&self, along: f64, across: f64) -> [f64; 2] {
        [
            self.point[0] + self.direction[0] * along + self.lit_normal[0] * across,
            self.point[1] + self.direction[1] * along + self.lit_normal[1] * across,
        ]
    }
}

/// Exact cone-kernel visibility at every texel of `receiver` for a hard
/// half-plane shadow bounded by `edge`.
pub(super) fn exact_edge_grid(receiver: &Receiver, edge: &Edge) -> VisibilityGrid {
    let mut grid = VisibilityGrid::new(receiver.width, receiver.height);
    for y in 0..receiver.height {
        for x in 0..receiver.width {
            let d = edge.signed_distance([f64::from(x) + 0.5, f64::from(y) + 0.5]);
            grid.set(x, y, cone_esf(d) as f32);
        }
    }
    grid
}

/// The texels bilinear reconstruction blends at texel coordinates `p`, with
/// their weights; `None` when one falls outside the grid or has no record.
pub(super) fn bilinear_taps(grid: &VisibilityGrid, p: [f64; 2]) -> Option<[(f32, f64); 4]> {
    let (fx, fy) = (p[0] - 0.5, p[1] - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (wx, wy) = (fx - x0, fy - y0);
    if x0 < 0.0 || y0 < 0.0 {
        return None;
    }
    let (x0, y0) = (x0 as u32, y0 as u32);
    if x0 + 1 >= grid.width() || y0 + 1 >= grid.height() {
        return None;
    }
    Some([
        (grid.get(x0, y0)?, (1.0 - wx) * (1.0 - wy)),
        (grid.get(x0 + 1, y0)?, wx * (1.0 - wy)),
        (grid.get(x0, y0 + 1)?, (1.0 - wx) * wy),
        (grid.get(x0 + 1, y0 + 1)?, wx * wy),
    ])
}

pub(super) fn bilinear(grid: &VisibilityGrid, p: [f64; 2]) -> Option<f64> {
    bilinear_taps(grid, p).map(|taps| taps.iter().map(|&(v, w)| f64::from(v) * w).sum())
}

/// Mean 10–90% ramp width, in texels, of bilinear reconstruction across
/// `edge`: on each profile perpendicular to the edge (one every quarter
/// texel along `along`), the length over which reconstruction lies strictly
/// between 0.1 and 0.9 within `reach` texels of the edge. Profiles that leave
/// the grid or touch an absent texel are skipped. Returns the mean and the
/// profile count.
pub(super) fn mean_ramp_width(
    grid: &VisibilityGrid,
    edge: &Edge,
    along: (f64, f64),
    reach: f64,
) -> (f64, usize) {
    const STEP: f64 = 0.01;
    let mut total = 0.0;
    let mut profiles = 0;
    let mut s = along.0;
    'profile: while s <= along.1 {
        let mut inside = 0usize;
        let samples = (2.0 * reach / STEP).round() as i64;
        for k in 0..=samples {
            let across = -reach + k as f64 * STEP;
            let Some(v) = bilinear(grid, edge.at(s, across)) else {
                s += 0.25;
                continue 'profile;
            };
            if v > 0.1 && v < 0.9 {
                inside += 1;
            }
        }
        total += inside as f64 * STEP;
        profiles += 1;
        s += 0.25;
    }
    (total / profiles.max(1) as f64, profiles)
}

/// Every axis-adjacent present pair that is exactly `0.0` beside exactly
/// `1.0`: a single-texel 0→1 jump.
pub(super) fn zero_one_jumps(grid: &VisibilityGrid) -> usize {
    let mut jumps = 0;
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let here = grid.get(x, y);
            for there in [
                (x + 1 < grid.width()).then(|| grid.get(x + 1, y)).flatten(),
                (y + 1 < grid.height())
                    .then(|| grid.get(x, y + 1))
                    .flatten(),
            ] {
                if let (Some(a), Some(b)) = (here, there)
                    && ((a == 0.0 && b == 1.0) || (a == 1.0 && b == 0.0))
                {
                    jumps += 1;
                }
            }
        }
    }
    jumps
}
