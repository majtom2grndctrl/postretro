//! Reflection-probe capture stage (PRL id 54): place authored and derived
//! probes over up-facing fluid faces, capture each probe's static world into
//! an octahedral colour tile and a distance tile, split probes whose capture
//! point fails their water, then cap.
//! See: context/lib/resource_management.md §5 · build_pipeline.md §Compiler pipeline, §Build Cache

mod animated_direct;
mod authored;
mod box_fit;
mod cap;
mod capture;
mod capture_batch;
mod capture_point;
mod capture_visibility;
mod delta_sample;
mod derived;
mod hit;
mod lightmap_sample;
mod materials;
mod placement;
mod reference;
mod sdf_lights;
mod sh_sample;
mod shading;
mod share;
mod split;
mod split_triggers;
mod stage_key;

#[cfg(test)]
mod capture_tests;
#[cfg(test)]
mod distance_tests;
#[cfg(test)]
mod placement_test_fixtures;
#[cfg(test)]
mod placement_tests;
#[cfg(test)]
mod reflection_probes_test_fixtures;
#[cfg(test)]
mod split_tests;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;

use bvh::bvh::Bvh;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::reflection_probes::{
    MAX_REFLECTION_PROBES, REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_TILE_BORDER,
    REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbesSection,
};
use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::cache::StageCache;
use crate::geometry::GeometryResult;
use crate::map_data::MapReflectionVolume;
use crate::partition::BspTree;

use animated_direct::AnimatedDirectSampler;
use box_fit::WorldBounds;
use cap::{apply_probe_cap, warn_capped};
use capture_batch::{assemble_section, capture_probes};
use capture_point::PointClassifier;
use capture_visibility::{DistanceTile, warn_hidden_water};
use delta_sample::DeltaShSampler;
use hit::SceneTracer;
use lightmap_sample::LightmapSampler;
use materials::SurfaceMaterials;
use placement::{PlacementScene, place_probes, up_facing_faces};
use reference::reference_value;
use sdf_lights::SdfLights;
use sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use shading::CaptureScene;
use share::ShareReport;
use split::{MAX_SPLIT_ROUNDS, SplitScene, StageProbe, split_round};
use stage_key::{cache_key, cache_payload, parse_cache_payload};

/// Cache stage identity for the whole id-54 section.
const STAGE_ID: &str = "reflection_probes";
/// Bump when capture shading, sampling or the payload changes. Placement
/// enters the key as its output, so a placement change re-keys by itself.
/// 3: animated direct and indirect at reference, sdf lights, light share.
/// 4: the distance tile (mean inverse L1 hit distance per texel).
/// 5: splitting by sight and hidden mirror hits, then the cap; a finite hit
/// never stores distance 0.
const STAGE_VERSION: u32 = 5;

/// How far the stage splits and how many probes it keeps. Production runs
/// [`MAX_SPLIT_ROUNDS`] under the array's layer cap; tests hold either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StageLimits {
    pub rounds: u32,
    pub cap: usize,
}

impl StageLimits {
    pub(crate) const PRODUCTION: Self = Self {
        rounds: MAX_SPLIT_ROUNDS,
        cap: MAX_REFLECTION_PROBES,
    };
}

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
    /// Id 27 as the level ships it: each animated light's bounce.
    pub delta_sh: Option<&'a DeltaShVolumesSection>,
    /// Id 25 as the level ships it: each animated light's direct.
    pub animated_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    /// Id 18: the runtime's lights, whose non-dynamic records are the spec
    /// lights the forward sdf loop shades.
    pub alpha_lights: &'a AlphaLightsSection,
    /// Id 23: the chunk windows the sdf selection reads.
    pub chunk_light_list: &'a ChunkLightListSection,
    /// Texture name → `.prm` key, from the TextureMips stage.
    pub texture_keys: &'a HashMap<String, [u8; 32]>,
    pub prm_root: &'a Path,
}

/// Id 54 for the level, from the cache when its key hits; `None` when no
/// probe is kept, so the section is omitted.
pub(crate) fn bake_or_load_reflection_probes(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    bake_or_load_with_limits(inputs, stage_cache, control, StageLimits::PRODUCTION)
}

/// [`bake_or_load_reflection_probes`] under `limits`. In order: placement,
/// capture, splitting (capture and retest, round by round), the cap.
pub(crate) fn bake_or_load_with_limits(
    inputs: &ReflectionProbeInputs<'_>,
    stage_cache: Option<&StageCache>,
    control: &BakeControl,
    limits: StageLimits,
) -> anyhow::Result<Option<ReflectionProbesSection>> {
    if inputs.reflection_volumes.is_empty()
        && up_facing_faces(inputs.environment_volumes).is_empty()
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
    let placement = place_probes(
        inputs.environment_volumes,
        inputs.reflection_volumes,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        limits.cap,
    )?;
    if placement.probes.is_empty() {
        return Ok(None);
    }

    // Capture, splitting and the cap are a pure function of the keyed
    // inputs, so a hit skips all three; the entry carries what their
    // warnings need, so a warm build warns as a cold one does.
    let key = cache_key(inputs, &placement, limits)?;
    if let Some((section, capped, reports)) = stage_cache
        .and_then(|cache| cache.get(&key))
        .and_then(|bytes| {
            parse_cache_payload(&bytes)
                .map_err(|e| log::warn!("[cache] corrupt reflection_probes entry, re-baking: {e}"))
                .ok()
        })
    {
        log::info!("[cache] reflection_probes hit");
        report(&section, &reports, capped, limits.cap, inputs);
        return Ok(Some(section));
    }
    log::info!("[cache] reflection_probes miss");

    let sh = ShVolumeSampler::new(inputs.sh_volume, PRODUCTION_PROBE_OCCLUSION)?;
    let deltas = inputs
        .delta_sh
        .map(|delta| {
            DeltaShSampler::new(
                delta,
                &sh,
                inputs.sh_volume.tile_dimension,
                inputs.sh_volume.grid_dimensions,
            )
        })
        .transpose()?;
    let references = references(&inputs.sh_volume.animation_descriptors);
    check_roster(inputs, references.len())?;
    let scene = CaptureScene {
        tracer,
        materials: SurfaceMaterials::new(
            &inputs.geometry.texture_names.names,
            inputs.texture_keys,
            inputs.prm_root,
        ),
        lightmap: LightmapSampler::new(inputs.lightmap),
        sh,
        animated_direct: inputs
            .animated_weight_maps
            .map(|maps| AnimatedDirectSampler::new(maps, inputs.lightmap)),
        deltas,
        sdf: SdfLights::new(inputs.alpha_lights, inputs.chunk_light_list),
        max_distance: ((bounds.max - bounds.min).length() + 1.0) as f32,
    };

    let mut probes: Vec<StageProbe> = placement
        .probes
        .into_iter()
        .map(|placed| StageProbe {
            placed,
            parent: None,
        })
        .collect();
    let mut published = 0;
    let mut captures = capture_probes(&scene, &probes, &references, control, &mut published)?;
    let faces = up_facing_faces(inputs.environment_volumes);
    let split_scene = SplitScene {
        tracer: &scene.tracer,
        classifier: &classifier,
        faces: &faces,
        max_distance: scene.max_distance,
        governor: control.governor(),
    };
    let mut batch: Range<usize> = 0..probes.len();
    for _ in 0..limits.rounds {
        let tiles: Vec<DistanceTile> = captures[batch.clone()]
            .iter()
            .map(|capture| {
                DistanceTile::from_bytes(
                    REFLECTION_PROBE_TILE_DIMENSION,
                    REFLECTION_PROBE_TILE_BORDER,
                    &capture.distance,
                )
            })
            .collect();
        let made = split_round(&mut probes, batch, &tiles, &split_scene);
        if made.is_empty() {
            break;
        }
        captures.extend(capture_probes(
            &scene,
            &probes[made.clone()],
            &references,
            control,
            &mut published,
        )?);
        batch = made;
    }
    let (kept, capped) = apply_probe_cap(&mut probes, placement.authored, limits.cap);

    let (section, reports) = assemble_section(&probes, captures, &kept);
    if let Some(cache) = stage_cache {
        cache.put(&key, &cache_payload(&section, capped, &reports));
    }
    report(&section, &reports, capped, limits.cap, inputs);
    Ok(Some(section))
}

/// Everything the stage tells the mapper about the finished section, from
/// the section and the cache entry alone.
fn report(
    section: &ReflectionProbesSection,
    reports: &[ShareReport],
    capped: usize,
    cap: usize,
    inputs: &ReflectionProbeInputs<'_>,
) {
    log_probes(section, reports);
    warn_capped(capped, cap);
    warn_hidden_water(section, inputs.environment_volumes);
}

/// Each animated light's reference value, by roster index: the runtime's
/// descriptors are the SH section's, one per AnimatedBakedLights row.
fn references(descriptors: &[AnimationDescriptor]) -> Vec<[f32; 3]> {
    descriptors.iter().map(reference_value).collect()
}

/// Every light the baked transport names must have a descriptor, and the
/// delta's descriptor table must be the identity: a share's roster index is
/// read straight into the runtime descriptor buffer (pin P10).
fn check_roster(inputs: &ReflectionProbeInputs<'_>, roster: usize) -> anyhow::Result<()> {
    if let Some(delta) = inputs.delta_sh {
        anyhow::ensure!(
            delta.animation_descriptor_indices.len() == roster
                && delta
                    .animation_descriptor_indices
                    .iter()
                    .enumerate()
                    .all(|(slot, &descriptor)| descriptor as usize == slot),
            "reflection probes need the identity animated-light roster in DeltaShVolumes"
        );
        anyhow::ensure!(
            delta.affinity_lights.iter().all(|&l| (l as usize) < roster),
            "DeltaShVolumes names an animated light past the {roster}-light roster"
        );
    }
    if let Some(maps) = inputs.animated_weight_maps {
        anyhow::ensure!(
            maps.texel_lights
                .iter()
                .all(|t| (t.light_index as usize) < roster),
            "AnimatedLightWeightMaps names an animated light past the {roster}-light roster"
        );
    }
    Ok(())
}

fn log_probes(section: &ReflectionProbesSection, reports: &[ShareReport]) {
    for (index, (probe, report)) in section.probes.iter().zip(reports).enumerate() {
        let faces = section
            .bindings
            .iter()
            .filter(|b| b.probe as usize == index)
            .count();
        log::info!(
            "[ReflectionProbes] probe {index}: entity {}, box {:?}..{:?}, capture point {:?}, \
             {faces} face(s)",
            probe.source_entity,
            probe.box_min,
            probe.box_max,
            probe.capture_point,
        );
        if report.animated_lights == 0 {
            log::info!("[ReflectionProbes] probe {index}: no animated light reaches its capture");
            continue;
        }
        let stored: Vec<String> = probe
            .lights
            .iter()
            .zip(report.totals)
            .filter(|(light, _)| light.is_used())
            .map(|(light, total)| {
                format!(
                    "light {} (reference [{:.3}, {:.3}, {:.3}], {:.3} of the capture)",
                    light.roster_index,
                    light.reference[0],
                    light.reference[1],
                    light.reference[2],
                    total
                )
            })
            .collect();
        log::info!(
            "[ReflectionProbes] probe {index}: covered share {:.3} of {} animated light(s); \
             stored {}",
            report.covered,
            report.animated_lights,
            stored.join(", "),
        );
        let held = report.animated_lights as usize - stored.len();
        if held > 0 {
            log::info!(
                "[ReflectionProbes] probe {index}: {held} animated light(s) past the \
                 {REFLECTION_PROBE_LIGHTS_PER_PROBE} stored stay at reference; covered share {:.3}",
                report.covered,
            );
        }
    }
    log::info!(
        "[ReflectionProbes] {} probe(s), {} binding(s), {} tile byte(s)",
        section.probes.len(),
        section.bindings.len(),
        section.tiles.len()
    );
}
