// ReflectionProbes (id 54) encode and untrusted decode.
// See: context/lib/build_pipeline.md §PRL section IDs

use super::{
    BindingList, MAX_REFLECTION_PROBES, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    REFLECTION_PROBE_LOBE_COUNT, REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBES_VERSION,
    ReflectionProbeFluidBinding, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbeWorldBinding, ReflectionProbesError, ReflectionProbesSection,
    reflection_probe_colour_chain_bytes, reflection_probe_colour_mips_valid,
    reflection_probe_distance_tile_bytes, reflection_probe_tile_bytes,
};

type Result<T> = std::result::Result<T, ReflectionProbesError>;

/// Largest tile edge a decoder accepts; past it a corrupt header would ask
/// for an absurd texture before the payload length check could catch it.
const MAX_TILE_DIMENSION: u32 = 1024;

const HEADER_SIZE: usize = 2 + 4 * 10;
const SHARE_ENTRY_SIZE: usize = 4 + 12 + REFLECTION_PROBE_LOBE_COUNT * 12;
const PROBE_SIZE: usize = 4 + 12 * 3 + 4 + REFLECTION_PROBE_LIGHTS_PER_PROBE * SHARE_ENTRY_SIZE;
const FLUID_BINDING_SIZE: usize = 12;
const WORLD_BINDING_SIZE: usize = 8;

impl ReflectionProbesSection {
    pub fn byte_len(&self) -> usize {
        HEADER_SIZE
            + self.probes.len() * PROBE_SIZE
            + self.fluid_bindings.len() * FLUID_BINDING_SIZE
            + self.world_bindings.len() * WORLD_BINDING_SIZE
            + self.tiles.len()
            + self.distances.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&REFLECTION_PROBES_VERSION.to_le_bytes());
        u32_out(&mut out, self.tile_dimension);
        u32_out(&mut out, self.tile_border);
        u32_out(&mut out, self.atlas_format);
        u32_out(&mut out, self.colour_mip_count);
        u32_out(&mut out, self.distance_format);
        count(&mut out, REFLECTION_PROBE_LIGHTS_PER_PROBE);
        count(&mut out, REFLECTION_PROBE_LOBE_COUNT);
        count(&mut out, self.probes.len());
        count(&mut out, self.fluid_bindings.len());
        count(&mut out, self.world_bindings.len());
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
        for binding in &self.fluid_bindings {
            u32_out(&mut out, binding.volume);
            u32_out(&mut out, binding.face);
            u32_out(&mut out, binding.probe);
        }
        for binding in &self.world_bindings {
            u32_out(&mut out, binding.face);
            u32_out(&mut out, binding.probe);
        }
        out.extend_from_slice(&self.tiles);
        out.extend_from_slice(&self.distances);
        out
    }

    /// Decode untrusted section bytes. Refused, each with a named
    /// [`ReflectionProbesError`]:
    /// - another version (`VersionMismatch`);
    /// - a header off the engine's contract: a tile dimension that is not a
    ///   multiple of 4 in 4..=`MAX_TILE_DIMENSION`, a border other than 1, an
    ///   unknown atlas or distance format, or another K or lobe count;
    /// - a colour mip count of 0, or one whose mips stop being whole 4×4
    ///   blocks or fall under 4² (`ColourMipCount`);
    /// - a probe count of 0, or over the layer cap;
    /// - a probe whose layer is past the probe count or reused, whose box min
    ///   exceeds its max, or whose capture point lies outside its box;
    /// - a non-finite box, capture point, reference or share, a negative
    ///   reference, or a share outside 0..=1;
    /// - an unused share entry carrying values, or two used entries in one
    ///   probe naming the same roster index (the light's change would count
    ///   twice in the ratio);
    /// - a binding past the probe count (`BindingProbePastCount`), a list out
    ///   of order (`FluidBindingOrder`, `WorldBindingOrder`), or a face bound
    ///   twice (`FluidFaceBoundTwice`, `WorldFaceBoundTwice`);
    /// - a tile or distance payload of the wrong length, or trailing bytes;
    /// - a negative or non-finite distance texel. −0.0 is refused too, by
    ///   choice: the compiler never writes it, and refusing it keeps one bit
    ///   pattern for "no surface".
    ///
    /// Every count is bounded by the bytes left before anything is allocated.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut r = Reader { data, offset: 0 };
        let version = r.u16("version")?;
        if version != REFLECTION_PROBES_VERSION {
            return Err(ReflectionProbesError::VersionMismatch {
                version,
                expected: REFLECTION_PROBES_VERSION,
            });
        }
        let tile_dimension = r.u32("tile dimension")?;
        let tile_border = r.u32("tile border")?;
        let atlas_format = r.u32("atlas format")?;
        let colour_mip_count = r.u32("colour mip count")?;
        let distance_format = r.u32("distance format")?;
        let lights_per_probe = r.u32("lights per probe")? as usize;
        let lobe_count = r.u32("lobe count")? as usize;
        if tile_dimension < 4
            || !tile_dimension.is_multiple_of(4)
            || tile_dimension > MAX_TILE_DIMENSION
        {
            return invalid(format!(
                "tile dimension {tile_dimension} must be a multiple of 4 in 4..={MAX_TILE_DIMENSION}"
            ));
        }
        if tile_border != REFLECTION_PROBE_TILE_BORDER {
            return invalid(format!(
                "tile border {tile_border}; the octahedral convention is {REFLECTION_PROBE_TILE_BORDER}"
            ));
        }
        if reflection_probe_tile_bytes(atlas_format, tile_dimension).is_none() {
            return invalid(format!("unknown atlas format {atlas_format}"));
        }
        if !reflection_probe_colour_mips_valid(tile_dimension, colour_mip_count) {
            return Err(ReflectionProbesError::ColourMipCount {
                count: colour_mip_count,
                tile_dimension,
            });
        }
        let chain_bytes =
            reflection_probe_colour_chain_bytes(atlas_format, tile_dimension, colour_mip_count)
                .expect("the format and the chain were checked above");
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
        let fluid_binding_count = r.u32("fluid binding count")? as usize;
        let world_binding_count = r.u32("world binding count")? as usize;
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

        let fluid_bindings = read_fluid_bindings(&mut r, fluid_binding_count, probe_count)?;
        let world_bindings = read_world_bindings(&mut r, world_binding_count, probe_count)?;

        let tiles = r.take(probe_count * chain_bytes, "tile payload")?.to_vec();
        let distances = r
            .take(probe_count * distance_tile_bytes, "distance payload")?
            .to_vec();
        let (texels, _) = distances.as_chunks::<2>();
        if let Some(texel) = texels
            .iter()
            .position(|bits| !f16_bits_are_finite_non_negative(u16::from_le_bytes(*bits)))
        {
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
            colour_mip_count,
            probes,
            fluid_bindings,
            world_bindings,
            tiles,
            distance_format,
            distances,
        })
    }
}

fn read_fluid_bindings(
    r: &mut Reader<'_>,
    count: usize,
    probe_count: usize,
) -> Result<Vec<ReflectionProbeFluidBinding>> {
    r.check_count("fluid binding count", count, FLUID_BINDING_SIZE)?;
    let mut bindings: Vec<ReflectionProbeFluidBinding> = Vec::with_capacity(count);
    for index in 0..count {
        let binding = ReflectionProbeFluidBinding {
            volume: r.u32(&format!("fluid binding {index} volume"))?,
            face: r.u32(&format!("fluid binding {index} face"))?,
            probe: r.u32(&format!("fluid binding {index} probe"))?,
        };
        check_probe(BindingList::Fluid, index, binding.probe, probe_count)?;
        if let Some(previous) = bindings.last() {
            let (volume, face) = (binding.volume, binding.face);
            if (volume, face) == (previous.volume, previous.face) {
                return Err(ReflectionProbesError::FluidFaceBoundTwice { volume, face });
            }
            if (volume, face) < (previous.volume, previous.face) {
                return Err(ReflectionProbesError::FluidBindingOrder {
                    index,
                    volume,
                    face,
                });
            }
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

fn read_world_bindings(
    r: &mut Reader<'_>,
    count: usize,
    probe_count: usize,
) -> Result<Vec<ReflectionProbeWorldBinding>> {
    r.check_count("world binding count", count, WORLD_BINDING_SIZE)?;
    let mut bindings: Vec<ReflectionProbeWorldBinding> = Vec::with_capacity(count);
    for index in 0..count {
        let binding = ReflectionProbeWorldBinding {
            face: r.u32(&format!("world binding {index} face"))?,
            probe: r.u32(&format!("world binding {index} probe"))?,
        };
        check_probe(BindingList::World, index, binding.probe, probe_count)?;
        if let Some(previous) = bindings.last() {
            let face = binding.face;
            if face == previous.face {
                return Err(ReflectionProbesError::WorldFaceBoundTwice { face });
            }
            if face < previous.face {
                return Err(ReflectionProbesError::WorldBindingOrder { index, face });
            }
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

fn check_probe(list: BindingList, index: usize, probe: u32, probe_count: usize) -> Result<()> {
    if probe as usize >= probe_count {
        return Err(ReflectionProbesError::BindingProbePastCount {
            list,
            index,
            probe,
            probe_count,
        });
    }
    Ok(())
}

/// Sign bit clear and exponent not all ones: a finite f16 at or above zero.
fn f16_bits_are_finite_non_negative(bits: u16) -> bool {
    bits & 0x8000 == 0 && bits & 0x7C00 != 0x7C00
}

fn read_probe(r: &mut Reader<'_>, p: usize) -> Result<ReflectionProbeRecord> {
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
    for (k, light) in lights
        .iter()
        .enumerate()
        .filter(|(_, light)| light.is_used())
    {
        if let Some(first) = lights[..k]
            .iter()
            .position(|earlier| earlier.roster_index == light.roster_index)
        {
            return invalid(format!(
                "{what} lights {first} and {k} both name animated light {}",
                light.roster_index
            ));
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

fn invalid<T>(message: String) -> Result<T> {
    Err(ReflectionProbesError::Invalid(message))
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
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| ReflectionProbesError::Truncated {
                what: what.to_string(),
            })?;
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u16(&mut self, what: &str) -> Result<u16> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, what: &str) -> Result<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn finite_f32(&mut self, what: &str) -> Result<f32> {
        let value = f32::from_bits(self.u32(what)?);
        if !value.is_finite() {
            return invalid(format!("{what} is not finite"));
        }
        Ok(value)
    }

    fn finite_vec3(&mut self, what: &str) -> Result<[f32; 3]> {
        Ok([
            self.finite_f32(what)?,
            self.finite_f32(what)?,
            self.finite_f32(what)?,
        ])
    }

    /// Refuse a count whose elements, each at least `min_element_size`
    /// bytes, the remaining bytes cannot hold, before anything is allocated.
    fn check_count(&self, what: &str, n: usize, min_element_size: usize) -> Result<()> {
        let remaining = self.data.len() - self.offset;
        if n > remaining / min_element_size {
            return invalid(format!(
                "{what} {n} exceeds what the remaining {remaining} bytes can hold"
            ));
        }
        Ok(())
    }

    fn bounded_count(&mut self, what: &str, min_element_size: usize) -> Result<usize> {
        let n = self.u32(what)? as usize;
        self.check_count(what, n, min_element_size)?;
        Ok(n)
    }
}
