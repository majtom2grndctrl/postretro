// Test scene for the SH base stage: a lit room, an annex, and bakes through the real entry points.
// See: context/lib/build_pipeline.md §Build Cache

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use bvh::bvh::Bvh;
use glam::{DVec3, Vec3};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::DEFAULT_BOUNCE_FILL;
use super::layer::{LayerKeys, LightDigest};
use super::layer_set::{LayerPlan, resolve_layers};
use super::patches::PatchSet;
use super::reach::PatchReachIndex;
use super::tests::polygon_geometry;
use super::volume::volume_key;
use crate::bake_control::BakeControl;
use crate::bake_counters::{BakeCounts, counted};
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::cache::{CacheKey, StageCache};
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::partition::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
use crate::sh_bake::{ShBakeCtx, ShConfig, probe_grid_layout, static_light_refs};
use crate::surface_materials::stats::BounceMaterials;
use crate::texel_footprint::{EmbeddedEmitters, NO_EMBEDDED_EMITTERS};

/// A quad wound `a, b, c, d`, its front along `(b - a) × (c - a)`.
fn quad(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> Vec<Vec3> {
    [a, b, c, d].into_iter().map(Vec3::from).collect()
}

/// An 8 × 3 × 6 m room (x 0..8, y 0..3, z 0..6) facing inward, a plate
/// hovering over its floor, and a 2 m annex floor at x 13..15 that no room
/// light's range reaches.
fn room_geometry() -> GeometryResult {
    let (x, y, z) = (8.0, 3.0, 6.0);
    polygon_geometry(&[
        quad([0.0, 0.0, 0.0], [0.0, 0.0, z], [x, 0.0, z], [x, 0.0, 0.0]),
        quad([0.0, y, 0.0], [x, y, 0.0], [x, y, z], [0.0, y, z]),
        quad([0.0, 0.0, 0.0], [0.0, y, 0.0], [0.0, y, z], [0.0, 0.0, z]),
        quad([x, 0.0, 0.0], [x, 0.0, z], [x, y, z], [x, y, 0.0]),
        quad([0.0, 0.0, 0.0], [x, 0.0, 0.0], [x, y, 0.0], [0.0, y, 0.0]),
        quad([0.0, 0.0, z], [0.0, y, z], [x, y, z], [x, 0.0, z]),
        // The plate, facing the floor.
        quad(
            [3.0, 1.2, 2.0],
            [5.0, 1.2, 2.0],
            [5.0, 1.2, 4.0],
            [3.0, 1.2, 4.0],
        ),
        // The annex floor.
        quad(
            [13.0, 0.0, 0.0],
            [13.0, 0.0, 2.0],
            [15.0, 0.0, 2.0],
            [15.0, 0.0, 0.0],
        ),
    ])
}

pub(super) fn point(origin: [f64; 3], light_size: f32, range: f32) -> MapLight {
    MapLight {
        origin: DVec3::from(origin),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: range,
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
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// Three room lights, one tinted and one hard, then the annex's light.
pub(super) fn room_lights() -> Vec<MapLight> {
    let mut tinted = point([6.0, 2.2, 4.0], 0.3, 5.0);
    tinted.color = [1.0, 0.4, 0.2];
    vec![
        point([2.0, 2.2, 2.0], 0.2, 5.0),
        tinted,
        point([4.0, 2.5, 3.0], 0.0, 4.0),
        annex_light(),
    ]
}

/// The light whose reach holds the annex alone.
pub(super) fn annex_light() -> MapLight {
    point([14.0, 1.0, 1.0], 0.1, 2.0)
}

/// What a static light flipped to animated by `_animated 1` becomes.
pub(super) fn animated(mut light: MapLight) -> MapLight {
    light.is_animated = true;
    light.animation = Some(crate::map_data::animated_light_placeholder(true));
    light
}

pub(super) struct Scene {
    pub(super) geometry: GeometryResult,
    pub(super) bvh: Bvh<f32, 3>,
    pub(super) primitives: Vec<BvhPrimitive>,
    tree: BspTree,
    exterior: HashSet<usize>,
    /// `None` bounces every face grey, as a map without textures does.
    materials: Option<BounceMaterials>,
}

/// Cache entry files by name.
pub(super) type EntryBytes = BTreeMap<String, Vec<u8>>;

impl Scene {
    pub(super) fn new() -> Self {
        Self::of(room_geometry())
    }

    /// A scene over `geometry`, one open leaf holding every probe.
    pub(super) fn of(geometry: GeometryResult) -> Self {
        let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
        let tree = BspTree {
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
        };
        Self {
            geometry,
            bvh,
            primitives,
            tree,
            exterior: HashSet::new(),
            materials: None,
        }
    }

    /// This scene with the half-space `x ≥ x_min` solid: its probes turn
    /// invalid while every face, and so the geometry, stays as it was, as a
    /// brush edit that moves only leaf solidity would leave it.
    pub(super) fn with_solid_beyond(mut self, x_min: f64) -> Self {
        let open = self.tree.leaves[0].clone();
        let solid = BspLeaf {
            is_solid: true,
            ..open.clone()
        };
        self.tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: x_min,
                front: BspChild::Leaf(1),
                back: BspChild::Leaf(0),
                parent: None,
            }],
            leaves: vec![open, solid],
        };
        self
    }

    /// This scene with every face's bounce albedo read from `materials`.
    pub(super) fn with_materials(mut self, materials: BounceMaterials) -> Self {
        self.materials = Some(materials);
        self
    }

    /// Run `f` over the SH bake context of `lights`.
    pub(super) fn with_ctx<R>(
        &self,
        lights: &[MapLight],
        f: impl FnOnce(&ShBakeCtx<'_>) -> R,
    ) -> R {
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let ctx = ShBakeCtx {
            bounce_materials: self.materials.as_ref(),
            bvh: &self.bvh,
            primitives: &self.primitives,
            geometry: &self.geometry,
            tree: &self.tree,
            exterior_leaves: &self.exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: lights.len(),
        };
        f(&ctx)
    }

    /// The production SH base bake (the gate's patch set, then
    /// [`super::bake_sh_base`]) at `spacing` and `fill`, under `control`.
    pub(super) fn bake_with(
        &self,
        lights: &[MapLight],
        spacing: f32,
        fill: f32,
        embedded: &EmbeddedEmitters,
        cache: Option<&StageCache>,
        control: &BakeControl,
    ) -> OctahedralShVolumeSection {
        self.with_ctx(lights, |ctx| {
            let refs = static_light_refs(ctx);
            let (handoff, _) = super::plan_sh_base(&self.geometry, &refs, spacing, fill, u64::MAX)
                .expect("priced");
            super::bake_sh_base(
                ctx,
                &ShConfig {
                    probe_spacing: spacing,
                },
                handoff,
                embedded,
                cache,
                control,
            )
        })
    }

    /// [`Self::bake_with`] at 1 m spacing and the default fill with no
    /// flagged light, on a pool of `threads` workers, and what it spent.
    pub(super) fn bake(
        &self,
        lights: &[MapLight],
        cache: Option<&StageCache>,
        threads: usize,
    ) -> (OctahedralShVolumeSection, BakeCounts) {
        self.bake_at(lights, DEFAULT_BOUNCE_FILL, cache, threads)
    }

    /// [`Self::bake`] at `fill`.
    pub(super) fn bake_at(
        &self,
        lights: &[MapLight],
        fill: f32,
        cache: Option<&StageCache>,
        threads: usize,
    ) -> (OctahedralShVolumeSection, BakeCounts) {
        counted(threads, || {
            self.bake_with(
                lights,
                1.0,
                fill,
                &NO_EMBEDDED_EMITTERS,
                cache,
                &BakeControl::unrestricted(),
            )
        })
    }

    /// The static lights' reach counts, as the gate prices them.
    pub(super) fn reach(&self, lights: &[MapLight]) -> Vec<u64> {
        let patches = PatchSet::build(&self.geometry);
        self.with_ctx(lights, |ctx| {
            PatchReachIndex::new(patches.patches()).counts_in_reach(&static_light_refs(ctx))
        })
    }

    /// Each layer a cold bake of `lights` shades, serialized, beside the
    /// key of the cache entry that holds it.
    pub(super) fn cold_layers(
        &self,
        lights: &[MapLight],
        embedded: &EmbeddedEmitters,
    ) -> Vec<(CacheKey, Vec<u8>)> {
        let patches = PatchSet::build(&self.geometry);
        self.with_ctx(lights, |ctx| {
            let refs = static_light_refs(ctx);
            let reach = PatchReachIndex::new(patches.patches()).counts_in_reach(&refs);
            let plan = LayerPlan::new(&refs, &reach, embedded);
            let layers = resolve_layers(
                &ctx.ray_ctx(),
                &patches,
                &refs,
                &plan,
                None,
                &BakeControl::unrestricted(),
            );
            let keys = self.keys();
            plan.layers
                .iter()
                .zip(layers)
                .map(|(planned, layer)| {
                    (
                        keys.key(planned.digest, planned.embedded),
                        layer.to_bytes(patches.len()),
                    )
                })
                .collect()
        })
    }

    pub(super) fn keys(&self) -> LayerKeys {
        LayerKeys::new(&self.geometry, self.materials.as_ref())
    }

    /// The cache key of `light`'s layer, unflagged.
    pub(super) fn key(&self, light: &MapLight) -> CacheKey {
        self.keys().key(LightDigest::of(light), false)
    }

    /// The key of the gathered volume a bake of `lights` at `spacing` and
    /// `fill`, with no flagged light, reads and writes.
    pub(super) fn volume_key_at(&self, lights: &[MapLight], spacing: f32, fill: f32) -> CacheKey {
        let reach = self.reach(lights);
        self.with_ctx(lights, |ctx| {
            let plan = LayerPlan::new(&static_light_refs(ctx), &reach, &NO_EMBEDDED_EMITTERS);
            let layout = probe_grid_layout(
                ctx,
                &ShConfig {
                    probe_spacing: spacing,
                },
            );
            volume_key(&self.keys(), &plan, &layout, spacing, fill)
        })
    }

    /// [`Self::volume_key_at`] for [`Self::bake`].
    pub(super) fn volume_key(&self, lights: &[MapLight]) -> CacheKey {
        self.volume_key_at(lights, 1.0, DEFAULT_BOUNCE_FILL)
    }

    /// Delete the gathered-volume entry a [`Self::bake`] of `lights` wrote
    /// to `dir`, so the next warm bake of them reads its layers.
    pub(super) fn forget_volume(&self, dir: &Path, lights: &[MapLight]) {
        std::fs::remove_file(dir.join(self.volume_key(lights).as_filename()))
            .expect("a warm bake wrote the volume entry");
    }
}

/// Every cache entry in `dir`, by file name.
pub(super) fn entries(dir: &Path) -> EntryBytes {
    std::fs::read_dir(dir)
        .expect("cache dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).expect("entry bytes"),
            )
        })
        .collect()
}
