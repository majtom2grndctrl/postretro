// Emissive PRL section (ID 60): per-texture glow strength and mean glow colour,
// and emissive volumes over static world faces.
// See: context/lib/build_pipeline.md §PRL section IDs · resource_management.md §4.5

use std::collections::HashSet;

use crate::lightmap::{eof, invalid};

/// Section-internal format epoch; any other version is refused as stale.
pub const EMISSIVE_VERSION: u16 = 1;

/// Most emissive volumes one level may carry. The compiler fails a map past it.
pub const MAX_EMISSIVE_VOLUMES: usize = 256;

/// Largest value any glow multiplier may take: a mod's per-texture strength and
/// a volume's min and max keys alike. A texture at the ceiling
/// inside a volume at the ceiling glows 64 × 64 = 4096, so bloom's six-level
/// pyramid sum stays under `Rgba16Float`'s 65504.
pub const EMISSIVE_VALUE_CEILING: f32 = 64.0;

/// One texture's baked glow, index-aligned with TextureNames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissiveTexture {
    /// The multiplier on the texture's `_e` colour: the mod's material file's
    /// value, else 1 with an `_e` map and 0 without.
    pub strength: f32,
    /// Linear mean of the non-black `_e` texels at full resolution; 0 for a
    /// texture without `_e` or with an all-black one.
    pub glow_color: [f32; 3],
}

impl EmissiveTexture {
    /// A texture that does not glow.
    pub const DARK: Self = Self {
        strength: 0.0,
        glow_color: [0.0; 3],
    };
}

/// One `emissive_volume` brush entity, in map order.
#[derive(Debug, Clone, PartialEq)]
pub struct EmissiveVolumeRecord {
    /// Multiplier range scripted brightness maps across, after the
    /// compiler's per-key fixes and swap: `min <= max`.
    pub min: f32,
    pub max: f32,
    /// Linear tint, each channel in `[0, 1]`.
    pub color: [f32; 3],
    /// Centre of the union AABB of the entity's brushes; the co-op binding
    /// check.
    pub origin: [f32; 3],
    /// Geometry face indices, strictly ascending; each owned by one volume.
    pub faces: Vec<u32>,
    /// Author-supplied script tags (FGD `_tags`, pre-split on whitespace).
    pub tags: Vec<String>,
}

/// Emissive PRL section. Every map emits it; a level without it is stale.
///
/// On-disk layout (little-endian):
///   u16  version (EMISSIVE_VERSION)
///   u32  texture_count (= TextureNames count)
///   repeat texture_count:
///     f32  strength
///     f32  glow_r, glow_g, glow_b (linear)
///   u32  volume_count (<= MAX_EMISSIVE_VOLUMES)
///   repeat volume_count:
///     f32  min, max
///     f32  color_r, color_g, color_b (linear)
///     f32  origin_x, origin_y, origin_z
///     u32  face_count; u32[face_count] ascending Geometry face indices
///     u32  tag_count; repeat: u32 tag_byte_len; u8[] tag_utf8
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmissiveSection {
    pub textures: Vec<EmissiveTexture>,
    pub volumes: Vec<EmissiveVolumeRecord>,
}

const TEXTURE_RECORD_BYTES: usize = 16;
/// min, max, colour, origin, face count, tag count.
const MIN_VOLUME_RECORD_BYTES: usize = 4 * 8 + 4 + 4;

impl EmissiveSection {
    /// `texture_count` textures that do not glow and no volume: what a
    /// writer with no material stage packs.
    pub fn dark(texture_count: usize) -> Self {
        Self {
            textures: vec![EmissiveTexture::DARK; texture_count],
            volumes: Vec::new(),
        }
    }

    /// The baked strength of texture `index`; 0 past the table, as for a
    /// face with no texture.
    pub fn texture_strength(&self, index: usize) -> f32 {
        self.textures
            .get(index)
            .map_or(0.0, |texture| texture.strength)
    }

    pub fn byte_len(&self) -> usize {
        2 + 4
            + self.textures.len() * TEXTURE_RECORD_BYTES
            + 4
            + self
                .volumes
                .iter()
                .map(|volume| {
                    MIN_VOLUME_RECORD_BYTES
                        + volume.faces.len() * 4
                        + volume.tags.iter().map(|tag| 4 + tag.len()).sum::<usize>()
                })
                .sum::<usize>()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.byte_len());
        buf.extend_from_slice(&EMISSIVE_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.textures.len() as u32).to_le_bytes());
        for texture in &self.textures {
            buf.extend_from_slice(&texture.strength.to_le_bytes());
            for channel in texture.glow_color {
                buf.extend_from_slice(&channel.to_le_bytes());
            }
        }
        buf.extend_from_slice(&(self.volumes.len() as u32).to_le_bytes());
        for volume in &self.volumes {
            buf.extend_from_slice(&volume.min.to_le_bytes());
            buf.extend_from_slice(&volume.max.to_le_bytes());
            for value in volume.color.iter().chain(&volume.origin) {
                buf.extend_from_slice(&value.to_le_bytes());
            }
            buf.extend_from_slice(&(volume.faces.len() as u32).to_le_bytes());
            for face in &volume.faces {
                buf.extend_from_slice(&face.to_le_bytes());
            }
            buf.extend_from_slice(&(volume.tags.len() as u32).to_le_bytes());
            for tag in &volume.tags {
                buf.extend_from_slice(&(tag.len() as u32).to_le_bytes());
                buf.extend_from_slice(tag.as_bytes());
            }
        }
        buf
    }

    /// Structural decode plus every value check that needs no other section:
    /// version, the volume cap, value ranges the compiler never writes, and
    /// face lists that ascend and are owned once. Counts against TextureNames
    /// and Geometry: [`Self::validate_against_level`].
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut reader = Reader { data, at: 0 };
        let version = reader.u16("version")?;
        if version != EMISSIVE_VERSION {
            return Err(invalid(format!(
                "Emissive version {version} is unknown; this build reads version \
                 {EMISSIVE_VERSION}; re-bake the level"
            )));
        }
        let texture_count = reader.count("texture count", TEXTURE_RECORD_BYTES)?;
        let mut textures = Vec::with_capacity(texture_count);
        for index in 0..texture_count {
            let strength = reader.f32(&format!("texture {index} strength"))?;
            check_multiplier(strength, &format!("texture {index} strength"))?;
            let glow_color = reader.vec3(&format!("texture {index} glow colour"))?;
            check_color(glow_color, &format!("texture {index} glow colour"))?;
            textures.push(EmissiveTexture {
                strength,
                glow_color,
            });
        }

        let volume_count = reader.u32("volume count")? as usize;
        if volume_count > MAX_EMISSIVE_VOLUMES {
            return Err(invalid(format!(
                "Emissive volume count {volume_count} exceeds the {MAX_EMISSIVE_VOLUMES}-volume cap"
            )));
        }
        reader.fits(volume_count, MIN_VOLUME_RECORD_BYTES, "volume count")?;
        let mut owned = HashSet::new();
        let mut volumes = Vec::with_capacity(volume_count);
        for index in 0..volume_count {
            let min = reader.f32(&format!("volume {index} min"))?;
            check_multiplier(min, &format!("volume {index} min"))?;
            let max = reader.f32(&format!("volume {index} max"))?;
            check_multiplier(max, &format!("volume {index} max"))?;
            if min > max {
                return Err(invalid(format!(
                    "Emissive volume {index} min {min} is above its max {max}"
                )));
            }
            let color = reader.vec3(&format!("volume {index} colour"))?;
            check_color(color, &format!("volume {index} colour"))?;
            let origin = reader.vec3(&format!("volume {index} origin"))?;
            if origin.iter().any(|value| !value.is_finite()) {
                return Err(invalid(format!(
                    "Emissive volume {index} origin {origin:?} is not finite"
                )));
            }
            let face_count = reader.count(&format!("volume {index} face count"), 4)?;
            let mut faces = Vec::with_capacity(face_count);
            for _ in 0..face_count {
                let face = reader.u32(&format!("volume {index} face"))?;
                if let Some(&previous) = faces.last()
                    && face <= previous
                {
                    return Err(invalid(format!(
                        "Emissive volume {index} face {face} follows {previous}; faces must ascend \
                         without repeats"
                    )));
                }
                if !owned.insert(face) {
                    return Err(invalid(format!(
                        "Emissive volume {index} face {face} is already owned by an earlier volume"
                    )));
                }
                faces.push(face);
            }
            let tag_count = reader.count(&format!("volume {index} tag count"), 4)?;
            let mut tags = Vec::with_capacity(tag_count);
            for tag in 0..tag_count {
                tags.push(reader.string(&format!("volume {index} tag {tag}"))?);
            }
            volumes.push(EmissiveVolumeRecord {
                min,
                max,
                color,
                origin,
                faces,
                tags,
            });
        }
        if reader.at != data.len() {
            return Err(invalid(format!(
                "Emissive section has {} trailing bytes",
                data.len() - reader.at
            )));
        }
        Ok(Self { textures, volumes })
    }

    /// One texture record per TextureNames entry, and every member face a
    /// Geometry face.
    pub fn validate_against_level(
        &self,
        texture_name_count: usize,
        face_count: usize,
    ) -> crate::Result<()> {
        if self.textures.len() != texture_name_count {
            return Err(invalid(format!(
                "Emissive carries {} texture records for {texture_name_count} texture names",
                self.textures.len()
            )));
        }
        for (index, volume) in self.volumes.iter().enumerate() {
            // Faces ascend, so the last is the largest.
            if let Some(&face) = volume.faces.last()
                && face as usize >= face_count
            {
                return Err(invalid(format!(
                    "Emissive volume {index} face {face} is outside the level's {face_count} faces"
                )));
            }
        }
        Ok(())
    }
}

/// A strength or volume multiplier: finite, not negative, at most the ceiling.
fn check_multiplier(value: f32, what: &str) -> crate::Result<()> {
    if !value.is_finite() || value < 0.0 || value > EMISSIVE_VALUE_CEILING {
        return Err(invalid(format!(
            "Emissive {what} {value} is outside [0, {EMISSIVE_VALUE_CEILING}]"
        )));
    }
    Ok(())
}

/// A linear colour: every channel finite and in `[0, 1]`.
fn check_color(color: [f32; 3], what: &str) -> crate::Result<()> {
    if color
        .iter()
        .any(|channel| !channel.is_finite() || !(0.0..=1.0).contains(channel))
    {
        return Err(invalid(format!(
            "Emissive {what} {color:?} has a channel outside [0, 1]"
        )));
    }
    Ok(())
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self, what: &str) -> crate::Result<[u8; N]> {
        let bytes = self
            .data
            .get(self.at..self.at + N)
            .ok_or_else(|| eof(format!("Emissive section truncated at {what}")))?;
        self.at += N;
        Ok(bytes.try_into().expect("slice of N bytes"))
    }

    fn u16(&mut self, what: &str) -> crate::Result<u16> {
        self.bytes(what).map(u16::from_le_bytes)
    }

    fn u32(&mut self, what: &str) -> crate::Result<u32> {
        self.bytes(what).map(u32::from_le_bytes)
    }

    fn f32(&mut self, what: &str) -> crate::Result<f32> {
        self.bytes(what).map(f32::from_le_bytes)
    }

    fn vec3(&mut self, what: &str) -> crate::Result<[f32; 3]> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    /// A count whose records take at least `record_bytes` each, refused
    /// before allocating when the rest of the section cannot hold them.
    fn count(&mut self, what: &str, record_bytes: usize) -> crate::Result<usize> {
        let count = self.u32(what)? as usize;
        self.fits(count, record_bytes, what)?;
        Ok(count)
    }

    fn fits(&self, count: usize, record_bytes: usize, what: &str) -> crate::Result<()> {
        let remaining = self.data.len() - self.at;
        if count > remaining / record_bytes {
            return Err(eof(format!(
                "Emissive {what} {count} exceeds what the remaining {remaining} bytes can hold"
            )));
        }
        Ok(())
    }

    fn string(&mut self, what: &str) -> crate::Result<String> {
        let len = self.u32(&format!("{what} length"))? as usize;
        let bytes = self
            .data
            .get(self.at..self.at + len)
            .ok_or_else(|| eof(format!("Emissive section truncated in {what}")))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| invalid(format!("Emissive {what} is not UTF-8")))?;
        self.at += len;
        Ok(text.to_owned())
    }
}

#[cfg(test)]
#[path = "emissive_tests.rs"]
mod tests;
