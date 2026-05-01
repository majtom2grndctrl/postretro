// MapEntity PRL section (ID 29): non-light, non-worldspawn map entities for
// runtime classname dispatch.
// See: context/lib/build_pipeline.md §PRL Compilation

use crate::FormatError;

/// One map-authored entity carried through to the runtime classname dispatch.
///
/// `classname` selects the engine handler. `origin` is in engine space (Y-up,
/// meters); `angles` is engine-convention Euler (radians, pitch/yaw/roll). The
/// compiler converts source-format angles before serialization, so the runtime
/// never sees Quake convention.
///
/// `key_values` is the residual KVP bag with reserved keys stripped
/// (`classname`, `origin`, `_tags`, `angle`, `angles`, `mangle`). `tags` is the
/// pre-split `_tags` list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapEntityRecord {
    pub classname: String,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub key_values: Vec<(String, String)>,
    pub tags: Vec<String>,
}

/// On-disk layout (little-endian):
///   u32  entry_count
///   repeat entry_count:
///     u32  classname_byte_len; u8[] classname_utf8
///     f32  origin_x; f32 origin_y; f32 origin_z
///     f32  angles_pitch; f32 angles_yaw; f32 angles_roll
///     u32  kvp_count
///     repeat kvp_count:
///       u32  key_byte_len;   u8[] key_utf8
///       u32  value_byte_len; u8[] value_utf8
///     u32  tag_count
///     repeat tag_count:
///       u32  tag_byte_len;   u8[] tag_utf8
///
/// Empty `key_values` / `tags` lists serialize as `u32(0)`. Strings carry no
/// null terminator. Mirrors `TextureNamesSection` / `LightTagsSection` framing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapEntitySection {
    pub entries: Vec<MapEntityRecord>,
}

impl MapEntitySection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            write_string(&mut buf, &e.classname);
            for c in e.origin {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            for c in e.angles {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            buf.extend_from_slice(&(e.key_values.len() as u32).to_le_bytes());
            for (k, v) in &e.key_values {
                write_string(&mut buf, k);
                write_string(&mut buf, v);
            }
            buf.extend_from_slice(&(e.tags.len() as u32).to_le_bytes());
            for t in &e.tags {
                write_string(&mut buf, t);
            }
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut o = 0usize;
        let count = read_u32(data, &mut o, "entry count")? as usize;
        // Sanity-check: each entry must contain at least the classname length
        // u32 + 6 floats (origin + angles) + kvp_count u32 + tag_count u32 =
        // 9 × 4 = 36 bytes with empty strings. An entry count larger than
        // `remaining / 36` cannot possibly be satisfied; reject early to
        // prevent `Vec::with_capacity` from attempting a huge allocation on a
        // malformed or truncated header (e.g. `u32::MAX` entry_count).
        const MIN_RECORD_SIZE: usize = 36;
        let remaining = data.len().saturating_sub(o);
        if count > remaining / MIN_RECORD_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "map entities: truncated — entry count {count} exceeds what remaining {remaining} bytes can hold"
                ),
            )));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let classname = read_string(data, &mut o, &format!("entry {i} classname"))?;
            let origin = read_vec3(data, &mut o, &format!("entry {i} origin"))?;
            let angles = read_vec3(data, &mut o, &format!("entry {i} angles"))?;

            let kvp_count = read_u32(data, &mut o, &format!("entry {i} kvp count"))? as usize;
            // Each KVP encodes at minimum two 4-byte length prefixes (key + value = 8 bytes with
            // empty strings). Reject implausible counts before `Vec::with_capacity` to avoid OOM
            // on corrupt or malicious input.
            const MIN_KVP_SIZE: usize = 8;
            let remaining_for_kvps = data.len().saturating_sub(o);
            if kvp_count > remaining_for_kvps / MIN_KVP_SIZE {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "map entities: entry {i} kvp count {kvp_count} exceeds what remaining {remaining_for_kvps} bytes can hold"
                    ),
                )));
            }
            let mut key_values = Vec::with_capacity(kvp_count);
            for j in 0..kvp_count {
                let k = read_string(data, &mut o, &format!("entry {i} kvp {j} key"))?;
                let v = read_string(data, &mut o, &format!("entry {i} kvp {j} value"))?;
                key_values.push((k, v));
            }

            let tag_count = read_u32(data, &mut o, &format!("entry {i} tag count"))? as usize;
            // Each tag encodes at minimum one 4-byte length prefix. Same rationale as kvp_count.
            const MIN_TAG_SIZE: usize = 4;
            let remaining_for_tags = data.len().saturating_sub(o);
            if tag_count > remaining_for_tags / MIN_TAG_SIZE {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "map entities: entry {i} tag count {tag_count} exceeds what remaining {remaining_for_tags} bytes can hold"
                    ),
                )));
            }
            let mut tags = Vec::with_capacity(tag_count);
            for j in 0..tag_count {
                tags.push(read_string(data, &mut o, &format!("entry {i} tag {j}"))?);
            }

            entries.push(MapEntityRecord {
                classname,
                origin,
                angles,
                key_values,
                tags,
            });
        }
        Ok(Self { entries })
    }
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn read_u32(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<u32> {
    if *o + 4 > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("map entities: truncated {ctx}"),
        )));
    }
    let v = u32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
    *o += 4;
    Ok(v)
}

fn read_f32(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<f32> {
    if *o + 4 > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("map entities: truncated {ctx}"),
        )));
    }
    let v = f32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
    *o += 4;
    Ok(v)
}

fn read_vec3(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<[f32; 3]> {
    let x = read_f32(data, o, ctx)?;
    let y = read_f32(data, o, ctx)?;
    let z = read_f32(data, o, ctx)?;
    Ok([x, y, z])
}

fn read_string(data: &[u8], o: &mut usize, ctx: &str) -> crate::Result<String> {
    let byte_len = read_u32(data, o, &format!("{ctx} length"))? as usize;
    if *o + byte_len > data.len() {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("map entities: truncated {ctx} payload"),
        )));
    }
    let s = std::str::from_utf8(&data[*o..*o + byte_len]).map_err(|_| {
        FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("map entities: invalid UTF-8 in {ctx}"),
        ))
    })?;
    *o += byte_len;
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let section = MapEntitySection::default();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 4);
        let restored = MapEntitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_single_entry_no_kvp_no_tags() {
        let section = MapEntitySection {
            entries: vec![MapEntityRecord {
                classname: "trigger_once".to_string(),
                origin: [1.0, 2.0, 3.0],
                angles: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
                key_values: vec![],
                tags: vec![],
            }],
        };
        let bytes = section.to_bytes();
        let restored = MapEntitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_with_kvps_and_tags() {
        let section = MapEntitySection {
            entries: vec![
                MapEntityRecord {
                    classname: "billboard_emitter".to_string(),
                    origin: [-4.0, 0.5, 12.25],
                    angles: [0.1, 0.2, -0.3],
                    key_values: vec![
                        ("rate".to_string(), "9.5".to_string()),
                        ("sprite".to_string(), "campfire".to_string()),
                    ],
                    tags: vec!["fx".to_string(), "campfires".to_string()],
                },
                MapEntityRecord {
                    classname: "monster_zombie".to_string(),
                    origin: [0.0, 0.0, 0.0],
                    angles: [0.0, 0.0, 0.0],
                    key_values: vec![],
                    tags: vec![],
                },
            ],
        };
        let bytes = section.to_bytes();
        let restored = MapEntitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn round_trip_unicode_strings() {
        let section = MapEntitySection {
            entries: vec![MapEntityRecord {
                classname: "écran".to_string(),
                origin: [0.0; 3],
                angles: [0.0; 3],
                key_values: vec![("clé".to_string(), "valeur ✨".to_string())],
                tags: vec!["thème".to_string()],
            }],
        };
        let bytes = section.to_bytes();
        let restored = MapEntitySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = MapEntitySection::from_bytes(&[0u8; 3]).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_truncated_payload() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // 1 entry
        buf.extend_from_slice(&5u32.to_le_bytes()); // claim 5-byte classname
        buf.extend_from_slice(b"abc"); // only 3 bytes
        let err = MapEntitySection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_invalid_utf8() {
        // Build a structurally-complete record so the count-bound check passes
        // and the UTF-8 validator is reached. Layout: entry_count(u32) +
        // classname_len(u32) + classname_bytes(2) + origin(3×f32) +
        // angles(3×f32) + kvp_count(u32) + tag_count(u32) = 42 bytes.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // entry_count
        buf.extend_from_slice(&2u32.to_le_bytes()); // classname byte len = 2
        buf.push(0xFF); // invalid UTF-8 byte 1
        buf.push(0xFE); // invalid UTF-8 byte 2
        for _ in 0..6 {
            buf.extend_from_slice(&0f32.to_le_bytes()); // origin + angles (6 × f32)
        }
        buf.extend_from_slice(&0u32.to_le_bytes()); // kvp_count
        buf.extend_from_slice(&0u32.to_le_bytes()); // tag_count
        let err = MapEntitySection::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("invalid UTF-8"));
    }
}
