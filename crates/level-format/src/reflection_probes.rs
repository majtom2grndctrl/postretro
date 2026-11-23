// ReflectionProbes PRL section (ID 54): compile-time box-projected probes,
// their per-light shares, the fluid and world faces they bind, and their
// colour and distance tiles.
// See: context/lib/resource_management.md §5 · build_pipeline.md §PRL section IDs

mod error;
mod wire;

pub use error::{BindingList, ReflectionProbesError};

use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::prm::PORTABLE_MAX_TEXTURE_ARRAY_LAYERS;

pub const REFLECTION_PROBES_VERSION: u16 = 3;

/// Engine capture tile edge in texels, border included. Carried in the header,
/// so a later authored size is additive.
pub const REFLECTION_PROBE_TILE_DIMENSION: u32 = 128;
/// The SH atlas's octahedral border convention: one texel copied across the
/// wrap (`octahedral::irradiance_tile_source_texel`).
pub const REFLECTION_PROBE_TILE_BORDER: u32 = 1;
/// Animated lights stored per probe (K). The fluid shader's share table is
/// sized by it, so a section with another K is refused.
pub const REFLECTION_PROBE_LIGHTS_PER_PROBE: usize = 4;
/// Directional lobes per share entry: +Y, +X, −X, +Z, −Z, −Y. −Y is last, so
/// the five a water surface reads keep the indices they had before it.
pub const REFLECTION_PROBE_LOBE_COUNT: usize = 6;
/// Probes per level: one array layer each, within the portable layer ceiling.
pub const MAX_REFLECTION_PROBES: usize = PORTABLE_MAX_TEXTURE_ARRAY_LAYERS as usize;
/// Share entry roster index marking an unused entry; it names no light.
pub const REFLECTION_PROBE_NO_LIGHT: u32 = u32::MAX;
/// Distance format tag, the only value: little-endian f16 per texel holding
/// the mean over its supersamples of 1/|hit − capture point|₁, 0 where the
/// ray hit nothing. Inverse L1 reconstructs planes exactly under bilinear
/// filtering within an octant, and 0 is the natural "no surface" sentinel.
pub const REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1: u32 = 0;
/// Smallest colour mip edge: one BC6H block.
pub const REFLECTION_PROBE_MIN_MIP_DIMENSION: u32 = 4;

/// Blend weights of the six share lobes for a lookup `direction`, in lobe
/// order: +Y, +X, −X, +Z, −Z, −Y. The compiler weights each captured texel's
/// light into the lobes by this, and the shaders and render-cpu blend the
/// stored shares by the same function, so they agree exactly.
///
/// Each lobe is its clamped cosine squared, `max(d · axis, 0)²`. For a unit
/// `d` the six clamped squares are `x² + y² + z² = 1`, so the weights sum to
/// 1 by construction; the final divide only removes rounding and a non-unit
/// input's length. Exponent 2 is the trig-free choice: each term is C¹
/// across its axis plane, so the blended ratio has no crease at a lobe edge,
/// and a lobe still holds half its weight 45° off its axis. For `y ≥ 0` the
/// −Y weight is exactly 0 and the first five are bit-identical to the
/// five-lobe form water was compiled against. A zero or non-finite direction
/// falls back to the +Y lobe, so the result is never NaN.
pub fn reflection_probe_lobe_weights(direction: [f32; 3]) -> [f32; REFLECTION_PROBE_LOBE_COUNT] {
    let [x, y, z] = direction;
    let pos = |v: f32| {
        let c = v.max(0.0);
        c * c
    };
    let weights = [pos(y), pos(x), pos(-x), pos(z), pos(-z), pos(-y)];
    let sum: f32 = weights.iter().sum();
    if !(sum.is_finite() && sum > 0.0) {
        return [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }
    weights.map(|w| w / sum)
}

/// Present only when some probe survived; absent means no probes.
///
/// On-disk layout (little-endian):
///   u16  version (3)
///   u32  tile dimension, u32 tile border, u32 atlas format tag (id 22's tags),
///   u32  colour mip count (≥ 1),
///   u32  distance format tag (`REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1`)
///   u32  lights per probe K, u32 lobe count, u32 probe count,
///   u32  fluid binding count, u32 world binding count
///   probe × count: u32 source entity, f32×3 box min, f32×3 box max,
///     f32×3 capture point, u32 layer, then K share entries:
///     u32 roster index, f32×3 RGB reference, f32×3 share × lobe count
///   fluid binding × count: u32 volume, u32 face, u32 probe
///   world binding × count: u32 face, u32 probe
///   colour tiles, layer-major: per probe, mips 0..count in sequence, mip m
///     at tile dimension >> m with its own 1-texel border, each in the atlas
///     format
///   distance tiles, layer-major, one level per probe at the tile dimension
///     and border, little-endian f16
///
/// Each binding list ascends strictly (fluid by (volume, face), world by
/// face), so a face binds at most once and the order is canonical.
/// Cross-section references (fluid volume and face against
/// EnvironmentVolumes, world face against Geometry, roster index against the
/// animated-light roster) are checked by the loader, which holds every
/// section.
#[derive(Debug, Clone, PartialEq)]
pub struct ReflectionProbesSection {
    pub tile_dimension: u32,
    pub tile_border: u32,
    /// `lightmap::IRRADIANCE_FORMAT_BC6H` or `IRRADIANCE_FORMAT_RGBA16F`.
    pub atlas_format: u32,
    /// Colour mips per probe, at least 1; every mip whole 4×4 blocks.
    pub colour_mip_count: u32,
    /// Authored probes in map entity order, then derived probes in fluid
    /// entity order.
    pub probes: Vec<ReflectionProbeRecord>,
    /// Fluid faces in EnvironmentVolumes (id 52) numbering.
    pub fluid_bindings: Vec<ReflectionProbeFluidBinding>,
    /// Static world faces in Geometry (id 17) numbering.
    pub world_bindings: Vec<ReflectionProbeWorldBinding>,
    /// Per probe in layer order, its colour mip chain.
    pub tiles: Vec<u8>,
    /// `REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1`.
    pub distance_format: u32,
    /// One distance tile per probe, layer-major, in the colour tile's mapping
    /// and border.
    pub distances: Vec<u8>,
}

/// One probe. Reflector-agnostic: nothing here is water-specific.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReflectionProbeRecord {
    /// The authored `reflection_volume` brush, or the fluid entity of a
    /// derived probe; for diagnostics.
    pub source_entity: u32,
    /// Projection box in engine space (Y-up, meters).
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    /// Where the tile was captured from; inside the box.
    pub capture_point: [f32; 3],
    /// Array layer holding this probe's tile.
    pub layer: u32,
    /// Top-K animated lights by contribution; unused entries hold
    /// `REFLECTION_PROBE_NO_LIGHT`.
    pub lights: [ReflectionProbeLightShare; REFLECTION_PROBE_LIGHTS_PER_PROBE],
}

/// One animated light's share of a probe's capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReflectionProbeLightShare {
    /// AnimatedBakedLights roster index, the runtime descriptor index.
    pub roster_index: u32,
    /// The light's RGB value at the capture's reference state.
    pub reference: [f32; 3],
    /// Per lobe, per RGB channel: the light's fraction of the captured light,
    /// in [0, 1].
    pub shares: [[f32; 3]; REFLECTION_PROBE_LOBE_COUNT],
}

impl ReflectionProbeLightShare {
    /// An unused entry, written as zeros so the section stays canonical.
    pub const UNUSED: Self = Self {
        roster_index: REFLECTION_PROBE_NO_LIGHT,
        reference: [0.0; 3],
        shares: [[0.0; 3]; REFLECTION_PROBE_LOBE_COUNT],
    };

    pub fn is_used(&self) -> bool {
        self.roster_index != REFLECTION_PROBE_NO_LIGHT
    }
}

/// An up-facing fluid face, addressed in EnvironmentVolumes (id 52) numbering,
/// and the probe it reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReflectionProbeFluidBinding {
    pub volume: u32,
    pub face: u32,
    pub probe: u32,
}

/// A static world face, addressed in Geometry (id 17) numbering, and the
/// probe it reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReflectionProbeWorldBinding {
    pub face: u32,
    pub probe: u32,
}

/// Bytes of one tile of `dimension` texels square in `format`, or `None` for
/// an unknown format.
pub fn reflection_probe_tile_bytes(format: u32, dimension: u32) -> Option<usize> {
    let dimension = dimension as usize;
    match format {
        IRRADIANCE_FORMAT_BC6H => Some((dimension / 4) * (dimension / 4) * 16),
        IRRADIANCE_FORMAT_RGBA16F => Some(dimension * dimension * 8),
        _ => None,
    }
}

/// Whether a `tile_dimension` tile carries `mip_count` colour mips: at least
/// one, each halving exactly, the last at least one 4×4 block and every one
/// whole blocks.
pub fn reflection_probe_colour_mips_valid(tile_dimension: u32, mip_count: u32) -> bool {
    let Some(last) = mip_count.checked_sub(1) else {
        return false;
    };
    let Some(block_span) = REFLECTION_PROBE_MIN_MIP_DIMENSION.checked_shl(last) else {
        return false;
    };
    // `checked_shl` only refuses a shift past the width; a span that wrapped
    // to a smaller value would pass the divide below.
    block_span >> last == REFLECTION_PROBE_MIN_MIP_DIMENSION
        && tile_dimension >= block_span
        && tile_dimension.is_multiple_of(block_span)
}

/// Edge of colour mip `mip` of a `tile_dimension` tile, border included.
pub fn reflection_probe_colour_mip_dimension(tile_dimension: u32, mip: u32) -> u32 {
    tile_dimension >> mip
}

/// Bytes of one probe's colour chain, `mip_count` mips of a `dimension` tile
/// in `format`, or `None` for an unknown format or an invalid chain.
pub fn reflection_probe_colour_chain_bytes(
    format: u32,
    dimension: u32,
    mip_count: u32,
) -> Option<usize> {
    if !reflection_probe_colour_mips_valid(dimension, mip_count) {
        return None;
    }
    (0..mip_count)
        .map(|mip| {
            reflection_probe_tile_bytes(
                format,
                reflection_probe_colour_mip_dimension(dimension, mip),
            )
        })
        .sum()
}

/// Bytes of one distance tile of `dimension` texels square in `format`, or
/// `None` for an unknown format.
pub fn reflection_probe_distance_tile_bytes(format: u32, dimension: u32) -> Option<usize> {
    let dimension = dimension as usize;
    match format {
        REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1 => Some(dimension * dimension * 2),
        _ => None,
    }
}

/// A distance payload of `probe_count` tiles that see no surface: every
/// lookup falls back to the box hit.
pub fn reflection_probe_zero_distances(probe_count: usize, dimension: u32) -> Vec<u8> {
    let bytes =
        reflection_probe_distance_tile_bytes(REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, dimension)
            .expect("the f16 inverse-L1 format is known");
    vec![0; probe_count * bytes]
}

impl ReflectionProbesSection {
    /// Bytes of one probe's colour chain.
    fn colour_chain_bytes(&self) -> usize {
        reflection_probe_colour_chain_bytes(
            self.atlas_format,
            self.tile_dimension,
            self.colour_mip_count,
        )
        .expect("a decoded or compiled section has a known atlas format and a valid mip chain")
    }

    /// Colour mip `mip` of the probe on array layer `layer`.
    pub fn colour_tile(&self, layer: usize, mip: u32) -> &[u8] {
        assert!(
            mip < self.colour_mip_count,
            "mip {mip} is past the {} colour mips",
            self.colour_mip_count
        );
        let tile = |m: u32| {
            reflection_probe_tile_bytes(
                self.atlas_format,
                reflection_probe_colour_mip_dimension(self.tile_dimension, m),
            )
            .expect("a decoded or compiled section has a known atlas format")
        };
        let start = layer * self.colour_chain_bytes() + (0..mip).map(tile).sum::<usize>();
        &self.tiles[start..start + tile(mip)]
    }

    /// One probe's distance tile bytes (little-endian f16).
    pub fn distance_tile(&self, layer: usize) -> &[u8] {
        let bytes = reflection_probe_distance_tile_bytes(self.distance_format, self.tile_dimension)
            .expect("a decoded or compiled section has a known distance format");
        &self.distances[layer * bytes..(layer + 1) * bytes]
    }

    /// The loader's check of the world bindings against Geometry (id 17):
    /// every bound face exists. The lists' own order is the decoder's.
    pub fn check_world_faces(&self, face_count: usize) -> Result<(), ReflectionProbesError> {
        match self
            .world_bindings
            .iter()
            .enumerate()
            .find(|(_, binding)| binding.face as usize >= face_count)
        {
            Some((index, binding)) => Err(ReflectionProbesError::WorldFacePastGeometry {
                index,
                face: binding.face,
                face_count,
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "reflection_probes_tests.rs"]
mod tests;
