use super::*;
use crate::SectionId;

fn share(roster_index: u32) -> ReflectionProbeLightShare {
    let mut shares = [[0.0; 3]; REFLECTION_PROBE_LOBE_COUNT];
    for (l, lobe) in shares.iter_mut().enumerate() {
        *lobe = [0.1 * l as f32, 0.5, 1.0];
    }
    ReflectionProbeLightShare {
        roster_index,
        reference: [2.0, 1.0, 0.0],
        shares,
    }
}

fn probe(source_entity: u32, layer: u32) -> ReflectionProbeRecord {
    let mut lights = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
    lights[0] = share(3);
    lights[1] = share(0);
    ReflectionProbeRecord {
        source_entity,
        box_min: [-4.0, -1.0, -4.0],
        box_max: [4.0, 3.0, 4.0],
        capture_point: [0.5, -0.998, 0.25],
        layer,
        lights,
    }
}

fn section() -> ReflectionProbesSection {
    let tile = reflection_probe_tile_bytes(IRRADIANCE_FORMAT_BC6H, REFLECTION_PROBE_TILE_DIMENSION)
        .expect("BC6H is a known format");
    ReflectionProbesSection {
        tile_dimension: REFLECTION_PROBE_TILE_DIMENSION,
        tile_border: REFLECTION_PROBE_TILE_BORDER,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        probes: vec![probe(7, 1), probe(2, 0)],
        bindings: vec![
            ReflectionProbeBinding {
                volume: 0,
                face: 2,
                probe: 1,
            },
            ReflectionProbeBinding {
                volume: 3,
                face: 0,
                probe: 0,
            },
            ReflectionProbeBinding {
                volume: 3,
                face: 1,
                probe: 0,
            },
        ],
        tiles: (0..2 * tile).map(|i| (i % 251) as u8).collect(),
    }
}

fn refusal(section: &ReflectionProbesSection) -> String {
    ReflectionProbesSection::from_bytes(&section.to_bytes())
        .expect_err("section must be refused")
        .to_string()
}

#[test]
fn reflection_probes_section_is_registered_as_id_54() {
    assert_eq!(SectionId::from_u32(54), Some(SectionId::ReflectionProbes));
    assert_eq!(SectionId::ReflectionProbes as u32, 54);
}

#[test]
fn reflection_probes_round_trip_every_field_and_tile() {
    let section = section();
    let bytes = section.to_bytes();
    assert_eq!(bytes.len(), section.byte_len());
    let decoded = ReflectionProbesSection::from_bytes(&bytes).expect("valid section decodes");
    assert_eq!(decoded, section);
    assert_eq!(decoded.tile(1), &section.tiles[16384..32768]);
}

#[test]
fn a_bc6h_capture_tile_at_the_engine_size_is_16_kib() {
    assert_eq!(
        reflection_probe_tile_bytes(IRRADIANCE_FORMAT_BC6H, REFLECTION_PROBE_TILE_DIMENSION),
        Some(16 * 1024)
    );
    assert_eq!(
        reflection_probe_tile_bytes(IRRADIANCE_FORMAT_RGBA16F, 4),
        Some(128)
    );
    assert_eq!(reflection_probe_tile_bytes(9, 128), None);
}

#[test]
fn a_share_entry_holding_the_reserved_value_loads() {
    let mut section = section();
    section.probes[0].lights =
        [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
    let decoded = ReflectionProbesSection::from_bytes(&section.to_bytes()).expect("reserved loads");
    assert!(
        decoded.probes[0]
            .lights
            .iter()
            .all(|light| !light.is_used())
    );
}

#[test]
fn a_later_version_is_refused_naming_the_section() {
    let mut bytes = section().to_bytes();
    bytes[0..2].copy_from_slice(&(REFLECTION_PROBES_VERSION + 1).to_le_bytes());
    let message = ReflectionProbesSection::from_bytes(&bytes)
        .expect_err("later version is refused")
        .to_string();
    assert!(message.contains("reflection probes"), "{message}");
    assert!(message.contains("unsupported version 2"), "{message}");
}

#[test]
fn a_binding_past_the_probe_count_is_refused() {
    let mut section = section();
    section.bindings[2].probe = 2;
    assert!(refusal(&section).contains("names probe 2, past the 2 probes"));
}

#[test]
fn a_face_bound_twice_is_refused() {
    let mut section = section();
    section.bindings[2].face = 0;
    assert!(refusal(&section).contains("volume 3 face 0 is bound twice"));
}

#[test]
fn bindings_out_of_canonical_order_are_refused() {
    let mut section = section();
    section.bindings.swap(0, 1);
    assert!(refusal(&section).contains("is out of order"));
}

#[test]
fn malformed_headers_are_refused() {
    let mut zero_probes = section();
    zero_probes.probes.clear();
    zero_probes.bindings.clear();
    zero_probes.tiles.clear();
    assert!(refusal(&zero_probes).contains("no probes"));

    let mut border = section();
    border.tile_border = 0;
    assert!(refusal(&border).contains("tile border 0"));

    let mut dimension = section();
    dimension.tile_dimension = 130;
    assert!(refusal(&dimension).contains("tile dimension 130"));

    let mut format = section();
    format.atlas_format = 9;
    assert!(refusal(&format).contains("unknown atlas format 9"));

    let mut bytes = section().to_bytes();
    // K sits after version, dimension, border and format.
    bytes[14..18].copy_from_slice(&5u32.to_le_bytes());
    let message = ReflectionProbesSection::from_bytes(&bytes)
        .expect_err("another K is refused")
        .to_string();
    assert!(message.contains("5 lights × 5 lobes"), "{message}");
}

#[test]
fn malformed_probes_are_refused() {
    let mut layer = section();
    layer.probes[1].layer = 1;
    assert!(refusal(&layer).contains("probe 1 reuses layer 1"));

    let mut past = section();
    past.probes[0].layer = 2;
    assert!(refusal(&past).contains("layer 2 is past the 2 layers"));

    let mut outside = section();
    outside.probes[0].capture_point[1] = -1.5;
    assert!(refusal(&outside).contains("capture point lies outside its box"));

    let mut inverted = section();
    inverted.probes[0].box_min[0] = 5.0;
    assert!(refusal(&inverted).contains("box min exceeds max"));

    let mut share_range = section();
    share_range.probes[0].lights[0].shares[2][1] = 1.5;
    assert!(refusal(&share_range).contains("lobe 2 share lies outside 0..=1"));

    let mut stale = section();
    stale.probes[0].lights[3].reference = [1.0; 3];
    assert!(refusal(&stale).contains("light 3 is unused but carries values"));
}

#[test]
fn a_tile_payload_of_the_wrong_length_is_refused() {
    let mut short = section().to_bytes();
    short.pop();
    let message = ReflectionProbesSection::from_bytes(&short)
        .expect_err("short payload is refused")
        .to_string();
    assert!(message.contains("truncated tile payload"), "{message}");

    let mut long = section().to_bytes();
    long.push(0);
    let message = ReflectionProbesSection::from_bytes(&long)
        .expect_err("trailing bytes are refused")
        .to_string();
    assert!(message.contains("trailing bytes"), "{message}");
}

// Constant assertions: the cap fits the portable array-layer floor, and the
// fluid shader's share table at the cap fits one 64 KiB uniform binding. The
// shader packs each entry as a vec4 (RGB reference, roster index) plus the
// lobes' RGB shares as unorm10×3, one u32 per lobe, padded to whole vec4s.
const SHARE_ENTRY_GPU_BYTES: usize = 16 + REFLECTION_PROBE_LOBE_COUNT.div_ceil(4) * 16;
const SHARE_TABLE_GPU_BYTES: usize =
    MAX_REFLECTION_PROBES * REFLECTION_PROBE_LIGHTS_PER_PROBE * SHARE_ENTRY_GPU_BYTES;
const _: () = assert!(MAX_REFLECTION_PROBES <= 256);
const _: () = assert!(SHARE_TABLE_GPU_BYTES <= 64 * 1024);
