// Octahedral irradiance atlas baker. Uses L2 SH as an intermediate projection.
// See: context/lib/build_pipeline.md

mod animation;
mod bounce;
mod grid;
mod light_eval;
mod probe;
mod section;
mod sh_math;
mod trace;

pub use animation::validate_light_animations;
#[cfg(test)]
pub(crate) use bounce::bounce_sample_index;
pub(crate) use bounce::{reflected_radiance, soft_visibility_seed};
pub(crate) use grid::{
    ProbeGridLayout, probe_grid_dims, probe_grid_layout, probe_is_valid_pub, vec3_from,
};
pub(crate) use light_eval::{
    bounce_contribution, incident_radiance_at_point, light_reaches_point, spot_cone_parameters,
};
#[cfg(test)]
pub(crate) use probe::bake_probe;
pub(crate) use probe::{bake_probe_direct_rgb, bake_probe_indirect_rgb};
pub use section::log_stats;
pub(crate) use section::{
    compact_section_from_dense_atlas, encode_sh_volume_section_bc6h, f16_bits_to_f32,
    sh_atlas_array_layout,
};
pub(crate) use sh_math::pack_octahedral_irradiance_tile;
pub(crate) use trace::{Hit, bounce_hit, gather_hit, segment_clear};

pub(crate) use animation::animation_descriptor_for_light;
use probe::bake_probe_with_source;
use sh_math::pack_octahedral_irradiance_atlas;

use std::collections::HashSet;

use bvh::bvh::Bvh;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::{
    ANIMATED_SLOT_NONE, AnimationDescriptor, OCTAHEDRAL_PROBE_STRIDE, OctahedralAtlasTexel,
    OctahedralShProbe, OctahedralShVolumeSection,
};
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT;
use crate::map_data::MapLight;
use crate::partition::BspTree;
use crate::surface_radiance::PatchGather;
#[cfg(test)]
use crate::texel_footprint::EmbeddedEmitters;

/// Default grid cell size in meters. Overridden by `--probe-spacing`.
pub const DEFAULT_PROBE_SPACING: f32 = 1.0;

const RAYS_PER_PROBE: u32 = 256;

// The one-sample bounce strides a probe's rays across the emitter lattice; a
// whole number of passes is what makes every lattice sample equally likely.
const _: () = assert!(RAYS_PER_PROBE.is_multiple_of(DEFAULT_AREA_SAMPLE_COUNT));

/// Fallback albedo when a hit has no resolved diffuse mean. SH stays
/// indirect-only: lightmap carries the direct term; folding direct into SH would
/// double-count it at runtime.
const BOUNCE_ALBEDO: f32 = crate::surface_materials::stats::GREY_BOUNCE_ALBEDO;

pub(crate) const SKY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// Offset to avoid self-intersections at the probe origin and shadow-ray hit points.
const RAY_EPSILON: f32 = 1.0e-3;

/// Rotates the Fibonacci lattice off the (0,0,1) axis so axis-aligned light directions
/// don't land on a degenerate sample. No RNG — identical input yields byte-identical output.
const SAMPLING_LATTICE_OFFSET: u64 = 0x5048_4542_414b_4552; // "PHBAKER"

/// How the bounce resolves a soft light's shadow at a gather-ray hit. A hard
/// light traces its one hard ray either way, bit for bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BounceVisibility {
    /// One emitter sample per (hit, light), stratified across the probe's
    /// gather rays (`bounce_sample_index`). Every bake ships this.
    OneSample,
    /// `soft_visibility`'s probe-and-escalate set at every hit: the bake
    /// before stratification, kept as the parity reference. A light the set
    /// flags (keyed by its global `static_lights` index) traces its full set
    /// with no early-out, the probe-point bakes' embedded-emitter rule.
    #[cfg(test)]
    FullSet(&'static EmbeddedEmitters),
    /// [`Self::FullSet`] with every hit's seed re-avalanched under this salt:
    /// another draw of the full set's own lattice-rotation noise, the parity
    /// measurement's control.
    #[cfg(test)]
    FullSetReseeded(u64, &'static EmbeddedEmitters),
}

/// Where a probe gather ray's hit takes its outgoing radiance from. Either
/// way the ray, its hit and its distance are the same, so the depth moments
/// are too.
#[derive(Clone, Copy)]
pub(crate) enum RadianceSource<'a> {
    /// Shade every static light at the hit, resolving soft shadows by the
    /// visibility mode: today's per-hit estimator.
    Lights(BounceVisibility),
    /// Read the surface radiance cache's patch at the hit; traces no shadow
    /// ray. The SH base bake, warm and cold (`surface_radiance::bake_sh_base`).
    Patches(&'a PatchGather<'a>),
}

/// Shared between the base SH baker and per-light delta SH baker (`delta_sh_bake.rs`).
pub(crate) struct RaytracingCtx<'a> {
    pub bounce_materials: Option<&'a crate::surface_materials::stats::BounceMaterials>,
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
}

pub struct ShBakeCtx<'a> {
    pub bounce_materials: Option<&'a crate::surface_materials::stats::BounceMaterials>,
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub tree: &'a BspTree,
    /// Probes in these leaves are flagged invalid — out-of-map radiance pollutes
    /// trilinear interpolation at the playable edge.
    pub exterior_leaves: &'a HashSet<usize>,
    pub static_lights: &'a StaticBakedLights<'a>,
    pub animated_lights: &'a AnimatedBakedLights<'a>,
    /// Total `MapLight` count before static/animated filtering. Sizes the
    /// `slot_for_map_light` table emitted with `OctahedralShVolumeSection`.
    pub total_light_count: usize,
}

impl<'a> ShBakeCtx<'a> {
    pub(crate) fn ray_ctx(&self) -> RaytracingCtx<'a> {
        RaytracingCtx {
            bounce_materials: self.bounce_materials,
            bvh: self.bvh,
            primitives: self.primitives,
            geometry: self.geometry,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BakedProbe {
    pub(crate) coefficients: [f32; 27],
    pub(crate) metadata: OctahedralShProbe,
}

impl Default for BakedProbe {
    fn default() -> Self {
        Self {
            coefficients: [0.0; 27],
            metadata: OctahedralShProbe::default(),
        }
    }
}

/// CLI-driven configuration for the SH volume bake.
#[derive(serde::Serialize)]
pub struct ShConfig {
    pub probe_spacing: f32,
}

/// Collect the static-baked light slice in `static_lights` (global) order — the
/// order the per-hit radiance sum iterates and the SH base's layer plan reads.
pub(crate) fn static_light_refs<'a>(inputs: &ShBakeCtx<'a>) -> Vec<&'a MapLight> {
    inputs
        .static_lights
        .entries()
        .iter()
        .map(|e| e.light)
        .collect()
}

/// Returns an empty section (`grid_dimensions == [0,0,0]`) for empty geometry,
/// matching the "no SH section" degradation path at runtime.
pub fn bake_sh_volume(inputs: &ShBakeCtx<'_>, config: &ShConfig) -> OctahedralShVolumeSection {
    bake_sh_volume_controlled(inputs, config, &BakeControl::unrestricted())
}

pub fn bake_sh_volume_controlled(
    inputs: &ShBakeCtx<'_>,
    config: &ShConfig,
    control: &BakeControl,
) -> OctahedralShVolumeSection {
    bake_sh_volume_with_visibility(inputs, config, control, BounceVisibility::OneSample)
}

/// The whole-volume bake with the bounce's soft-light visibility chosen; the
/// production entry points pass [`BounceVisibility::OneSample`].
pub(crate) fn bake_sh_volume_with_visibility(
    inputs: &ShBakeCtx<'_>,
    config: &ShConfig,
    control: &BakeControl,
    visibility: BounceVisibility,
) -> OctahedralShVolumeSection {
    bake_sh_volume_with_source(inputs, config, control, RadianceSource::Lights(visibility))
}

/// The whole-volume bake with the gather hits' radiance source chosen.
pub(crate) fn bake_sh_volume_with_source(
    inputs: &ShBakeCtx<'_>,
    config: &ShConfig,
    control: &BakeControl,
    source: RadianceSource<'_>,
) -> OctahedralShVolumeSection {
    bake_sh_volume_after(inputs, config, control, source, 0)
}

/// [`bake_sh_volume_with_source`] as the last phase of a stage whose
/// earlier phases published `prior_units` of progress: the total it
/// publishes counts them, one unit per probe after.
pub(crate) fn bake_sh_volume_after(
    inputs: &ShBakeCtx<'_>,
    config: &ShConfig,
    control: &BakeControl,
    source: RadianceSource<'_>,
    prior_units: usize,
) -> OctahedralShVolumeSection {
    let layout = probe_grid_layout(inputs, config);
    if !layout.is_empty() {
        control.publish_total(prior_units + layout.total_probes());
    }
    bake_sh_volume_over(inputs, &layout, control, source)
}

/// The whole-volume bake over a resolved probe `layout`, advancing
/// `control` one unit per probe; the caller publishes the total. Returns an
/// empty section (`grid_dimensions == [0,0,0]`) for an empty layout.
pub(crate) fn bake_sh_volume_over(
    inputs: &ShBakeCtx<'_>,
    layout: &ProbeGridLayout,
    control: &BakeControl,
    source: RadianceSource<'_>,
) -> OctahedralShVolumeSection {
    if layout.is_empty() {
        return OctahedralShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: layout.cell_size,
            grid_dimensions: [0, 0, 0],
            probe_stride: OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            layer_count: 0,
            tiles_per_layer: 0,
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            compact_atlas: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: vec![ANIMATED_SLOT_NONE; inputs.total_light_count],
        };
    }

    let world_min = layout.world_min;
    let dims = layout.dims;
    let total = layout.total_probes();
    let cell_size = layout.cell_size;
    let far_sentinel = layout.far_sentinel;

    let static_lights: Vec<&MapLight> = static_light_refs(inputs);
    let (animation_descriptors, slot_for_map_light) = animation_tables(inputs);

    let baked_probes: Vec<BakedProbe> = (0..total)
        .into_par_iter()
        .map(|i| {
            let _permit = control.governor().enter();
            let probe = bake_probe_with_source(
                inputs,
                vec3_from(layout.probe_positions[i]),
                &static_lights,
                // Whole-volume bake: the slice IS the global set, so identity.
                None,
                far_sentinel,
                layout.validity[i] != 0,
                i as u64,
                source,
            );
            control.advance(1);
            probe
        })
        .collect();
    let base_probes: Vec<OctahedralShProbe> = baked_probes.iter().map(|p| p.metadata).collect();
    let atlas_layout = sh_atlas_array_layout(dims, DEFAULT_IRRADIANCE_TILE_DIMENSION)
        .unwrap_or_else(|e| panic!("{e}"));
    let atlas_texels = pack_octahedral_irradiance_atlas(
        &baked_probes,
        dims,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_layout,
    );

    let section = OctahedralShVolumeSection {
        grid_origin: [world_min.x as f32, world_min.y as f32, world_min.z as f32],
        cell_size,
        grid_dimensions: dims,
        probe_stride: OCTAHEDRAL_PROBE_STRIDE,
        tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
        tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
        atlas_dimensions: [atlas_layout.atlas_width, atlas_layout.atlas_height],
        layer_count: atlas_layout.layer_count,
        tiles_per_layer: atlas_layout.tiles_per_layer,
        atlas_tiles_per_row: atlas_layout.atlas_tiles_per_row,
        probes: base_probes,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        compact_atlas: Vec::new(),
        animation_descriptors,
        slot_for_map_light,
    };
    compact_section_from_dense_atlas(section, &atlas_texels)
}

/// The id-34 section's animation tables: one runtime descriptor per animated
/// light, and the map-light-index to animated-slot table the runtime
/// animation lookup reads. They follow the animated light set alone, never
/// a probe ray, so every build derives them afresh; planning uses the same
/// metadata before probe rays run. Per-light monochrome SH layers are gone:
/// animated indirect is the SH compose pass's delta volumes.
pub(crate) fn animation_tables(inputs: &ShBakeCtx<'_>) -> (Vec<AnimationDescriptor>, Vec<u32>) {
    let descriptors = inputs
        .animated_lights
        .entries()
        .iter()
        .map(|entry| animation_descriptor_for_light(entry.light))
        .collect();
    let slots = inputs
        .animated_lights
        .slot_for_source_lights(inputs.total_light_count);
    (descriptors, slots)
}

/// Fold every code constant the probe gather reads into `hasher`: its ray
/// count and lattice rotation, the self-intersection offset, the sky colour,
/// the grey bounce fallback, and the tile geometry and format of the
/// gathered volume. A cache key over the gather's output folds these, so
/// changing one misses it without relying on a stage-version bump.
pub(crate) fn fold_gather_constants(hasher: &mut blake3::Hasher) {
    hasher.update(&RAYS_PER_PROBE.to_le_bytes());
    hasher.update(&SAMPLING_LATTICE_OFFSET.to_le_bytes());
    hasher.update(&RAY_EPSILON.to_le_bytes());
    for channel in SKY_COLOR {
        hasher.update(&channel.to_le_bytes());
    }
    hasher.update(&BOUNCE_ALBEDO.to_le_bytes());
    hasher.update(&DEFAULT_IRRADIANCE_TILE_DIMENSION.to_le_bytes());
    hasher.update(&DEFAULT_IRRADIANCE_TILE_BORDER.to_le_bytes());
    hasher.update(&IRRADIANCE_FORMAT_RGBA16F.to_le_bytes());
}

/// Host bytes the whole-volume bake of a `dims` grid holds, by
/// representation: what the SH base working-set gate prices before the bake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShVolumeBytes {
    /// Every probe's baked coefficients and metadata plus the dense atlas
    /// they pack into: co-resident while the bake compacts the atlas.
    pub(crate) dense: u64,
    /// One encoded or decoded copy of the section the bake returns: probe
    /// metadata plus an atlas no larger than the dense one.
    pub(crate) section: u64,
}

/// [`ShVolumeBytes`] for a `dims` grid, or `None` when a product overflows.
/// A grid too large for the atlas layout prices one tile per probe; the bake
/// refuses that grid itself.
pub(crate) fn sh_volume_bytes(dims: [u32; 3]) -> Option<ShVolumeBytes> {
    let probes = u64::from(dims[0])
        .checked_mul(u64::from(dims[1]))?
        .checked_mul(u64::from(dims[2]))?;
    if probes == 0 {
        return Some(ShVolumeBytes {
            dense: 0,
            section: 0,
        });
    }
    let tile_texels = u64::from(DEFAULT_IRRADIANCE_TILE_DIMENSION).pow(2);
    let atlas_texels = match sh_atlas_array_layout(dims, DEFAULT_IRRADIANCE_TILE_DIMENSION) {
        Ok(layout) => u64::from(layout.layer_count)
            .checked_mul(u64::from(layout.atlas_width))?
            .checked_mul(u64::from(layout.atlas_height))?,
        Err(_) => probes.checked_mul(tile_texels)?,
    };
    let atlas = atlas_texels.checked_mul(size_of::<OctahedralAtlasTexel>() as u64)?;
    let dense = probes
        .checked_mul(size_of::<BakedProbe>() as u64)?
        .checked_add(atlas)?;
    let section = probes
        .checked_mul(size_of::<OctahedralShProbe>() as u64)?
        .checked_add(atlas)?;
    Some(ShVolumeBytes { dense, section })
}

#[cfg(test)]
pub(crate) mod bounce_parity_tests;

#[cfg(test)]
mod animation_tests;
#[cfg(test)]
mod bounce_estimator_tests;
#[cfg(test)]
mod bounce_tests;
#[cfg(test)]
mod colour_tests;
#[cfg(test)]
mod empty_material_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod light_eval_tests;
#[cfg(test)]
mod moments_tests;
#[cfg(test)]
mod patch_source_tests;
#[cfg(test)]
mod section_tests;
#[cfg(test)]
mod sh_math_tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod trace_tests;
#[cfg(test)]
mod volume_tests;
