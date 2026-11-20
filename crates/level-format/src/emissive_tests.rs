use super::*;

fn volume(faces: &[u32]) -> EmissiveVolumeRecord {
    EmissiveVolumeRecord {
        min: 0.25,
        max: 2.0,
        color: [1.0, 0.5, 0.0],
        origin: [1.0, -2.0, 3.5],
        faces: faces.to_vec(),
        tags: vec!["bar_sign".into(), "lobby".into()],
    }
}

fn section() -> EmissiveSection {
    EmissiveSection {
        textures: vec![
            EmissiveTexture {
                strength: 4.0,
                glow_color: [0.43, 0.2, 0.9],
            },
            EmissiveTexture::DARK,
            EmissiveTexture {
                strength: EMISSIVE_VALUE_CEILING,
                glow_color: [1.0; 3],
            },
        ],
        volumes: vec![volume(&[0, 3, 7]), volume(&[]), volume(&[1, 9])],
    }
}

fn refused(section: &EmissiveSection) -> String {
    EmissiveSection::from_bytes(&section.to_bytes())
        .expect_err("the loader must refuse this section")
        .to_string()
}

#[test]
fn emissive_section_round_trips_textures_and_volumes() {
    let original = section();
    let bytes = original.to_bytes();
    assert_eq!(bytes.len(), original.byte_len());
    assert_eq!(EmissiveSection::from_bytes(&bytes).unwrap(), original);

    // The defaults a map with no volume writes round-trip too.
    let dark = EmissiveSection::dark(2);
    assert_eq!(EmissiveSection::from_bytes(&dark.to_bytes()).unwrap(), dark);
    let empty = EmissiveSection::default();
    assert_eq!(
        EmissiveSection::from_bytes(&empty.to_bytes()).unwrap(),
        empty
    );
}

#[test]
fn emissive_section_lays_out_the_wire_format_little_endian() {
    let bytes = EmissiveSection {
        textures: vec![EmissiveTexture {
            strength: 4.0,
            glow_color: [0.5, 0.25, 0.0],
        }],
        volumes: vec![EmissiveVolumeRecord {
            min: 0.0,
            max: 1.0,
            color: [1.0; 3],
            origin: [0.0; 3],
            faces: vec![5],
            tags: vec!["a".into()],
        }],
    }
    .to_bytes();
    let f32_at = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 1);
    assert_eq!(u32_at(2), 1, "texture count");
    assert_eq!(f32_at(6), 4.0, "strength");
    assert_eq!([f32_at(10), f32_at(14), f32_at(18)], [0.5, 0.25, 0.0]);
    assert_eq!(u32_at(22), 1, "volume count");
    assert_eq!([f32_at(26), f32_at(30)], [0.0, 1.0], "min, max");
    assert_eq!(u32_at(58), 1, "face count");
    assert_eq!(u32_at(62), 5, "face");
    assert_eq!(u32_at(66), 1, "tag count");
    assert_eq!(u32_at(70), 1, "tag length");
    assert_eq!(&bytes[74..], b"a");
}

#[test]
fn emissive_section_refuses_another_version() {
    for version in [0u16, 2, u16::MAX] {
        let mut bytes = section().to_bytes();
        bytes[0..2].copy_from_slice(&version.to_le_bytes());
        let error = EmissiveSection::from_bytes(&bytes).unwrap_err().to_string();
        assert!(error.contains("version"), "{error}");
    }
}

#[test]
fn emissive_section_refuses_more_volumes_than_the_cap() {
    let at_cap = EmissiveSection {
        textures: Vec::new(),
        volumes: (0..MAX_EMISSIVE_VOLUMES as u32)
            .map(|face| volume(&[face]))
            .collect(),
    };
    assert!(EmissiveSection::from_bytes(&at_cap.to_bytes()).is_ok());
    let mut over = at_cap.clone();
    over.volumes.push(volume(&[MAX_EMISSIVE_VOLUMES as u32]));
    assert!(refused(&over).contains("cap"));
}

#[test]
fn emissive_section_refuses_faces_out_of_order_repeated_or_owned_twice() {
    let mut descending = section();
    descending.volumes[0].faces = vec![3, 1];
    assert!(refused(&descending).contains("ascend"));
    let mut repeated = section();
    repeated.volumes[0].faces = vec![3, 3];
    assert!(refused(&repeated).contains("ascend"));
    let mut shared = section();
    shared.volumes[2].faces = vec![7];
    assert!(refused(&shared).contains("already owned"));
}

#[test]
fn emissive_section_refuses_values_the_compiler_never_writes() {
    type Corruption = fn(&mut EmissiveSection);
    let cases: [(&str, Corruption); 11] = [
        ("min above max", |s| {
            s.volumes[0].min = 3.0;
            s.volumes[0].max = 2.0;
        }),
        ("negative min", |s| s.volumes[0].min = -0.5),
        ("non-finite max", |s| s.volumes[0].max = f32::NAN),
        ("max past the ceiling", |s| {
            s.volumes[0].max = EMISSIVE_VALUE_CEILING * 2.0
        }),
        ("colour channel above 1", |s| s.volumes[0].color[1] = 1.5),
        ("negative colour channel", |s| s.volumes[0].color[2] = -0.1),
        ("non-finite origin", |s| {
            s.volumes[1].origin[0] = f32::INFINITY
        }),
        ("negative strength", |s| s.textures[0].strength = -1.0),
        ("non-finite strength", |s| s.textures[0].strength = f32::NAN),
        ("strength past the ceiling", |s| {
            s.textures[2].strength = EMISSIVE_VALUE_CEILING + 1.0
        }),
        ("glow colour above 1", |s| {
            s.textures[0].glow_color[0] = 1.01
        }),
    ];
    for (case, corrupt) in cases {
        let mut bad = section();
        corrupt(&mut bad);
        assert!(
            EmissiveSection::from_bytes(&bad.to_bytes()).is_err(),
            "{case} must be refused"
        );
    }
}

#[test]
fn emissive_section_refuses_truncated_trailing_and_oversized_counts() {
    let bytes = section().to_bytes();
    for cut in [0, 1, 5, bytes.len() / 2, bytes.len() - 1] {
        assert!(
            EmissiveSection::from_bytes(&bytes[..cut]).is_err(),
            "cut at {cut}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(
        EmissiveSection::from_bytes(&trailing)
            .unwrap_err()
            .to_string()
            .contains("trailing")
    );
    // A texture count no remaining bytes can hold is refused before allocating.
    let mut huge = bytes;
    huge[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(EmissiveSection::from_bytes(&huge).is_err());
}

#[test]
fn emissive_section_must_match_the_level_texture_and_face_counts() {
    let decoded = EmissiveSection::from_bytes(&section().to_bytes()).unwrap();
    assert!(decoded.validate_against_level(3, 10).is_ok());
    let error = decoded
        .validate_against_level(4, 10)
        .unwrap_err()
        .to_string();
    assert!(error.contains("texture"), "{error}");
    let error = decoded
        .validate_against_level(3, 9)
        .unwrap_err()
        .to_string();
    assert!(error.contains("face 9"), "{error}");
}

#[test]
fn texture_strength_reads_zero_past_the_table() {
    let decoded = section();
    assert_eq!(decoded.texture_strength(0), 4.0);
    assert_eq!(decoded.texture_strength(1), 0.0);
    assert_eq!(decoded.texture_strength(99), 0.0);
}
