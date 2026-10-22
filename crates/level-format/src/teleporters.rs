//! Teleporters (id 55): convex regions and resolved arrival poses.
//! See: context/lib/build_pipeline.md §PRL section IDs
use crate::convex_leaf_codec::{
    SectionReader, brushes_byte_len, write_brushes, write_count, write_f32, write_string,
    write_vec3,
};
use crate::environment_volumes::EnvironmentBrush;
pub const TELEPORTERS_VERSION: u16 = 1;
/// The pawn record reserves 0xffff for no teleporter.
pub const MAX_TELEPORTERS: usize = u16::MAX as usize;
/// Id29 companion metadata binds to this source entity index.
pub const TELEPORTER_SOURCE_ENTITY_KEY: &str = "_teleporter_source_entity";
const MIN_RECORD_SIZE: usize = 50;

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
    /// Engine metres.
    pub destination_origin: [f32; 3],
    /// Engine yaw in radians, the same convention as id-29 entity angles.
    pub destination_yaw: f32,
    /// Seamless only; classic writes zeros.
    pub landmark_origin: [f32; 3],
    pub landmark_yaw: f32,
    pub arrival_sound: String,
    pub on_teleport: String,
}

/// Little-endian u16 epoch, u32 record count. Each record: source u32,
/// mode u8, enabled u8, u32 brush count and id52 brush encoding, destination
/// xyz/yaw and landmark xyz/yaw f32, then two u32-length UTF-8 strings. A
/// classic record's landmark must still be finite to decode, and is then
/// ignored: it decodes as zeros.
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
                // The brush count is inside MIN_RECORD_SIZE.
                MIN_RECORD_SIZE - 4
                    + t.arrival_sound.len()
                    + t.on_teleport.len()
                    + brushes_byte_len(&t.brushes)
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
        write_count(&mut out, self.teleporters.len());
        for t in &self.teleporters {
            out.extend_from_slice(&t.source_entity.to_le_bytes());
            out.extend_from_slice(&[t.mode as u8, u8::from(t.enabled_on_spawn)]);
            write_brushes(&mut out, &t.brushes);
            write_vec3(&mut out, t.destination_origin);
            write_f32(&mut out, t.destination_yaw);
            let (origin, yaw) = match t.mode {
                TeleportMode::Classic => ([0.0; 3], 0.0),
                TeleportMode::Seamless => (t.landmark_origin, t.landmark_yaw),
            };
            write_vec3(&mut out, origin);
            write_f32(&mut out, yaw);
            write_string(&mut out, &t.arrival_sound);
            write_string(&mut out, &t.on_teleport);
        }
        out
    }
    /// Validate all nested counts before allocating and all floats on read.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = SectionReader::new(data, "teleporters");
        let version = r.u16("version")?;
        if version != TELEPORTERS_VERSION {
            return r.invalid(format!("unsupported version {version}"));
        }
        let n = r.bounded_count("record count", MIN_RECORD_SIZE)?;
        if n > MAX_TELEPORTERS {
            return r.invalid(format!("{n} records exceed {MAX_TELEPORTERS}"));
        }
        let mut teleporters = Vec::with_capacity(n);
        for i in 0..n {
            let what = format!("record {i}");
            let source_entity = r.u32(&what)?;
            if teleporters
                .last()
                .is_some_and(|t: &TeleporterRecord| t.source_entity >= source_entity)
            {
                return r.invalid(format!("{what} source entity is not in strict map order"));
            }
            let mode = match r.u8(&what)? {
                0 => TeleportMode::Classic,
                1 => TeleportMode::Seamless,
                value => return r.invalid(format!("{what} unknown mode {value}")),
            };
            let enabled_on_spawn = r.flag(&format!("{what} enabled"))?;
            let brushes = r.brushes(&what)?;
            let destination_origin = r.finite_vec3("destination origin")?;
            let destination_yaw = r.finite_f32("destination yaw")?;
            let mut landmark_origin = r.finite_vec3("landmark origin")?;
            let mut landmark_yaw = r.finite_f32("landmark yaw")?;
            // Every f32 field is validated alike; classic then ignores its landmark.
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
        r.finish()?;
        Ok(Self { teleporters })
    }
}

#[cfg(test)]
mod tests;
