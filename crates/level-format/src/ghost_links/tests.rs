use super::*;
use crate::environment_volumes::EnvironmentPlane;

fn brush() -> EnvironmentBrush {
    EnvironmentBrush {
        aabb_min: [0.0; 3],
        aabb_max: [1.0; 3],
        planes: (0..3)
            .flat_map(|axis| {
                [-1.0, 1.0].map(|sign| {
                    let mut normal = [0.0; 3];
                    normal[axis] = sign;
                    EnvironmentPlane {
                        normal,
                        distance: if sign > 0.0 { 1.0 } else { 0.0 },
                    }
                })
            })
            .collect(),
    }
}

/// Style "echo" (4 bytes), so the brush count starts at byte 20.
fn record() -> GhostLinkRecord {
    GhostLinkRecord {
        source_entity: 7,
        enabled_on_spawn: false,
        members: GhostMembers::PLAYERS | GhostMembers::MOVERS,
        style: "echo".to_owned(),
        brushes: vec![brush(), brush()],
        landmark_origin: [1.0, 2.0, 3.0],
        landmark_yaw: -1.0,
        target_origin: [3.0, 4.0, 5.0],
        target_yaw: 1.0,
        ghost_tags: vec!["barrier".to_owned(), "lift".to_owned()],
    }
}

fn section() -> GhostLinksSection {
    GhostLinksSection {
        links: vec![record()],
    }
}

fn rejects(bytes: &[u8], message: &str) {
    let error = GhostLinksSection::from_bytes(bytes).expect_err("invalid ghost-link bytes fail");
    assert!(error.to_string().contains(message), "{error}");
}

/// Offset of the first link's landmark origin: header, fixed prefix, style,
/// brush count and two six-plane brushes.
const LANDMARK: usize = 6 + 4 + 1 + 1 + 4 + 4 + 4 + 2 * (28 + 6 * 16);

#[test]
fn ghost_link_section_round_trips_every_field_and_documented_layout() {
    let mut second = record();
    second.source_entity = 9;
    second.enabled_on_spawn = true;
    second.members = GhostMembers::all();
    second.style = String::new();
    second.ghost_tags.clear();
    let section = GhostLinksSection {
        links: vec![record(), second],
    };
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.byte_len());
    // Version 1, two links; link 0: source 7, disarmed, players + movers, "echo".
    assert_eq!(&bytes[..6], &[1, 0, 2, 0, 0, 0]);
    assert_eq!(&bytes[6..12], &[7, 0, 0, 0, 0, 0b1001]);
    assert_eq!(&bytes[12..20], &[4, 0, 0, 0, b'e', b'c', b'h', b'o']);
    assert_eq!(&bytes[20..24], &2u32.to_le_bytes());
    assert_eq!(&bytes[LANDMARK..LANDMARK + 4], &1.0f32.to_le_bytes());
    assert_eq!(GhostLinksSection::from_bytes(&bytes).unwrap(), section);
    assert_eq!(
        crate::SectionId::from_u32(56),
        Some(crate::SectionId::GhostLinks)
    );
}

#[test]
fn ghost_link_member_names_match_the_documented_bit_order() {
    let named: Vec<_> = GhostMembers::NAMED
        .iter()
        .map(|(name, category)| (*name, category.bits()))
        .collect();
    assert_eq!(
        named,
        [
            ("players", 1),
            ("entities", 2),
            ("projectiles", 4),
            ("movers", 8)
        ]
    );
    assert_eq!(GhostMembers::all().bits(), 0x0F);
    assert_eq!(
        GhostMembers::from_fgd_name("movers"),
        Some(GhostMembers::MOVERS)
    );
    assert_eq!(GhostMembers::from_fgd_name("Players"), None);
}

#[test]
fn ghost_link_section_rejects_every_truncation_and_trailing_bytes() {
    let mut bytes = section().to_bytes();
    for len in 0..bytes.len() {
        assert!(
            GhostLinksSection::from_bytes(&bytes[..len]).is_err(),
            "length {len}"
        );
    }
    bytes.push(0);
    rejects(&bytes, "trailing");
}

#[test]
fn ghost_link_section_rejects_bad_header_counts_flags_and_member_bits() {
    let base = section().to_bytes();
    let mut bad = base.clone();
    bad[0] = 2;
    rejects(&bad, "version");
    rejects(&GhostLinksSection::default().to_bytes(), "link count 0");
    let mut bad = vec![0u8; 6 + (MAX_GHOST_LINKS + 1) * MIN_RECORD_SIZE];
    bad[..2].copy_from_slice(&1u16.to_le_bytes());
    bad[2..6].copy_from_slice(&65_536u32.to_le_bytes());
    rejects(&bad, "exceed");
    let mut bad = base.clone();
    bad[10] = 2;
    rejects(&bad, "0 or 1");
    for reserved in [0x10u8, 0x80] {
        let mut bad = base.clone();
        bad[11] = reserved;
        rejects(&bad, "past bit 3");
    }
}

#[test]
fn ghost_link_section_rejects_invalid_convex_leaves_and_utf8() {
    let base = section().to_bytes();
    let mut bad = base.clone();
    bad[20..24].copy_from_slice(&0u32.to_le_bytes());
    rejects(&bad, "no brushes");
    let mut bad = base.clone();
    bad[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    rejects(&bad, "remaining");
    let mut bad = base.clone();
    bad[24..28].copy_from_slice(&2.0f32.to_le_bytes());
    rejects(&bad, "AABB");
    let mut bad = base.clone();
    bad[48..52].copy_from_slice(&3u32.to_le_bytes());
    rejects(&bad, "four planes");
    let mut bad = base.clone();
    bad[52..64].fill(0);
    rejects(&bad, "zero plane normal");
    let mut bad = base.clone();
    bad[16] = 0xFF;
    rejects(&bad, "UTF-8");
}

#[test]
fn ghost_link_section_rejects_nonfinite_poses_and_out_of_order_sources() {
    for mutate in 0..4 {
        let mut section = section();
        let link = &mut section.links[0];
        match mutate {
            0 => link.landmark_origin[0] = f32::INFINITY,
            1 => link.landmark_yaw = f32::NAN,
            2 => link.target_origin[2] = f32::NEG_INFINITY,
            _ => link.target_yaw = f32::NAN,
        }
        rejects(&section.to_bytes(), "finite");
    }
    let mut repeated = section();
    repeated.links.push(record());
    rejects(&repeated.to_bytes(), "map order");
}

#[test]
fn ghost_link_section_rejects_empty_and_duplicated_ghost_tags() {
    let mut empty = section();
    empty.links[0].ghost_tags[1] = String::new();
    rejects(&empty.to_bytes(), "ghost tag 1 is empty");
    let mut duplicated = section();
    duplicated.links[0].ghost_tags[1] = "barrier".to_owned();
    rejects(&duplicated.to_bytes(), "`barrier` is duplicated");
}
