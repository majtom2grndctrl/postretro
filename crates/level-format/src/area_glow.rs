// Area glow PRL section (ID 62): the emitting faces of each `light_area` and
// the glow they draw, which follows their light's compose descriptor.
// See: context/lib/build_pipeline.md §PRL section IDs · resource_management.md §4.5

use std::collections::HashSet;

use crate::emissive::{EMISSIVE_VALUE_CEILING, MAX_EMISSIVE_VOLUMES};
use crate::lightmap::{eof, invalid};

/// Section-internal format epoch; any other version is refused.
pub const AREA_GLOW_VERSION: u16 = 1;

/// Entries in the forward pass's glow table: the 64 KiB uniform binding limit
/// at one `vec4<f32>` each. Volume entries come first, area entries after.
pub const GLOW_TABLE_ENTRIES: usize = 4096;

/// The first table entry an area face takes: entry 0 is "no volume" and
/// entries `1..=MAX_EMISSIVE_VOLUMES` are the volumes, so a per-vertex id at
/// or past this value names an area entry.
pub const AREA_GLOW_FIRST_ENTRY: usize = MAX_EMISSIVE_VOLUMES + 1;

/// Most emitting faces one level may carry: one table entry each. The
/// compiler fails a map past it; the loader refuses a section past it.
pub const MAX_AREA_GLOW_FACES: usize = GLOW_TABLE_ENTRIES - AREA_GLOW_FIRST_ENTRY;

/// The on-disk value of a light that has no compose descriptor slot: its
/// faces draw the static glow, `glow × color`, on every frame.
const NO_COMPOSE_SLOT: u32 = u32::MAX;

/// One area light's glow and the Geometry faces that draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaGlowLight {
    /// `_glow`, already within `[0, EMISSIVE_VALUE_CEILING]`.
    pub glow: f32,
    /// The light's authored `_color`, linear, as its compose descriptor's
    /// base colour carries it.
    pub color: [f32; 3],
    /// The light's intensity (`light` over the Quake reference), the factor
    /// its compose descriptor's base colour also carries; the runtime divides
    /// it out so the glow never scales with `light`.
    pub intensity: f32,
    /// The light's compose descriptor slot (AnimatedBakedLights order), or
    /// `None` for a static light, or one whose intensity is 0, whose faces
    /// draw `glow × color` unchanged.
    pub compose_slot: Option<u32>,
    /// Geometry face indices, strictly ascending; each owned by one light.
    pub faces: Vec<u32>,
}

/// Area glow PRL section. Written only when the map has an area light; a
/// level without it draws no area glow.
///
/// On-disk layout (little-endian):
///   u16  version (AREA_GLOW_VERSION)
///   u32  light_count
///   repeat light_count:
///     f32  glow
///     f32  color_r, color_g, color_b (linear)
///     f32  intensity
///     u32  compose_slot (u32::MAX = none)
///     u32  face_count; u32[face_count] ascending Geometry face indices
///
/// The faces of every light together are at most [`MAX_AREA_GLOW_FACES`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AreaGlowSection {
    pub lights: Vec<AreaGlowLight>,
}

/// glow, colour, intensity, slot, face count.
const MIN_LIGHT_RECORD_BYTES: usize = 4 * 5 + 4 + 4;

impl AreaGlowSection {
    /// Every emitting face, in table order: lights in section order, each
    /// light's faces ascending. Entry `k` is table entry
    /// `AREA_GLOW_FIRST_ENTRY + k`.
    pub fn entries(&self) -> impl Iterator<Item = (u32, &AreaGlowLight)> + '_ {
        self.lights
            .iter()
            .flat_map(|light| light.faces.iter().map(move |&face| (face, light)))
    }

    /// Emitting faces across every light.
    pub fn face_count(&self) -> usize {
        self.lights.iter().map(|light| light.faces.len()).sum()
    }

    pub fn byte_len(&self) -> usize {
        2 + 4
            + self
                .lights
                .iter()
                .map(|light| MIN_LIGHT_RECORD_BYTES + light.faces.len() * 4)
                .sum::<usize>()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.byte_len());
        buf.extend_from_slice(&AREA_GLOW_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.lights.len() as u32).to_le_bytes());
        for light in &self.lights {
            buf.extend_from_slice(&light.glow.to_le_bytes());
            for channel in light.color {
                buf.extend_from_slice(&channel.to_le_bytes());
            }
            buf.extend_from_slice(&light.intensity.to_le_bytes());
            let slot = light.compose_slot.unwrap_or(NO_COMPOSE_SLOT);
            buf.extend_from_slice(&slot.to_le_bytes());
            buf.extend_from_slice(&(light.faces.len() as u32).to_le_bytes());
            for face in &light.faces {
                buf.extend_from_slice(&face.to_le_bytes());
            }
        }
        buf
    }

    /// Structural decode plus every value check that needs no other section:
    /// version, the face cap, value ranges the compiler never writes, and
    /// face lists that ascend and are owned once. Faces against Geometry and
    /// id 60's volumes: [`Self::validate_against_level`].
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut at = 0usize;
        let mut take = |n: usize, what: &str| -> crate::Result<&[u8]> {
            let bytes = data
                .get(at..at + n)
                .ok_or_else(|| eof(format!("AreaGlow section truncated at {what}")))?;
            at += n;
            Ok(bytes)
        };
        let u16_at = |bytes: &[u8]| u16::from_le_bytes(bytes.try_into().expect("two bytes"));
        let u32_at = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().expect("four bytes"));
        let f32_at = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().expect("four bytes"));

        let version = u16_at(take(2, "version")?);
        if version != AREA_GLOW_VERSION {
            return Err(invalid(format!(
                "AreaGlow version {version} is unknown; this build reads version \
                 {AREA_GLOW_VERSION}; re-bake the level"
            )));
        }
        let light_count = u32_at(take(4, "light count")?) as usize;
        let remaining = data.len().saturating_sub(6);
        if light_count > remaining / MIN_LIGHT_RECORD_BYTES {
            return Err(eof(format!(
                "AreaGlow light count {light_count} exceeds what the remaining {remaining} bytes \
                 can hold"
            )));
        }
        let mut owned = HashSet::new();
        let mut total_faces = 0usize;
        let mut lights = Vec::with_capacity(light_count);
        for index in 0..light_count {
            let glow = f32_at(take(4, "glow")?);
            if !glow.is_finite() || !(0.0..=EMISSIVE_VALUE_CEILING).contains(&glow) {
                return Err(invalid(format!(
                    "AreaGlow light {index} glow {glow} is outside [0, {EMISSIVE_VALUE_CEILING}]"
                )));
            }
            let mut color = [0.0f32; 3];
            for channel in &mut color {
                *channel = f32_at(take(4, "colour")?);
            }
            if color.iter().any(|c| !c.is_finite() || *c < 0.0) {
                return Err(invalid(format!(
                    "AreaGlow light {index} colour {color:?} has a negative or non-finite channel"
                )));
            }
            let intensity = f32_at(take(4, "intensity")?);
            if !intensity.is_finite() || intensity < 0.0 {
                return Err(invalid(format!(
                    "AreaGlow light {index} intensity {intensity} is negative or not finite"
                )));
            }
            let slot = u32_at(take(4, "compose slot")?);
            let compose_slot = (slot != NO_COMPOSE_SLOT).then_some(slot);
            let face_count = u32_at(take(4, "face count")?) as usize;
            total_faces = total_faces.saturating_add(face_count);
            if total_faces > MAX_AREA_GLOW_FACES {
                return Err(invalid(format!(
                    "AreaGlow holds more than {MAX_AREA_GLOW_FACES} emitting faces, the glow \
                     table's cap"
                )));
            }
            let mut faces = Vec::with_capacity(face_count.min(MAX_AREA_GLOW_FACES));
            for _ in 0..face_count {
                let face = u32_at(take(4, "face")?);
                if let Some(&previous) = faces.last()
                    && face <= previous
                {
                    return Err(invalid(format!(
                        "AreaGlow light {index} face {face} follows {previous}; faces must \
                         ascend without repeats"
                    )));
                }
                if !owned.insert(face) {
                    return Err(invalid(format!(
                        "AreaGlow light {index} face {face} is already owned by an earlier light"
                    )));
                }
                faces.push(face);
            }
            lights.push(AreaGlowLight {
                glow,
                color,
                intensity,
                compose_slot,
                faces,
            });
        }
        if at != data.len() {
            return Err(invalid(format!(
                "AreaGlow section has {} trailing bytes",
                data.len() - at
            )));
        }
        Ok(Self { lights })
    }

    /// Every face a Geometry face, and none also held by an emissive volume:
    /// a face has one glow owner.
    pub fn validate_against_level(
        &self,
        face_count: usize,
        emissive: &crate::emissive::EmissiveSection,
    ) -> crate::Result<()> {
        for (index, light) in self.lights.iter().enumerate() {
            if let Some(&face) = light.faces.last()
                && face as usize >= face_count
            {
                return Err(invalid(format!(
                    "AreaGlow light {index} face {face} is outside the level's {face_count} faces"
                )));
            }
        }
        if emissive
            .volumes
            .iter()
            .any(|volume| !volume.faces.is_empty())
        {
            let area: HashSet<u32> = self.entries().map(|(face, _)| face).collect();
            for (volume_index, volume) in emissive.volumes.iter().enumerate() {
                if let Some(face) = volume.faces.iter().find(|face| area.contains(face)) {
                    return Err(invalid(format!(
                        "AreaGlow face {face} is also emissive volume {volume_index}'s; an area \
                         light's face glows with its light alone"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emissive::{EmissiveSection, EmissiveVolumeRecord};

    fn light(compose_slot: Option<u32>, faces: Vec<u32>) -> AreaGlowLight {
        AreaGlowLight {
            glow: 4.0,
            color: [1.0, 0.25, 0.0],
            intensity: 1.5,
            compose_slot,
            faces,
        }
    }

    fn message(error: crate::FormatError) -> String {
        error.to_string()
    }

    #[test]
    fn round_trips_static_and_slotted_lights_in_table_order() {
        let section = AreaGlowSection {
            lights: vec![light(None, vec![3, 9]), light(Some(2), vec![4])],
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), section.byte_len());
        let decoded = AreaGlowSection::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, section);
        let entries: Vec<(u32, Option<u32>)> = decoded
            .entries()
            .map(|(face, light)| (face, light.compose_slot))
            .collect();
        assert_eq!(entries, [(3, None), (9, None), (4, Some(2))]);
        assert_eq!(decoded.face_count(), 3);
    }

    #[test]
    fn the_cap_fills_the_table_after_the_volume_entries() {
        assert_eq!(AREA_GLOW_FIRST_ENTRY, 257);
        assert_eq!(MAX_AREA_GLOW_FACES, 3839);
        assert_eq!(GLOW_TABLE_ENTRIES * 16, 64 * 1024);
    }

    // Acceptance row 60 (loader half): a table past the cap is refused, one
    // at the cap decodes.
    #[test]
    fn a_section_past_the_face_cap_is_refused() {
        let at_cap = AreaGlowSection {
            lights: vec![light(None, (0..MAX_AREA_GLOW_FACES as u32).collect())],
        };
        AreaGlowSection::from_bytes(&at_cap.to_bytes()).expect("the cap itself decodes");
        let over = AreaGlowSection {
            lights: vec![
                light(None, (0..MAX_AREA_GLOW_FACES as u32).collect()),
                light(Some(0), vec![MAX_AREA_GLOW_FACES as u32]),
            ],
        };
        let text = message(AreaGlowSection::from_bytes(&over.to_bytes()).unwrap_err());
        assert!(text.contains("more than 3839 emitting faces"), "{text}");
    }

    #[test]
    fn values_the_compiler_never_writes_are_refused() {
        let cases: [(AreaGlowSection, &str); 5] = [
            (
                AreaGlowSection {
                    lights: vec![AreaGlowLight {
                        glow: 64.5,
                        ..light(None, vec![0])
                    }],
                },
                "glow 64.5",
            ),
            (
                AreaGlowSection {
                    lights: vec![AreaGlowLight {
                        intensity: -1.0,
                        ..light(None, vec![0])
                    }],
                },
                "intensity -1",
            ),
            (
                AreaGlowSection {
                    lights: vec![light(None, vec![2, 1])],
                },
                "must ascend",
            ),
            (
                AreaGlowSection {
                    lights: vec![light(None, vec![1]), light(None, vec![1])],
                },
                "already owned",
            ),
            (
                AreaGlowSection {
                    lights: vec![AreaGlowLight {
                        color: [f32::NAN, 0.0, 0.0],
                        ..light(None, vec![0])
                    }],
                },
                "colour",
            ),
        ];
        for (section, expected) in cases {
            let text = message(AreaGlowSection::from_bytes(&section.to_bytes()).unwrap_err());
            assert!(text.contains(expected), "{expected}: {text}");
        }
        let mut bytes = AreaGlowSection::default().to_bytes();
        bytes[0] = 9;
        assert!(message(AreaGlowSection::from_bytes(&bytes).unwrap_err()).contains("version 9"));
        let mut trailing = AreaGlowSection::default().to_bytes();
        trailing.push(0);
        assert!(message(AreaGlowSection::from_bytes(&trailing).unwrap_err()).contains("trailing"));
    }

    #[test]
    fn faces_must_fit_the_level_and_never_join_a_volume() {
        let section = AreaGlowSection {
            lights: vec![light(Some(0), vec![1, 5])],
        };
        let dark = EmissiveSection::dark(0);
        section.validate_against_level(6, &dark).unwrap();
        let text = message(section.validate_against_level(5, &dark).unwrap_err());
        assert!(
            text.contains("face 5 is outside the level's 5 faces"),
            "{text}"
        );

        let volume = EmissiveVolumeRecord {
            min: 0.0,
            max: 1.0,
            color: [1.0; 3],
            origin: [0.0; 3],
            faces: vec![5],
            tags: Vec::new(),
        };
        let held = EmissiveSection {
            textures: Vec::new(),
            volumes: vec![volume],
        };
        let text = message(section.validate_against_level(6, &held).unwrap_err());
        assert!(
            text.contains("face 5 is also emissive volume 0's"),
            "{text}"
        );
    }
}
