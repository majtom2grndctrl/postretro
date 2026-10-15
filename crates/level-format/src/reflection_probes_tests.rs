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
        distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
        distances: reflection_probe_zero_distances(2, REFLECTION_PROBE_TILE_DIMENSION),
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
    assert!(message.contains("unsupported version 3"), "{message}");
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
    // K sits after version, dimension, border, atlas and distance formats.
    bytes[18..22].copy_from_slice(&5u32.to_le_bytes());
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
fn distance_tiles_round_trip_beside_the_colour_tiles() {
    let mut section = section();
    let texels = (REFLECTION_PROBE_TILE_DIMENSION * REFLECTION_PROBE_TILE_DIMENSION) as usize;
    // f16 bits from 0 (no surface) up through ordinary positive values.
    section.distances = (0..2 * texels)
        .flat_map(|i| ((i % 0x3C00) as u16).to_le_bytes())
        .collect();
    let decoded = ReflectionProbesSection::from_bytes(&section.to_bytes())
        .expect("a section with distances decodes");
    assert_eq!(decoded, section);
    assert_eq!(
        decoded.distance_tile(1),
        &section.distances[2 * texels..4 * texels]
    );
}

#[test]
fn a_version_one_section_is_refused() {
    let mut bytes = section().to_bytes();
    bytes[0..2].copy_from_slice(&1u16.to_le_bytes());
    let message = ReflectionProbesSection::from_bytes(&bytes)
        .expect_err("version 1 predates distance tiles")
        .to_string();
    assert!(message.contains("unsupported version 1"), "{message}");
}

#[test]
fn malformed_distance_payloads_are_refused() {
    let mut format = section();
    format.distance_format = 7;
    assert!(refusal(&format).contains("unknown distance format 7"));

    let mut negative = section();
    negative.distances[2..4].copy_from_slice(&0xBC00u16.to_le_bytes());
    assert!(refusal(&negative).contains("negative or non-finite value at texel 1"));

    let mut infinite = section();
    infinite.distances[0..2].copy_from_slice(&0x7C00u16.to_le_bytes());
    assert!(refusal(&infinite).contains("negative or non-finite value at texel 0"));

    let mut short = section().to_bytes();
    short.truncate(short.len() - 2);
    let message = ReflectionProbesSection::from_bytes(&short)
        .expect_err("a short distance payload is refused")
        .to_string();
    assert!(message.contains("truncated distance payload"), "{message}");
}

#[test]
fn a_tile_payload_of_the_wrong_length_is_refused() {
    // Cut inside the colour tiles, ahead of the distance payload.
    let full = section();
    let mut short = full.to_bytes();
    short.truncate(short.len() - full.distances.len() - 1);
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

// Constant assertion: the cap fits the portable array-layer floor. The share
// table's GPU layout lives with the fluid pass in render-cpu
// (`fluid_surface::reflection`), which asserts its real size at this cap fits
// one 64 KiB uniform binding.
const _: () = assert!(MAX_REFLECTION_PROBES <= 256);

fn unit(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / len)
}

#[test]
fn lobe_weights_sum_to_one_and_peak_on_their_axes() {
    let axes = [
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    for (lobe, axis) in axes.iter().enumerate() {
        let weights = reflection_probe_lobe_weights(*axis);
        for (other, w) in weights.iter().enumerate() {
            let expected = if other == lobe { 1.0 } else { 0.0 };
            assert!((w - expected).abs() < 1e-6, "lobe {lobe}: {weights:?}");
        }
    }
    for step in 0..2000 {
        let t = step as f32 * 0.0137;
        let d = unit([t.sin() * 1.3, (t * 0.7).cos(), (t * 1.9).sin() - 0.2]);
        let weights = reflection_probe_lobe_weights(d);
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "{d:?}: {weights:?}");
        assert!(
            weights.iter().all(|w| (0.0..=1.0).contains(w)),
            "{weights:?}"
        );
    }
}

/// Half weight 45° off an axis: the pinned exponent is 2.
#[test]
fn lobe_weights_split_evenly_between_neighbouring_axes() {
    let w = reflection_probe_lobe_weights(unit([1.0, 1.0, 0.0]));
    assert!(
        (w[0] - 0.5).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6,
        "{w:?}"
    );
}

#[test]
fn straight_down_gives_the_horizontal_lobes_equal_weight_and_its_neighbours_agree() {
    let down = reflection_probe_lobe_weights([0.0, -1.0, 0.0]);
    assert_eq!(down, [0.0, 0.25, 0.25, 0.25, 0.25]);
    // Continuity at the pole: a direction a hair off straight down reads
    // almost the same weights, whichever way it leans.
    for lean in [[1e-3, 0.0], [-1e-3, 0.0], [0.0, 1e-3], [7e-4, -7e-4]] {
        let near = reflection_probe_lobe_weights(unit([lean[0], -1.0, lean[1]]));
        for (a, b) in near.iter().zip(&down) {
            assert!((a - b).abs() < 1e-5, "{near:?}");
        }
    }
}

/// P7's premise: adjacent directions read adjacent weights everywhere,
/// across every lobe edge and the horizon.
#[test]
fn lobe_weights_are_continuous_across_lobe_edges() {
    let step = 1e-3f32;
    for i in 0..4000 {
        let theta = i as f32 * std::f32::consts::TAU / 4000.0;
        for elevation in [-0.9f32, -0.3, 0.0, 0.2, 0.7, 1.4] {
            let at = |t: f32, e: f32| [e.cos() * t.cos(), e.sin(), e.cos() * t.sin()];
            let a = reflection_probe_lobe_weights(at(theta, elevation));
            let b = reflection_probe_lobe_weights(at(theta + step, elevation));
            let c = reflection_probe_lobe_weights(at(theta, elevation + step));
            for lobe in 0..REFLECTION_PROBE_LOBE_COUNT {
                assert!(
                    (a[lobe] - b[lobe]).abs() < 4.0 * step,
                    "{theta} {elevation}"
                );
                assert!(
                    (a[lobe] - c[lobe]).abs() < 4.0 * step,
                    "{theta} {elevation}"
                );
            }
        }
    }
}

#[test]
fn lobe_weights_never_return_nan() {
    for d in [[0.0; 3], [f32::NAN; 3], [f32::INFINITY, 0.0, 0.0]] {
        let w = reflection_probe_lobe_weights(d);
        assert!(w.iter().all(|v| v.is_finite()), "{d:?}: {w:?}");
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6, "{d:?}: {w:?}");
    }
}
