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

fn record() -> SeamRecord {
    SeamRecord {
        source_entity: 7,
        name: "pair_door".to_owned(),
        target: "seam-pair-b".to_owned(),
        activation: SeamActivation::Use,
        mode: SeamMode::Seamless,
        enabled_on_spawn: false,
        brushes: vec![brush(), brush()],
        landmark_origin: [1.0, 2.0, 3.0],
        landmark_yaw: -1.0,
        on_arrive: "seam.arrived".to_owned(),
    }
}

fn section() -> SeamsSection {
    SeamsSection {
        seams: vec![record()],
    }
}

// Byte offsets into `section().to_bytes()`.
const NAME_LEN: usize = 10;
const TARGET_LEN: usize = NAME_LEN + 4 + "pair_door".len();
const ACTIVATION: usize = TARGET_LEN + 4 + "seam-pair-b".len();
const MODE: usize = ACTIVATION + 1;
const ENABLED: usize = MODE + 1;
const BRUSH_COUNT: usize = ENABLED + 1;
/// Two six-plane brushes: AABB, plane count, planes.
const LANDMARK: usize = BRUSH_COUNT + 4 + 2 * (28 + 6 * 16);

fn rejects(bytes: &[u8], message: &str) {
    let error = SeamsSection::from_bytes(bytes).expect_err("invalid seam bytes must fail");
    assert!(error.to_string().contains(message), "{error}");
    assert!(error.to_string().contains("seams"), "{error}");
}

#[test]
fn seam_section_round_trip_and_layout() {
    let section = section();
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.byte_len());
    assert_eq!(&bytes[..6], &[1, 0, 1, 0, 0, 0]);
    assert_eq!(&bytes[6..10], &[7, 0, 0, 0]);
    assert_eq!(&bytes[NAME_LEN..NAME_LEN + 4], &[9, 0, 0, 0]);
    assert_eq!(&bytes[ACTIVATION..BRUSH_COUNT + 4], &[1, 1, 0, 2, 0, 0, 0]);
    assert_eq!(
        &bytes[LANDMARK..LANDMARK + 4],
        &1.0f32.to_le_bytes(),
        "landmark follows the brushes"
    );
    assert_eq!(SeamsSection::from_bytes(&bytes).unwrap(), section);
    assert_eq!(
        crate::SectionId::from_u32(61),
        Some(crate::SectionId::Seams)
    );
    assert_eq!(
        SeamsSection::from_bytes(&SeamsSection::default().to_bytes()).unwrap(),
        SeamsSection::default()
    );
}

#[test]
fn seam_section_round_trips_empty_optionals_and_both_enum_values() {
    let mut first = record();
    first.source_entity = 2;
    first.activation = SeamActivation::Touch;
    first.mode = SeamMode::Classic;
    first.enabled_on_spawn = true;
    first.on_arrive = String::new();
    first.landmark_yaw = 0.0;
    let mut second = record();
    second.name = "other".to_owned();
    let section = SeamsSection {
        seams: vec![first, second],
    };
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.byte_len());
    let decoded = SeamsSection::from_bytes(&bytes).unwrap();
    assert_eq!(decoded, section);
    assert_eq!(decoded.seams[0].on_arrive, "");
    assert_eq!(
        (decoded.seams[0].activation, decoded.seams[0].mode),
        (SeamActivation::Touch, SeamMode::Classic)
    );
    assert_eq!(
        (decoded.seams[1].activation, decoded.seams[1].mode),
        (SeamActivation::Use, SeamMode::Seamless)
    );
}

#[test]
fn seam_section_rejects_every_truncation_and_trailing_bytes() {
    let mut bytes = section().to_bytes();
    for len in 0..bytes.len() {
        assert!(
            SeamsSection::from_bytes(&bytes[..len]).is_err(),
            "length {len}"
        );
    }
    bytes.push(0);
    rejects(&bytes, "trailing");
}

#[test]
fn seam_section_rejects_bad_header_enums_flag_and_brushes() {
    let base = section().to_bytes();
    let mut bad = base.clone();
    bad[0] = 2;
    rejects(&bad, "version");
    let mut bad = base.clone();
    bad[ACTIVATION] = 2;
    rejects(&bad, "unknown activation 2");
    let mut bad = base.clone();
    bad[MODE] = 2;
    rejects(&bad, "unknown mode 2");
    let mut bad = base.clone();
    bad[ENABLED] = 2;
    rejects(&bad, "0 or 1");
    let mut bad = base.clone();
    bad[BRUSH_COUNT..BRUSH_COUNT + 4].copy_from_slice(&0u32.to_le_bytes());
    rejects(&bad, "no brushes");
    let mut bad = base.clone();
    bad[BRUSH_COUNT..BRUSH_COUNT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    rejects(&bad, "remaining");
    let mut bad = base.clone();
    bad[NAME_LEN + 4] = 0xff;
    rejects(&bad, "UTF-8");
    let mut bad = vec![0u8; 6 + (MAX_SEAMS + 1) * MIN_RECORD_SIZE];
    bad[..2].copy_from_slice(&1u16.to_le_bytes());
    bad[2..6].copy_from_slice(&65536u32.to_le_bytes());
    rejects(&bad, "exceed");
}

#[test]
fn seam_section_rejects_nonfinite_landmarks() {
    for mutate in [0, 1] {
        let mut section = section();
        let record = &mut section.seams[0];
        match mutate {
            0 => record.landmark_origin[2] = f32::INFINITY,
            _ => record.landmark_yaw = f32::NAN,
        }
        rejects(&section.to_bytes(), "finite");
    }
    let mut section = section();
    section.seams[0].brushes[0].aabb_min[0] = f32::NAN;
    rejects(&section.to_bytes(), "finite");
}

#[test]
fn seam_section_rejects_empty_name_or_target_and_repeated_names() {
    let mut section = section();
    section.seams[0].name = String::new();
    rejects(&section.to_bytes(), "empty name");
    let mut section = self::section();
    section.seams[0].target = String::new();
    rejects(&section.to_bytes(), "empty target");
    let mut section = self::section();
    let mut twin = record();
    twin.source_entity = 8;
    section.seams.push(twin);
    rejects(&section.to_bytes(), "repeats seam name `pair_door`");
}

#[test]
fn seam_section_rejects_out_of_order_and_repeated_source_entities() {
    for source in [7, 6] {
        let mut section = section();
        let mut next = record();
        next.source_entity = source;
        next.name = "next".to_owned();
        section.seams.push(next);
        rejects(&section.to_bytes(), "map order");
    }
}
