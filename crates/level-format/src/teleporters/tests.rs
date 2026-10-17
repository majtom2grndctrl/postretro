use super::*;

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
fn record() -> TeleporterRecord {
    TeleporterRecord {
        source_entity: 7,
        mode: TeleportMode::Seamless,
        enabled_on_spawn: false,
        brushes: vec![brush(), brush()],
        destination_origin: [3.0, 4.0, 5.0],
        destination_yaw: 1.0,
        landmark_origin: [1.0, 2.0, 3.0],
        landmark_yaw: -1.0,
        arrival_sound: "sfx/arrival".to_owned(),
        on_teleport: "gate.used".to_owned(),
    }
}
fn section() -> TeleportersSection {
    TeleportersSection {
        teleporters: vec![record()],
    }
}
fn rejects(bytes: &[u8], message: &str) {
    let error =
        TeleportersSection::from_bytes(bytes).expect_err("invalid teleporter bytes must fail");
    assert!(error.to_string().contains(message), "{error}");
}
#[test]
fn teleport_section_round_trip_and_layout() {
    let section = section();
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.byte_len());
    assert_eq!(&bytes[..6], &[1, 0, 1, 0, 0, 0]);
    assert_eq!(&bytes[6..12], &[7, 0, 0, 0, 1, 0]);
    assert_eq!(TeleportersSection::from_bytes(&bytes).unwrap(), section);
    assert_eq!(
        crate::SectionId::from_u32(55),
        Some(crate::SectionId::Teleporters)
    );
    assert_eq!(
        TeleportersSection::from_bytes(&TeleportersSection::default().to_bytes()).unwrap(),
        TeleportersSection::default()
    );
}
#[test]
fn teleport_classic_writes_and_ignores_landmark_pose() {
    let mut section = section();
    section.teleporters[0].mode = TeleportMode::Classic;
    let bytes = section.to_bytes();
    let landmark_offset = 16 + 2 * (28 + 6 * 16) + 16;
    assert_eq!(&bytes[landmark_offset..landmark_offset + 16], &[0; 16]);
    let mut noncanonical = bytes;
    noncanonical[landmark_offset..landmark_offset + 4].copy_from_slice(&4.0f32.to_le_bytes());
    let decoded = TeleportersSection::from_bytes(&noncanonical).unwrap();
    assert_eq!(decoded.teleporters[0].landmark_origin, [0.0; 3]);
    assert_eq!(decoded.teleporters[0].landmark_yaw, 0.0);
}
#[test]
fn teleport_section_rejects_every_truncation_and_trailing_bytes() {
    let mut bytes = section().to_bytes();
    for len in 0..bytes.len() {
        assert!(
            TeleportersSection::from_bytes(&bytes[..len]).is_err(),
            "length {len}"
        );
    }
    bytes.push(0);
    rejects(&bytes, "trailing");
}
#[test]
fn teleport_section_rejects_bad_header_mode_flag_counts_and_geometry() {
    let base = section().to_bytes();
    let mut bad = base.clone();
    bad[0] = 2;
    rejects(&bad, "version");
    let mut bad = base.clone();
    bad[10] = 2;
    rejects(&bad, "mode");
    let mut bad = base.clone();
    bad[11] = 2;
    rejects(&bad, "0 or 1");
    let mut bad = base.clone();
    bad[12..16].copy_from_slice(&0u32.to_le_bytes());
    rejects(&bad, "no brushes");
    let mut bad = base.clone();
    bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    rejects(&bad, "remaining");
    let mut bad = base.clone();
    bad[16..20].copy_from_slice(&2.0f32.to_le_bytes());
    rejects(&bad, "AABB");
    let mut bad = base.clone();
    bad[40..44].copy_from_slice(&3u32.to_le_bytes());
    rejects(&bad, "four planes");
    let mut bad = base.clone();
    bad[44..56].fill(0);
    rejects(&bad, "zero plane normal");
    let mut bad = base.clone();
    bad[16..20].copy_from_slice(&f32::NAN.to_le_bytes());
    rejects(&bad, "finite");
    let mut bad = base.clone();
    *bad.last_mut().unwrap() = 255;
    rejects(&bad, "UTF-8");
    let mut bad = vec![0u8; 6 + (MAX_TELEPORTERS + 1) * MIN_RECORD_SIZE];
    bad[..2].copy_from_slice(&1u16.to_le_bytes());
    bad[2..6].copy_from_slice(&65536u32.to_le_bytes());
    rejects(&bad, "exceed");
}
#[test]
fn teleport_section_rejects_nonfinite_poses_and_duplicate_source_indices() {
    for mutate in [0, 1, 2, 3] {
        let mut section = section();
        let record = &mut section.teleporters[0];
        match mutate {
            0 => record.destination_origin[0] = f32::INFINITY,
            1 => record.destination_yaw = f32::NAN,
            2 => record.landmark_origin[1] = f32::NEG_INFINITY,
            _ => record.landmark_yaw = f32::NAN,
        }
        rejects(&section.to_bytes(), "finite");
    }
    let mut section = section();
    section.teleporters.push(record());
    rejects(&section.to_bytes(), "map order");
}
