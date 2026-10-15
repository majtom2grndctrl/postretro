// ReflectionProbes PRL section (ID 54): compile-time box-projected probes,
// their per-light shares, the fluid faces they bind, and their capture and
// distance tiles.
// See: context/lib/resource_management.md §5 · build_pipeline.md §PRL section IDs

use crate::FormatError;
use crate::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use crate::prm::PORTABLE_MAX_TEXTURE_ARRAY_LAYERS;

pub const REFLECTION_PROBES_VERSION: u16 = 2;

/// Engine capture tile edge in texels, border included. Carried in the header,
/// so a later authored size is additive.
pub const REFLECTION_PROBE_TILE_DIMENSION: u32 = 128;
/// The SH atlas's octahedral border convention: one texel copied across the
/// wrap (`octahedral::irradiance_tile_source_texel`).
pub const REFLECTION_PROBE_TILE_BORDER: u32 = 1;
/// Animated lights stored per probe (K). The fluid shader's share table is
/// sized by it, so a section with another K is refused.
pub const REFLECTION_PROBE_LIGHTS_PER_PROBE: usize = 4;
/// Directional lobes per share entry: up, then +X, −X, +Z, −Z.
pub const REFLECTION_PROBE_LOBE_COUNT: usize = 5;
/// Probes per level: one array layer each, within the portable layer ceiling.
pub const MAX_REFLECTION_PROBES: usize = PORTABLE_MAX_TEXTURE_ARRAY_LAYERS as usize;
/// Share entry roster index marking an unused entry; it names no light.
pub const REFLECTION_PROBE_NO_LIGHT: u32 = u32::MAX;
/// Distance format tag, the only value: little-endian f16 per texel holding
/// the mean over its supersamples of 1/|hit − capture point|₁, 0 where the
/// ray hit nothing. Inverse L1 reconstructs planes exactly under bilinear
/// filtering within an octant, and 0 is the natural "no surface" sentinel.
pub const REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1: u32 = 0;

/// Blend weights of the five share lobes for a lookup `direction`, in lobe
/// order: up (+Y), +X, −X, +Z, −Z. The compiler weights each captured texel's
/// light into the lobes by this, and the fluid shader and render-cpu blend
/// the stored shares by the same function, so the two agree exactly.
///
/// Each lobe is its clamped cosine squared, `max(d · axis, 0)²`, and the
/// downward part `max(−y, 0)²` is split evenly over the four horizontal lobes.
/// For a unit `d` the six clamped squares are `x² + y² + z² = 1`, so the
/// weights sum to 1 by construction; the final divide only removes rounding
/// and a non-unit input's length. Exponent 2 is the trig-free choice: each
/// term is C¹ across its axis plane, so the blended ratio has no crease at a
/// lobe edge (P7), and a lobe still holds half its weight 45° off its axis.
/// Straight down gives each horizontal lobe ¼, the limit of every nearby
/// direction. A zero or non-finite direction falls back to the up lobe, so
/// the result is never NaN.
pub fn reflection_probe_lobe_weights(direction: [f32; 3]) -> [f32; REFLECTION_PROBE_LOBE_COUNT] {
    let [x, y, z] = direction;
    let pos = |v: f32| {
        let c = v.max(0.0);
        c * c
    };
    let down = pos(-y) * 0.25;
    let weights = [
        pos(y),
        pos(x) + down,
        pos(-x) + down,
        pos(z) + down,
        pos(-z) + down,
    ];
    let sum: f32 = weights.iter().sum();
    if !(sum.is_finite() && sum > 0.0) {
        return [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    weights.map(|w| w / sum)
}

/// Largest tile edge a decoder accepts; past it a corrupt header would ask
/// for an absurd texture before the payload length check could catch it.
const MAX_TILE_DIMENSION: u32 = 1024;

const HEADER_SIZE: usize = 2 + 4 * 8;
const SHARE_ENTRY_SIZE: usize = 4 + 12 + REFLECTION_PROBE_LOBE_COUNT * 12;
const PROBE_SIZE: usize = 4 + 12 * 3 + 4 + REFLECTION_PROBE_LIGHTS_PER_PROBE * SHARE_ENTRY_SIZE;
const BINDING_SIZE: usize = 12;

/// Present only when some up-facing fluid face kept a probe; absent means no
/// probes.
///
/// On-disk layout (little-endian):
///   u16  version (2)
///   u32  tile dimension, u32 tile border, u32 atlas format tag (id 22's tags),
///   u32  distance format tag (`REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1`)
///   u32  lights per probe K, u32 lobe count, u32 probe count, u32 binding count
///   probe × count: u32 source entity, f32×3 box min, f32×3 box max,
///     f32×3 capture point, u32 layer, then K share entries:
///     u32 roster index, f32×3 RGB reference, f32×3 share × lobe count
///   binding × count: u32 volume, u32 face, u32 probe
///   tiles, layer-major, each in the atlas format
///   distance tiles, layer-major, one per probe at the tile dimension and
///     border, little-endian f16
///
/// Bindings ascend strictly by (volume, face), so a face binds at most once
/// and the order is canonical. Cross-section references (volume and face
/// against EnvironmentVolumes, roster index against the animated-light
/// roster) are checked by the loader, which holds every section.
#[derive(Debug, Clone, PartialEq)]
pub struct ReflectionProbesSection {
    pub tile_dimension: u32,
    pub tile_border: u32,
    /// `lightmap::IRRADIANCE_FORMAT_BC6H` or `IRRADIANCE_FORMAT_RGBA16F`.
    pub atlas_format: u32,
    /// Authored probes in map entity order, then derived probes in fluid
    /// entity order.
    pub probes: Vec<ReflectionProbeRecord>,
    pub bindings: Vec<ReflectionProbeBinding>,
    /// One tile per probe, layer-major.
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
pub struct ReflectionProbeBinding {
    pub volume: u32,
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
    pub fn byte_len(&self) -> usize {
        HEADER_SIZE
            + self.probes.len() * PROBE_SIZE
            + self.bindings.len() * BINDING_SIZE
            + self.tiles.len()
            + self.distances.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&REFLECTION_PROBES_VERSION.to_le_bytes());
        u32_out(&mut out, self.tile_dimension);
        u32_out(&mut out, self.tile_border);
        u32_out(&mut out, self.atlas_format);
        u32_out(&mut out, self.distance_format);
        count(&mut out, REFLECTION_PROBE_LIGHTS_PER_PROBE);
        count(&mut out, REFLECTION_PROBE_LOBE_COUNT);
        count(&mut out, self.probes.len());
        count(&mut out, self.bindings.len());
        for probe in &self.probes {
            u32_out(&mut out, probe.source_entity);
            vec3(&mut out, probe.box_min);
            vec3(&mut out, probe.box_max);
            vec3(&mut out, probe.capture_point);
            u32_out(&mut out, probe.layer);
            for light in &probe.lights {
                u32_out(&mut out, light.roster_index);
                vec3(&mut out, light.reference);
                for lobe in light.shares {
                    vec3(&mut out, lobe);
                }
            }
        }
        for binding in &self.bindings {
            u32_out(&mut out, binding.volume);
            u32_out(&mut out, binding.face);
            u32_out(&mut out, binding.probe);
        }
        out.extend_from_slice(&self.tiles);
        out.extend_from_slice(&self.distances);
        out
    }

    /// Decode untrusted section bytes: another version, a malformed header, a
    /// binding past the probe count, a face bound twice, a tile or distance
    /// payload of the wrong length, or a negative or non-finite distance is
    /// refused. Every count is bounded by the bytes left
    /// before anything is allocated.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = Reader { data, offset: 0 };
        let version = r.u16("version")?;
        if version != REFLECTION_PROBES_VERSION {
            return invalid(format!(
                "unsupported version {version}; this engine reads version {REFLECTION_PROBES_VERSION}"
            ));
        }
        let tile_dimension = r.u32("tile dimension")?;
        let tile_border = r.u32("tile border")?;
        let atlas_format = r.u32("atlas format")?;
        let distance_format = r.u32("distance format")?;
        let lights_per_probe = r.u32("lights per probe")? as usize;
        let lobe_count = r.u32("lobe count")? as usize;
        if tile_dimension < 4 || tile_dimension % 4 != 0 || tile_dimension > MAX_TILE_DIMENSION {
            return invalid(format!(
                "tile dimension {tile_dimension} must be a multiple of 4 in 4..={MAX_TILE_DIMENSION}"
            ));
        }
        if tile_border != REFLECTION_PROBE_TILE_BORDER {
            return invalid(format!(
                "tile border {tile_border}; the octahedral convention is {REFLECTION_PROBE_TILE_BORDER}"
            ));
        }
        let Some(tile_bytes) = reflection_probe_tile_bytes(atlas_format, tile_dimension) else {
            return invalid(format!("unknown atlas format {atlas_format}"));
        };
        let Some(distance_tile_bytes) =
            reflection_probe_distance_tile_bytes(distance_format, tile_dimension)
        else {
            return invalid(format!("unknown distance format {distance_format}"));
        };
        if lights_per_probe != REFLECTION_PROBE_LIGHTS_PER_PROBE
            || lobe_count != REFLECTION_PROBE_LOBE_COUNT
        {
            return invalid(format!(
                "{lights_per_probe} lights × {lobe_count} lobes per probe; this engine reads \
                 {REFLECTION_PROBE_LIGHTS_PER_PROBE} × {REFLECTION_PROBE_LOBE_COUNT}"
            ));
        }

        let probe_count = r.bounded_count("probe count", PROBE_SIZE)?;
        let binding_count = r.u32("binding count")? as usize;
        if probe_count == 0 {
            return invalid("no probes; a level without probes omits the section".into());
        }
        if probe_count > MAX_REFLECTION_PROBES {
            return invalid(format!(
                "{probe_count} probes exceed the {MAX_REFLECTION_PROBES}-layer cap"
            ));
        }
        let mut probes = Vec::with_capacity(probe_count);
        let mut layer_taken = vec![false; probe_count];
        for p in 0..probe_count {
            let probe = read_probe(&mut r, p)?;
            let layer = probe.layer as usize;
            if layer >= probe_count {
                return invalid(format!(
                    "probe {p} layer {layer} is past the {probe_count} layers"
                ));
            }
            if std::mem::replace(&mut layer_taken[layer], true) {
                return invalid(format!("probe {p} reuses layer {layer}"));
            }
            probes.push(probe);
        }

        let remaining = data.len() - r.offset;
        if binding_count > remaining / BINDING_SIZE {
            return invalid(format!(
                "binding count {binding_count} exceeds what the remaining {remaining} bytes can hold"
            ));
        }
        let mut bindings: Vec<ReflectionProbeBinding> = Vec::with_capacity(binding_count);
        for b in 0..binding_count {
            let binding = ReflectionProbeBinding {
                volume: r.u32(&format!("binding {b} volume"))?,
                face: r.u32(&format!("binding {b} face"))?,
                probe: r.u32(&format!("binding {b} probe"))?,
            };
            if binding.probe as usize >= probe_count {
                return invalid(format!(
                    "binding {b} names probe {}, past the {probe_count} probes",
                    binding.probe
                ));
            }
            if let Some(previous) = bindings.last()
                && (binding.volume, binding.face) <= (previous.volume, previous.face)
            {
                let (volume, face) = (binding.volume, binding.face);
                return invalid(if (volume, face) == (previous.volume, previous.face) {
                    format!("volume {volume} face {face} is bound twice")
                } else {
                    format!("binding {b} (volume {volume}, face {face}) is out of order")
                });
            }
            bindings.push(binding);
        }

        let tiles_len = probe_count * tile_bytes;
        let tiles = r.take(tiles_len, "tile payload")?.to_vec();
        let distances = r
            .take(probe_count * distance_tile_bytes, "distance payload")?
            .to_vec();
        if let Some(texel) = distances.chunks_exact(2).position(|bits| {
            !f16_bits_are_finite_non_negative(u16::from_le_bytes([bits[0], bits[1]]))
        }) {
            return invalid(format!(
                "distance payload holds a negative or non-finite value at texel {texel}"
            ));
        }
        if r.offset != data.len() {
            return invalid(format!(
                "trailing bytes: expected {}, got {}",
                r.offset,
                data.len()
            ));
        }
        Ok(Self {
            tile_dimension,
            tile_border,
            atlas_format,
            probes,
            bindings,
            tiles,
            distance_format,
            distances,
        })
    }

    /// One probe's tile bytes.
    pub fn tile(&self, layer: usize) -> &[u8] {
        let bytes = reflection_probe_tile_bytes(self.atlas_format, self.tile_dimension)
            .expect("a decoded or compiled section has a known atlas format");
        &self.tiles[layer * bytes..(layer + 1) * bytes]
    }

    /// One probe's distance tile bytes (little-endian f16).
    pub fn distance_tile(&self, layer: usize) -> &[u8] {
        let bytes = reflection_probe_distance_tile_bytes(self.distance_format, self.tile_dimension)
            .expect("a decoded or compiled section has a known distance format");
        &self.distances[layer * bytes..(layer + 1) * bytes]
    }
}

/// Sign bit clear and exponent not all ones: a finite f16 at or above zero.
fn f16_bits_are_finite_non_negative(bits: u16) -> bool {
    bits & 0x8000 == 0 && bits & 0x7C00 != 0x7C00
}

fn read_probe(r: &mut Reader<'_>, p: usize) -> crate::Result<ReflectionProbeRecord> {
    let what = format!("probe {p}");
    let source_entity = r.u32(&format!("{what} source entity"))?;
    let box_min = r.finite_vec3(&format!("{what} box min"))?;
    let box_max = r.finite_vec3(&format!("{what} box max"))?;
    let capture_point = r.finite_vec3(&format!("{what} capture point"))?;
    let layer = r.u32(&format!("{what} layer"))?;
    if (0..3).any(|axis| box_min[axis] > box_max[axis]) {
        return invalid(format!("{what} box min exceeds max"));
    }
    if (0..3).any(|axis| !(box_min[axis]..=box_max[axis]).contains(&capture_point[axis])) {
        return invalid(format!("{what} capture point lies outside its box"));
    }
    let mut lights = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
    for (k, light) in lights.iter_mut().enumerate() {
        let what = format!("{what} light {k}");
        light.roster_index = r.u32(&format!("{what} roster index"))?;
        light.reference = r.finite_vec3(&format!("{what} reference"))?;
        if light.reference.iter().any(|channel| *channel < 0.0) {
            return invalid(format!("{what} reference is negative"));
        }
        for (l, lobe) in light.shares.iter_mut().enumerate() {
            *lobe = r.finite_vec3(&format!("{what} lobe {l} share"))?;
            if lobe.iter().any(|share| !(0.0..=1.0).contains(share)) {
                return invalid(format!("{what} lobe {l} share lies outside 0..=1"));
            }
        }
        if !light.is_used() && *light != ReflectionProbeLightShare::UNUSED {
            return invalid(format!("{what} is unused but carries values"));
        }
    }
    Ok(ReflectionProbeRecord {
        source_entity,
        box_min,
        box_max,
        capture_point,
        layer,
        lights,
    })
}

fn invalid<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("reflection probes: {message}"),
    )))
}

fn count(out: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).expect("reflection probe counts fit the PRL u32 field");
    u32_out(out, n);
}

fn u32_out(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn vec3(out: &mut Vec<u8>, value: [f32; 3]) {
    for v in value {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize, what: &str) -> crate::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("reflection probes: truncated {what}"),
                ))
            })?;
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u16(&mut self, what: &str) -> crate::Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, what: &str) -> crate::Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn finite_f32(&mut self, what: &str) -> crate::Result<f32> {
        let value = f32::from_bits(self.u32(what)?);
        if !value.is_finite() {
            return invalid(format!("{what} is not finite"));
        }
        Ok(value)
    }

    fn finite_vec3(&mut self, what: &str) -> crate::Result<[f32; 3]> {
        Ok([
            self.finite_f32(what)?,
            self.finite_f32(what)?,
            self.finite_f32(what)?,
        ])
    }

    /// A count whose elements each need at least `min_element_size` bytes,
    /// rejected before allocation when the remaining bytes cannot hold it.
    fn bounded_count(&mut self, what: &str, min_element_size: usize) -> crate::Result<usize> {
        let n = self.u32(what)? as usize;
        let remaining = self.data.len() - self.offset;
        if n > remaining / min_element_size {
            return invalid(format!(
                "{what} {n} exceeds what the remaining {remaining} bytes can hold"
            ));
        }
        Ok(n)
    }
}

#[cfg(test)]
#[path = "reflection_probes_tests.rs"]
mod tests;
