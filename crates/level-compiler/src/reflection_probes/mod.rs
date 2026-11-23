//! Reflection-probe capture stage (PRL id 54): place authored and derived
//! probes over up-facing fluid faces and, behind `--glossy-reflections`,
//! glossy world faces of every facing; capture each probe's static world
//! into an octahedral colour tile and a distance tile.
//! See: context/lib/resource_management.md §5 · build_pipeline.md §Compiler pipeline, §Build Cache

mod animated_direct;
mod authored;
mod box_fit;
mod capture;
mod capture_bounds;
mod capture_brushes;
mod capture_exclusion;
mod capture_fill;
mod capture_point;
mod capture_visibility;
mod delta_sample;
mod derived;
mod emissive_factor;
mod exclusion;
mod exclusion_underside;
mod facing;
mod fit_frame;
mod front_side;
mod glossy_census;
mod glossy_materials;
mod glossy_placement;
mod hit;
mod lightmap_sample;
pub(crate) mod placement;
mod plane_basis;
mod probe_capture;
mod probe_log;
mod reference;
mod roster;
mod sdf_lights;
mod self_transparency;
mod sh_sample;
mod shading;
mod share;
mod stage_key;
mod world_reflectors;

#[cfg(test)]
mod area_glow_capture_tests;
#[cfg(test)]
mod box_fit_tests;
#[cfg(test)]
mod capture_exclusion_held_tests;
#[cfg(test)]
mod capture_exclusion_oracle_tests;
#[cfg(test)]
mod capture_exclusion_test_fixtures;
#[cfg(test)]
mod capture_exclusion_tests;
#[cfg(test)]
mod capture_fill_tests;
#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod distance_tests;
#[cfg(test)]
mod exclusion_stage_tests;
#[cfg(test)]
mod exclusion_tests;
#[cfg(test)]
mod glossy_census_prl_tests;
#[cfg(test)]
mod glossy_census_tests;
#[cfg(test)]
mod glossy_facing_tests;
#[cfg(test)]
mod glossy_placement_tests;
#[cfg(test)]
pub(crate) mod placement_test_fixtures;
#[cfg(test)]
mod placement_tests;
#[cfg(test)]
mod reflection_probes_test_fixtures;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use bvh::bvh::Bvh;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::area_glow::AreaGlowSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::emissive::{EmissiveTexture, EmissiveVolumeRecord};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::probe_depth_tiles::ProbeDepthTilesSection;
use postretro_level_format::reflection_probes::{MAX_REFLECTION_PROBES, ReflectionProbesSection};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::StageCache;
use crate::geometry::GeometryResult;
use crate::map_data::{BrushVolume, MapReflectionVolume, ReflectionCapture};
use crate::partition::BspTree;

use crate::surface_materials::SurfaceMaterials;
use animated_direct::AnimatedDirectSampler;
use box_fit::WorldBounds;
use capture_brushes::{BrushFaces, CaptureBrushes};
use capture_exclusion::ProbeExclusion;
use capture_point::PointClassifier;
use capture_visibility::warn_hidden_reflectors;
use delta_sample::DeltaShSampler;
use emissive_factor::{AreaGlowFaces, EmissiveFaceFactors};
use exclusion::{ExclusionConstants, ExclusionScene, classify_probes_within_boxes};
pub(crate) use hit::SceneTracer;
use lightmap_sample::LightmapSampler;
#[cfg(test)]
use placement::fluid_bindings;
use placement::{PlacedProbe, PlacementScene, place_probes, up_facing_faces};
use probe_capture::capture_probes;
#[cfg(test)]
use probe_capture::capture_section;
use probe_log::log_probes;
use roster::{check_roster, references};
use sdf_lights::SdfLights;
use self_transparency::transparent_faces;
use sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use shading::CaptureScene;
use share::log_negative_references;
use stage_key::{cache_key, cache_payload, parse_cache_payload};

/// Cache stage identity for the whole id-54 section.
const STAGE_ID: &str = "reflection_probes";
/// Bump when capture shading, sampling or the payload changes. Placement
/// enters the key as its output, so a placement change re-keys by itself.
/// 3: animated direct and indirect at reference, sdf lights, light share.
/// 4: the distance tile (mean inverse L1 hit distance per texel).
/// 5, 6: unused; never reuse them, as their entries may sit in local caches.
/// 7: a finite hit never stores distance 0.
/// 8: shares over the upper hemisphere; derived box fit stops at the ceiling;
/// ties within tolerance; SH clamped after the deltas, as the runtime does;
/// the emissive mip from its own texel footprint; a light with a negative
/// reference never stored.
/// 9: a wall past a column's first ceiling hit still tops it when farther
/// out (a sunken pool under an overhanging lip fits the room); the key
/// gains id 45's descriptor table.
/// 10: unreleased build of this feature; never reuse it, as its entries may
/// sit in local caches.
/// 11: capture exclusion: primary rays skip each probe's excluded brushes,
/// with fill and leak guard; the key gains the override table, the rule's
/// constants, each probe's excluded set with its hulls and contacts, and
/// each kept contact candidate's faces, planes and sides.
/// 12: an excluded hull's exit reads the neighbour the ray is inside first,
/// kept solid winning a tie, not the first excluded one in brush order.
/// 13: kept solid passed over at an exit for a nearer excluded neighbour
/// bounds every later read: past its depth the ray takes the leak guard.
/// 14: a probe with a depth tile (id 59) weighs by the tile's moments toward
/// the sample, as the runtime does; the key gains id 59.
/// 15: emissive strength is the baked id-60 value (`materials.toml`, else 1
/// with an `_e` map), not the texture-name prefix, times a member face's
/// emissive volume `max × colour`; the key gains each volume's factor and
/// faces.
/// 16: id 54 version 3: six lobes (−Y appended, zero for water), a colour
/// mip count (1), and a world binding list. Glossy placement added no
/// version: the key gained the gate, each probe's world faces and its
/// transparent faces, so no earlier entry can hit.
const STAGE_VERSION: u32 = 16;

/// What the stage reads. Everything here enters its cache key.
pub(crate) struct ReflectionProbeInputs<'a> {
    /// The static world: movers and environment volumes are not in it.
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    /// Id 52, whose up-facing fluid faces bind the probes and whose fluid
    /// brushes drop a capture point inside them.
    pub environment_volumes: &'a EnvironmentVolumesSection,
    /// Authored boxes, in map entity order.
    pub reflection_volumes: &'a [MapReflectionVolume],
    /// The static world's brush hulls (`MapData::brush_volumes`) and each
    /// post-cut face's brush, index-aligned with `geometry`'s faces. They
    /// reach the capture only through a probe's excluded set, so they key it
    /// through that set.
    pub brush_volumes: &'a [BrushVolume],
    pub face_brushes: &'a [Option<u32>],
    /// Each brush's resolved capture override, index-aligned with
    /// `brush_volumes` (`MapData::brush_capture_override`).
    pub capture_override: &'a [ReflectionCapture],
    /// The compiler BSP and its exterior leaves: a capture point in a solid
    /// leaf is in solid, one in an exterior leaf is outside the hull.
    pub tree: &'a BspTree,
    pub exterior_leaves: &'a HashSet<usize>,
    /// Id 22 as encoded: the capture decodes the texels it reads.
    pub lightmap: &'a LightmapSection,
    /// The packed pre-BC6H base SH volume id 34 encodes. Warm SH differs from
    /// cold, so the key hashes this output, not the SH bake's inputs. Its
    /// animation descriptors are the runtime's, by roster index.
    pub sh_volume: &'a OctahedralShVolumeSection,
    /// Id 59 as the level ships it: the probes that weigh by per-direction
    /// depth moments, and their tiles.
    pub depth_tiles: &'a ProbeDepthTilesSection,
    /// Id 27 as the level ships it: each animated light's bounce.
    pub delta_sh: Option<&'a DeltaShVolumesSection>,
    /// Id 25 as the level ships it: each animated light's direct.
    pub animated_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    /// Id 45 as the level ships it. The capture reads none of it; the roster
    /// check refuses a descriptor table that is not the identity, as the
    /// loader does, so only that table enters the key.
    pub animated_direct_sh: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    /// Id 18: the runtime's lights, whose non-dynamic records are the spec
    /// lights the forward sdf loop shades.
    pub alpha_lights: &'a AlphaLightsSection,
    /// Id 23: the chunk windows the sdf selection reads.
    pub chunk_light_list: &'a ChunkLightListSection,
    /// Texture name → `.prm` key, from the TextureMips stage.
    pub texture_keys: &'a HashMap<String, [u8; 32]>,
    /// Id 60's per-texture records, index-aligned with the texture names:
    /// the strength the runtime material uniform also packs.
    pub emissive_textures: &'a [EmissiveTexture],
    /// Id 60's volume records: a member face shades at its volume's
    /// no-script reference state, `max × colour`.
    pub emissive_volumes: &'a [EmissiveVolumeRecord],
    /// Id 62, when the map has an area light: each area face shades at its
    /// light's glow, `_glow` keying this stage and no lighting bake.
    pub area_glow: Option<&'a AreaGlowSection>,
    pub prm_root: &'a Path,
    /// `prl-build --glossy-reflections`: glossy world faces take probes,
    /// derived or authored. Off, every map bakes as it did before glossy
    /// placement, its glossy materials included.
    pub glossy_reflections: bool,
}

/// The bindings placement gives `environment` over a compiled static world,
/// as the stage places them: for tests comparing two face sets of one level.
#[cfg(test)]
pub(crate) fn placed_bindings(
    environment: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    world: &crate::fixture_pipeline::FixturePipeline,
) -> Vec<postretro_level_format::reflection_probes::ReflectionProbeFluidBinding> {
    let bounds =
        WorldBounds::from_positions(world.geometry.geometry.vertices.iter().map(|v| &v.position))
            .expect("the level has static world");
    let tracer = SceneTracer::new(&world.bvh, &world.primitives, &world.geometry);
    let classifier =
        PointClassifier::new(&world.full_tree, &world.full_exterior_leaves, environment);
    let control = BakeControl::unrestricted();
    let probes = place_probes(
        environment,
        authored,
        &[],
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        MAX_REFLECTION_PROBES,
    )
    .expect("placement keeps within the cap");
    fluid_bindings(&probes)
}

/// Id 54 for the level, from the cache when its key hits; `None` when no
/// probe is kept, so the section is omitted.
pub(crate) fn bake_or_load_reflection_probes(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    bake_or_load_with(
        inputs,
        stage_cache,
        control,
        &ExclusionConstants::PRODUCTION,
        classify_probes_within_boxes,
        Reflectors::SelfTransparent,
    )
}

/// The stage with the rule under `constants`, for tests perturbing them.
#[cfg(test)]
pub(crate) fn bake_or_load_under(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
    constants: &ExclusionConstants,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    bake_or_load_with(
        inputs,
        stage_cache,
        control,
        constants,
        classify_probes_within_boxes,
        Reflectors::SelfTransparent,
    )
}

/// The stage with every probe's excluded set forced empty: the unfiltered
/// capture, for tests comparing against it.
#[cfg(test)]
pub(crate) fn bake_or_load_unfiltered(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    bake_or_load_with(
        inputs,
        stage_cache,
        control,
        &ExclusionConstants::PRODUCTION,
        |_, probes, _| vec![Vec::new(); probes.len()],
        Reflectors::SelfTransparent,
    )
}

/// The stage with glossy probes' primary rays under `reflectors`: tests
/// compare the self-transparent capture with the self-occluded ones it
/// exists to avoid.
#[cfg(test)]
pub(crate) fn bake_or_load_with_reflectors(
    inputs: &ReflectionProbeInputs<'_>,
    reflectors: Reflectors,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    bake_or_load_with(
        inputs,
        None,
        &BakeControl::unrestricted(),
        &ExclusionConstants::PRODUCTION,
        classify_probes_within_boxes,
        reflectors,
    )
}

/// What a glossy probe's primary rays pass through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reflectors {
    /// Every face in its reflector plane, bound or not: production.
    SelfTransparent,
    /// Nothing: its own floor is in its capture.
    #[cfg(test)]
    Opaque,
    /// Its bound faces alone: a coplanar matte face stays in its capture.
    #[cfg(test)]
    BoundOnly,
}

/// The stage, each placed probe's excluded set from `classify` under
/// `constants`, the instance the key hashes.
fn bake_or_load_with(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
    constants: &ExclusionConstants,
    classify: impl FnOnce(&ExclusionScene<'_>, &[PlacedProbe], &ExclusionConstants) -> Vec<Vec<u32>>,
    reflectors: Reflectors,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    // The census line reports on every build, gate on or off.
    let world_faces = glossy_placement::glossy_world_faces(inputs, control.governor());
    if inputs.reflection_volumes.is_empty()
        && up_facing_faces(inputs.environment_volumes).is_empty()
        && world_faces.is_empty()
    {
        return Ok(None);
    }
    let Some(bounds) = WorldBounds::from_positions(
        inputs
            .geometry
            .geometry
            .vertices
            .iter()
            .map(|v| &v.position),
    ) else {
        log::warn!("[ReflectionProbes] the level has no static world to reflect; no probes");
        return Ok(None);
    };
    let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
    // Placement runs on every build, warm included, so its drop warnings
    // always reach the mapper; its output keys the capture.
    let classifier = PointClassifier::new(
        inputs.tree,
        inputs.exterior_leaves,
        inputs.environment_volumes,
    );
    let probes = place_probes(
        inputs.environment_volumes,
        inputs.reflection_volumes,
        &world_faces,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        MAX_REFLECTION_PROBES,
    )?;
    if probes.is_empty() {
        return Ok(None);
    }

    // References are a cheap function of the keyed descriptors; computed
    // before the lookup so a warm build logs the lights no probe stores, as
    // a cold one does.
    let references = references(&inputs.sh_volume.animation_descriptors);
    log_negative_references(&references);

    // Classification follows placement and the cap, which never see it, and
    // runs on every build: each probe's excluded set, its hulls and their
    // contacts key the capture.
    anyhow::ensure!(
        inputs.capture_override.len() == inputs.brush_volumes.len()
            && inputs.face_brushes.len() == inputs.geometry.face_index_ranges.len(),
        "{} capture override(s) for {} brush(es), {} face brush(es) for {} face(s)",
        inputs.capture_override.len(),
        inputs.brush_volumes.len(),
        inputs.face_brushes.len(),
        inputs.geometry.face_index_ranges.len(),
    );
    let excluded = classify(
        &ExclusionScene {
            brush_volumes: inputs.brush_volumes,
            capture_override: inputs.capture_override,
            face_brush: inputs.face_brushes,
            geometry: inputs.geometry,
            environment_volumes: inputs.environment_volumes,
        },
        &probes,
        constants,
    );
    anyhow::ensure!(
        excluded.len() == probes.len(),
        "{} excluded set(s) for {} probe(s)",
        excluded.len(),
        probes.len()
    );
    let brush_faces = BrushFaces::new(inputs.face_brushes, inputs.brush_volumes.len());
    let brushes = CaptureBrushes {
        volumes: inputs.brush_volumes,
        face_brushes: inputs.face_brushes,
        faces: &brush_faces,
    };
    let transparent = match reflectors {
        Reflectors::SelfTransparent => transparent_faces(inputs.geometry, &probes, &world_faces),
        #[cfg(test)]
        Reflectors::Opaque => vec![Vec::new(); probes.len()],
        #[cfg(test)]
        Reflectors::BoundOnly => probes.iter().map(|p| p.world_faces.clone()).collect(),
    };
    let exclusions: Vec<ProbeExclusion<'_>> = probes
        .iter()
        .zip(&excluded)
        .zip(transparent)
        .map(|((probe, set), faces)| {
            ProbeExclusion::new(&tracer, brushes, set, probe).with_transparent_faces(&tracer, faces)
        })
        .collect();
    for (index, exclusion) in exclusions.iter().enumerate() {
        log::info!(
            "[ReflectionProbes] probe {index}: {} brush(es) excluded from its capture",
            exclusion.excluded().len()
        );
    }

    // Capture is a pure function of the keyed inputs, so a hit skips it.
    let key = cache_key(
        inputs,
        &probes,
        &exclusions,
        constants,
        crate::ray_traversal::TRACER_EPOCH,
    )?;
    if let Some((section, reports)) =
        stage_cache
            .and_then(|cache| cache.get(&key))
            .and_then(|bytes| {
                parse_cache_payload(&bytes)
                    .map_err(|e| {
                        log::warn!("[cache] corrupt reflection_probes entry, re-baking: {e}")
                    })
                    .ok()
            })
    {
        log::info!("[cache] reflection_probes hit");
        log_probes(&section, &reports);
        warn_hidden_reflectors(&section, &probes, inputs.environment_volumes, &world_faces);
        return Ok(Some(section));
    }
    log::info!("[cache] reflection_probes miss");

    check_roster(inputs, references.len())?;
    let sh = ShVolumeSampler::new(inputs.sh_volume, PRODUCTION_PROBE_OCCLUSION)?
        .with_depth_tiles(inputs.depth_tiles)?;
    let deltas = inputs
        .delta_sh
        .map(|delta| {
            DeltaShSampler::new(
                delta,
                &sh,
                inputs.sh_volume.tile_dimension,
                inputs.sh_volume.grid_dimensions,
                &references,
            )
        })
        .transpose()?;
    let scene = CaptureScene {
        tracer,
        brushes,
        materials: SurfaceMaterials::new(
            &inputs.geometry.texture_names.names,
            inputs.texture_keys,
            inputs.emissive_textures,
            inputs.prm_root,
        ),
        emissive: EmissiveFaceFactors::new(
            inputs.emissive_volumes,
            inputs.geometry.geometry.faces.len(),
        ),
        area_glow: AreaGlowFaces::new(
            inputs.area_glow,
            &inputs.sh_volume.animation_descriptors,
            inputs.geometry.geometry.faces.len(),
        ),
        lightmap: LightmapSampler::new(inputs.lightmap),
        sh,
        animated_direct: inputs
            .animated_weight_maps
            .map(|maps| AnimatedDirectSampler::new(maps, inputs.lightmap)),
        deltas,
        sdf: SdfLights::new(inputs.alpha_lights, inputs.chunk_light_list),
        max_distance: bounds.diagonal() as f32 + 1.0,
    };
    let (section, reports) = capture_probes(&scene, &probes, &exclusions, &references, control)?;
    if let Some(cache) = stage_cache {
        cache.put(&key, &cache_payload(&section, &reports));
    }
    log_probes(&section, &reports);
    warn_hidden_reflectors(&section, &probes, inputs.environment_volumes, &world_faces);
    Ok(Some(section))
}
