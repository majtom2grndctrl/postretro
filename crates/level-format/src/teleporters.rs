//! Teleporters (id 55): convex regions and resolved arrival poses.
//! See: context/lib/build_pipeline.md §PRL section IDs
use crate::FormatError;
use crate::environment_volumes::{EnvironmentBrush, EnvironmentPlane, MIN_PLANES_PER_BRUSH};
pub const TELEPORTERS_VERSION: u16 = 1;
/// The pawn record reserves 0xffff for no teleporter.
pub const MAX_TELEPORTERS: usize = u16::MAX as usize;
/// Id29 companion metadata binds to this source entity index.
pub const TELEPORTER_SOURCE_ENTITY_KEY: &str = "_teleporter_source_entity";
const MIN_RECORD_SIZE: usize = 50;
const MIN_BRUSH_SIZE: usize = 28;
const PLANE_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TeleportMode {
    Classic = 0,
    Seamless = 1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeleporterRecord {
    pub source_entity: u32,
    pub mode: TeleportMode,
    pub enabled_on_spawn: bool,
    pub brushes: Vec<EnvironmentBrush>,
    pub destination_origin: [f32; 3],
    pub destination_yaw: f32,
    pub landmark_origin: [f32; 3],
    pub landmark_yaw: f32,
    pub arrival_sound: String,
    pub on_teleport: String,
}

/// Little-endian u16 epoch, u32 record count. Each record: source u32,
/// mode u8, enabled u8, u32 brush count and id52 brush encoding, destination
/// xyz/yaw and landmark xyz/yaw f32, then two u32-length UTF-8 strings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeleportersSection {
    pub teleporters: Vec<TeleporterRecord>,
}
impl TeleportersSection {
    pub fn byte_len(&self) -> usize {
        6 + self
            .teleporters
            .iter()
            .map(|t| {
                MIN_RECORD_SIZE
                    + t.arrival_sound.len()
                    + t.on_teleport.len()
                    + t.brushes
                        .iter()
                        .map(|b| MIN_BRUSH_SIZE + PLANE_SIZE * b.planes.len())
                        .sum::<usize>()
            })
            .sum::<usize>()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        assert!(
            self.teleporters.len() <= MAX_TELEPORTERS,
            "teleporter count exceeds index space"
        );
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&TELEPORTERS_VERSION.to_le_bytes());
        count(&mut out, self.teleporters.len());
        for t in &self.teleporters {
            out.extend_from_slice(&t.source_entity.to_le_bytes());
            out.extend_from_slice(&[t.mode as u8, u8::from(t.enabled_on_spawn)]);
            count(&mut out, t.brushes.len());
            for b in &t.brushes {
                vec3(&mut out, b.aabb_min);
                vec3(&mut out, b.aabb_max);
                count(&mut out, b.planes.len());
                for p in &b.planes {
                    vec3(&mut out, p.normal);
                    f32_out(&mut out, p.distance);
                }
            }
            vec3(&mut out, t.destination_origin);
            f32_out(&mut out, t.destination_yaw);
            let (origin, yaw) = match t.mode {
                TeleportMode::Classic => ([0.0; 3], 0.0),
                TeleportMode::Seamless => (t.landmark_origin, t.landmark_yaw),
            };
            vec3(&mut out, origin);
            f32_out(&mut out, yaw);
            for value in [&t.arrival_sound, &t.on_teleport] {
                count(&mut out, value.len());
                out.extend_from_slice(value.as_bytes());
            }
        }
        out
    }
    /// Validate all nested counts before allocating and all floats on read.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = Reader { data, offset: 0 };
        let version = r.u16("version")?;
        if version != TELEPORTERS_VERSION {
            return invalid(format!("unsupported version {version}"));
        }
        let n = r.bounded_count("record count", MIN_RECORD_SIZE)?;
        if n > MAX_TELEPORTERS {
            return invalid(format!("{n} records exceed {MAX_TELEPORTERS}"));
        }
        let mut teleporters = Vec::with_capacity(n);
        for i in 0..n {
            let what = format!("record {i}");
            let source_entity = r.u32(&what)?;
            if teleporters
                .last()
                .is_some_and(|t: &TeleporterRecord| t.source_entity >= source_entity)
            {
                return invalid(format!("{what} source entity is not in strict map order"));
            }
            let mode = match r.u8(&what)? {
                0 => TeleportMode::Classic,
                1 => TeleportMode::Seamless,
                value => return invalid(format!("{what} unknown mode {value}")),
            };
            let enabled_on_spawn = r.flag(&format!("{what} enabled"))?;
            let nb = r.bounded_count("brush count", MIN_BRUSH_SIZE)?;
            if nb == 0 {
                return invalid(format!("{what} has no brushes"));
            }
            let mut brushes = Vec::with_capacity(nb);
            for b in 0..nb {
                let what = format!("record {i} brush {b}");
                let aabb_min = r.finite_vec3(&what)?;
                let aabb_max = r.finite_vec3(&what)?;
                if aabb_min.iter().zip(aabb_max).any(|(min, max)| *min > max) {
                    return invalid(format!("{what} inverted AABB"));
                }
                let np = r.bounded_count("plane count", PLANE_SIZE)?;
                if np < MIN_PLANES_PER_BRUSH {
                    return invalid(format!("{what} needs at least four planes"));
                }
                let mut planes = Vec::with_capacity(np);
                for _ in 0..np {
                    let normal = r.finite_vec3(&what)?;
                    if normal == [0.0; 3] {
                        return invalid(format!("{what} has zero plane normal"));
                    }
                    planes.push(EnvironmentPlane {
                        normal,
                        distance: r.finite_f32(&what)?,
                    });
                }
                brushes.push(EnvironmentBrush {
                    aabb_min,
                    aabb_max,
                    planes,
                });
            }
            let destination_origin = r.finite_vec3("destination origin")?;
            let destination_yaw = r.finite_f32("destination yaw")?;
            let mut landmark_origin = r.finite_vec3("landmark origin")?;
            let mut landmark_yaw = r.finite_f32("landmark yaw")?;
            if mode == TeleportMode::Classic {
                landmark_origin = [0.0; 3];
                landmark_yaw = 0.0;
            }
            teleporters.push(TeleporterRecord {
                source_entity,
                mode,
                enabled_on_spawn,
                brushes,
                destination_origin,
                destination_yaw,
                landmark_origin,
                landmark_yaw,
                arrival_sound: r.string("arrival sound")?,
                on_teleport: r.string("on teleport")?,
            });
        }
        if r.offset != data.len() {
            return invalid("trailing bytes".to_owned());
        }
        Ok(Self { teleporters })
    }
}

fn invalid<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("teleporters: {message}"),
    )))
}

fn count(out: &mut Vec<u8>, n: usize) {
    let n = u32::try_from(n).expect("teleporter counts fit the PRL u32 field");
    out.extend_from_slice(&n.to_le_bytes());
}

fn f32_out(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn vec3(out: &mut Vec<u8>, value: [f32; 3]) {
    for v in value {
        f32_out(out, v);
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
                    format!("teleporters: truncated {what}"),
                ))
            })?;
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> crate::Result<u8> {
        Ok(self.take(1, what)?[0])
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

    fn flag(&mut self, what: &str) -> crate::Result<bool> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            other => invalid(format!("{what} must be 0 or 1, got {other}")),
        }
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

    fn string(&mut self, what: &str) -> crate::Result<String> {
        let n = self.u32(&format!("{what} length"))? as usize;
        let bytes = self.take(n, what)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .or_else(|_| invalid(format!("invalid UTF-8 in {what}")))
    }
}

#[cfg(test)]
mod tests;
