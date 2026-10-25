//! GhostLinks (id 56): link regions, landmark → target poses, starting state
//! and ghosted mover tags. Presentation only; no level digest hashes it.
//! See: context/lib/build_pipeline.md §PRL section IDs · context/lib/entity_model.md §7d
use crate::convex_leaf_codec::{
    SectionReader, brushes_byte_len, write_brushes, write_count, write_f32, write_string,
    write_vec3,
};
use crate::environment_volumes::EnvironmentBrush;
use bitflags::bitflags;

pub const GHOST_LINKS_VERSION: u16 = 1;
/// Map-order link indices are `u16`; 0xFFFF is reserved for no link.
pub const MAX_GHOST_LINKS: usize = u16::MAX as usize;
/// The map classname of a link volume and of its id-29 companion record.
pub const GHOST_LINK_CLASSNAME: &str = "ghost_link";
/// Id-29 companion metadata binds to this source entity index.
pub const GHOST_LINK_SOURCE_ENTITY_KEY: &str = "_ghost_link_source_entity";
/// Source entity, enabled, members, style length, brush count, landmark
/// xyz/yaw, target xyz/yaw, ghost-tag count.
const MIN_RECORD_SIZE: usize = 4 + 1 + 1 + 4 + 4 + 16 + 16 + 4;
/// A ghost tag is never empty: its length and at least one byte.
const MIN_TAG_SIZE: usize = 4 + 1;

bitflags! {
    /// The member categories a link shows, in the bit order PRL id 56 and the
    /// link-state wire payload share. Bits 4–7 are reserved and rejected.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct GhostMembers: u8 {
        /// Player pawns.
        const PLAYERS = 1 << 0;
        /// Non-player descriptor entities.
        const ENTITIES = 1 << 1;
        const PROJECTILES = 1 << 2;
        /// Kinematic movers named by `ghost_tags`.
        const MOVERS = 1 << 3;
    }
}

impl GhostMembers {
    /// The `ghost_members` FGD spelling of each category, in bit order.
    pub const NAMED: [(&'static str, GhostMembers); 4] = [
        ("players", GhostMembers::PLAYERS),
        ("entities", GhostMembers::ENTITIES),
        ("projectiles", GhostMembers::PROJECTILES),
        ("movers", GhostMembers::MOVERS),
    ];

    /// The category an FGD `ghost_members` word names.
    pub fn from_fgd_name(name: &str) -> Option<Self> {
        Self::NAMED
            .iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, category)| *category)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GhostLinkRecord {
    /// Raw `.map` entity index, as teleporters and environment volumes key it.
    pub source_entity: u32,
    pub enabled_on_spawn: bool,
    /// Starting member categories.
    pub members: GhostMembers,
    /// Manifest ghost style name; empty means the engine floor style.
    pub style: String,
    /// One convex leaf per authored brush; the link volume is their union.
    pub brushes: Vec<EnvironmentBrush>,
    /// Engine metres and engine yaw radians, as teleporter poses.
    pub landmark_origin: [f32; 3],
    pub landmark_yaw: f32,
    pub target_origin: [f32; 3],
    pub target_yaw: f32,
    /// Mover tags this link ghosts, authored order, deduplicated, never empty.
    pub ghost_tags: Vec<String>,
}

/// Little-endian u16 version, u32 link count (1..=65,535). Each record:
/// source u32, enabled u8, members u8, u32-length UTF-8 style, u32 brush
/// count and the id-52 brush encoding, landmark xyz/yaw and target xyz/yaw
/// f32, then u32 ghost-tag count and each tag u32-length UTF-8. The section
/// is absent when a map has no links, so an empty one never encodes validly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GhostLinksSection {
    pub links: Vec<GhostLinkRecord>,
}

impl GhostLinksSection {
    pub fn byte_len(&self) -> usize {
        6 + self
            .links
            .iter()
            .map(|link| {
                // The brush count is inside MIN_RECORD_SIZE.
                MIN_RECORD_SIZE - 4
                    + link.style.len()
                    + brushes_byte_len(&link.brushes)
                    + link.ghost_tags.iter().map(|t| 4 + t.len()).sum::<usize>()
            })
            .sum::<usize>()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        assert!(
            self.links.len() <= MAX_GHOST_LINKS,
            "ghost link count exceeds index space"
        );
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&GHOST_LINKS_VERSION.to_le_bytes());
        write_count(&mut out, self.links.len());
        for link in &self.links {
            out.extend_from_slice(&link.source_entity.to_le_bytes());
            out.extend_from_slice(&[u8::from(link.enabled_on_spawn), link.members.bits()]);
            write_string(&mut out, &link.style);
            write_brushes(&mut out, &link.brushes);
            write_vec3(&mut out, link.landmark_origin);
            write_f32(&mut out, link.landmark_yaw);
            write_vec3(&mut out, link.target_origin);
            write_f32(&mut out, link.target_yaw);
            write_count(&mut out, link.ghost_tags.len());
            for tag in &link.ghost_tags {
                write_string(&mut out, tag);
            }
        }
        out
    }

    /// Validate all nested counts before allocating and all floats on read.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = SectionReader::new(data, "ghost links");
        let version = r.u16("version")?;
        if version != GHOST_LINKS_VERSION {
            return r.invalid(format!("unsupported version {version}"));
        }
        let n = r.bounded_count("link count", MIN_RECORD_SIZE)?;
        if n == 0 {
            return r.invalid("link count 0; a map with no links omits the section".to_owned());
        }
        if n > MAX_GHOST_LINKS {
            return r.invalid(format!("{n} links exceed {MAX_GHOST_LINKS}"));
        }
        let mut links: Vec<GhostLinkRecord> = Vec::with_capacity(n);
        for i in 0..n {
            let what = format!("link {i}");
            let source_entity = r.u32(&format!("{what} source entity"))?;
            if links
                .last()
                .is_some_and(|link| link.source_entity >= source_entity)
            {
                return r.invalid(format!("{what} source entity is not in strict map order"));
            }
            let enabled_on_spawn = r.flag(&format!("{what} enabled"))?;
            let raw_members = r.u8(&format!("{what} member categories"))?;
            let Some(members) = GhostMembers::from_bits(raw_members) else {
                return r.invalid(format!(
                    "{what} member categories {raw_members:#04x} set bits past bit 3"
                ));
            };
            let style = r.string(&format!("{what} style"))?;
            let brushes = r.brushes(&what)?;
            let landmark_origin = r.finite_vec3(&format!("{what} landmark origin"))?;
            let landmark_yaw = r.finite_f32(&format!("{what} landmark yaw"))?;
            let target_origin = r.finite_vec3(&format!("{what} target origin"))?;
            let target_yaw = r.finite_f32(&format!("{what} target yaw"))?;
            let tag_count = r.bounded_count(&format!("{what} ghost-tag count"), MIN_TAG_SIZE)?;
            let mut ghost_tags: Vec<String> = Vec::with_capacity(tag_count);
            // A set, not `contains`: untrusted bytes can carry many tags.
            let mut seen = std::collections::HashSet::with_capacity(tag_count);
            for t in 0..tag_count {
                let tag = r.string(&format!("{what} ghost tag {t}"))?;
                if tag.is_empty() {
                    return r.invalid(format!("{what} ghost tag {t} is empty"));
                }
                if !seen.insert(tag.clone()) {
                    return r.invalid(format!("{what} ghost tag `{tag}` is duplicated"));
                }
                ghost_tags.push(tag);
            }
            links.push(GhostLinkRecord {
                source_entity,
                enabled_on_spawn,
                members,
                style,
                brushes,
                landmark_origin,
                landmark_yaw,
                target_origin,
                target_yaw,
                ghost_tags,
            });
        }
        r.finish()?;
        Ok(Self { links })
    }
}

#[cfg(test)]
mod tests;
