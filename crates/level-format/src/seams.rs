//! Seams (id 61): level-seam regions, catalog targets and resolved landmarks.
//! See: context/lib/build_pipeline.md §PRL section IDs (Seams, id 61)
use crate::convex_leaf_codec::{
    SectionReader, brushes_byte_len, write_brushes, write_count, write_f32, write_string,
    write_vec3,
};
use crate::environment_volumes::EnvironmentBrush;
use std::collections::HashSet;

pub const SEAMS_VERSION: u16 = 1;
/// Seam indices share the teleporter's u16 index space.
pub const MAX_SEAMS: usize = u16::MAX as usize;
/// Id29 companion metadata binds to this source entity index.
pub const SEAM_SOURCE_ENTITY_KEY: &str = "_seam_source_entity";
/// The FGD brush classname, and the classname of each seam's id-29 companion.
pub const SEAM_CLASSNAME: &str = "level_seam";
/// Source u32, three string lengths, activation, mode, enabled, brush count,
/// landmark xyz and yaw.
const MIN_RECORD_SIZE: usize = 4 + 4 + 4 + 3 + 4 + 16 + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SeamActivation {
    Touch = 0,
    Use = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SeamMode {
    Classic = 0,
    /// Crosses exactly as classic until the seamless unit lands.
    Seamless = 1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeamRecord {
    /// Raw `.map` entity index, as teleporters key it; the id-29 companion
    /// carries it under [`SEAM_SOURCE_ENTITY_KEY`].
    pub source_entity: u32,
    /// Shared by both maps of a seam pair; unique within one map.
    pub name: String,
    /// The next map's catalog id. Not validated at compile time.
    pub target: String,
    pub activation: SeamActivation,
    pub mode: SeamMode,
    pub enabled_on_spawn: bool,
    pub brushes: Vec<EnvironmentBrush>,
    /// Engine metres: the authored landmark, else the brushes' union AABB centre.
    pub landmark_origin: [f32; 3],
    /// Engine yaw in radians; 0 for a defaulted landmark.
    pub landmark_yaw: f32,
    /// Reaction name; empty means none.
    pub on_arrive: String,
}

/// Little-endian u16 epoch, u32 record count. Each record: source u32, name
/// and target strings, activation u8, mode u8, enabled u8, u32 brush count and
/// id52 brush encoding, landmark xyz/yaw f32, then the `on_arrive` string.
/// Strings are u32-length UTF-8; empty means absent, and a seam's name and
/// target are never absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeamsSection {
    pub seams: Vec<SeamRecord>,
}

impl SeamsSection {
    pub fn byte_len(&self) -> usize {
        6 + self
            .seams
            .iter()
            .map(|s| {
                // The brush count is inside MIN_RECORD_SIZE.
                MIN_RECORD_SIZE - 4
                    + s.name.len()
                    + s.target.len()
                    + s.on_arrive.len()
                    + brushes_byte_len(&s.brushes)
            })
            .sum::<usize>()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        assert!(
            self.seams.len() <= MAX_SEAMS,
            "seam count exceeds index space"
        );
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&SEAMS_VERSION.to_le_bytes());
        write_count(&mut out, self.seams.len());
        for s in &self.seams {
            out.extend_from_slice(&s.source_entity.to_le_bytes());
            write_string(&mut out, &s.name);
            write_string(&mut out, &s.target);
            out.extend_from_slice(&[
                s.activation as u8,
                s.mode as u8,
                u8::from(s.enabled_on_spawn),
            ]);
            write_brushes(&mut out, &s.brushes);
            write_vec3(&mut out, s.landmark_origin);
            write_f32(&mut out, s.landmark_yaw);
            write_string(&mut out, &s.on_arrive);
        }
        out
    }

    /// Validate all nested counts before allocating, every float, both enums,
    /// strict map order, and a non-empty, unrepeated name and a target.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = SectionReader::new(data, "seams");
        let version = r.u16("version")?;
        if version != SEAMS_VERSION {
            return r.invalid(format!("unsupported version {version}"));
        }
        let n = r.bounded_count("record count", MIN_RECORD_SIZE)?;
        if n > MAX_SEAMS {
            return r.invalid(format!("{n} records exceed {MAX_SEAMS}"));
        }
        let mut seams: Vec<SeamRecord> = Vec::with_capacity(n);
        let mut names = HashSet::with_capacity(n);
        for i in 0..n {
            let what = format!("record {i}");
            let source_entity = r.u32(&what)?;
            if seams
                .last()
                .is_some_and(|s| s.source_entity >= source_entity)
            {
                return r.invalid(format!("{what} source entity is not in strict map order"));
            }
            let name = r.string(&format!("{what} name"))?;
            if name.is_empty() {
                return r.invalid(format!("{what} has an empty name"));
            }
            if !names.insert(name.clone()) {
                return r.invalid(format!("{what} repeats seam name `{name}`"));
            }
            let target = r.string(&format!("{what} target"))?;
            if target.is_empty() {
                return r.invalid(format!("{what} has an empty target"));
            }
            let activation = match r.u8(&what)? {
                0 => SeamActivation::Touch,
                1 => SeamActivation::Use,
                value => return r.invalid(format!("{what} unknown activation {value}")),
            };
            let mode = match r.u8(&what)? {
                0 => SeamMode::Classic,
                1 => SeamMode::Seamless,
                value => return r.invalid(format!("{what} unknown mode {value}")),
            };
            let enabled_on_spawn = r.flag(&format!("{what} enabled"))?;
            let brushes = r.brushes(&what)?;
            let landmark_origin = r.finite_vec3("landmark origin")?;
            let landmark_yaw = r.finite_f32("landmark yaw")?;
            let on_arrive = r.string("on arrive")?;
            seams.push(SeamRecord {
                source_entity,
                name,
                target,
                activation,
                mode,
                enabled_on_spawn,
                brushes,
                landmark_origin,
                landmark_yaw,
                on_arrive,
            });
        }
        r.finish()?;
        Ok(Self { seams })
    }
}

#[cfg(test)]
mod tests;
