//! Synthetic ownership baselines for layer-close and cut-carry lifetime pins.
use super::*;

// Frozen before production edits at 7e7614d36. The id 42 digests were recaptured
// for `SMB8`, after these same rows passed with the old tag and every plane stored,
// and again for `SMB9`, after `assert_masks_are_translations` showed every owned
// chart's mask texels byte-equal to its lightmap-layout fill; owners and drop
// reports kept their digests. Coded planes are pinned decoded (stored raw), so
// the digests hold the texels, not a zstd build's frames.
fn expected_pin(label: &str) -> (&'static str, &'static str) {
    match label {
        "one-layer" => (
            "51e264dcd633b3cea1d9970025eba6dcce2744a55642de3b2513c7120fb1f574",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "three-layers" => (
            "20e35c8a5e46472ebccc67aa7db54be032a08232ae160c6bb07cb0310e1fbbd5",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "dark-middle-layer" => (
            "5f8f8b8ebbea7ff5690f186cbab7b6c13ca32f4d4c189ae380a3d4c67b0dd251",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "cut-same-layer" => (
            "fa2a5765e92b0865904f75459cc3b7f37c5fb08fea01163191ffafc823dd8fdc",
            "0f272f137ea21210d2692c8680b9eb0db62508e8e9573a3e41a339c730ded9cb",
        ),
        "cut-first-to-last" => (
            "2a20613d8a8a60a253964642d98401c48ab5067fc9b393c1182338ff2134438f",
            "0f272f137ea21210d2692c8680b9eb0db62508e8e9573a3e41a339c730ded9cb",
        ),
        "cut-owners-baked-nothing" => (
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            "b341720f2ad435043532219c2bb700e304301d7cdabf20839afb3db9fd2a3600",
        ),
        _ => panic!("missing pre-change ownership baseline: {label}"),
    }
}

fn golden(label: &str, section: Option<ShadowmaskAtlasSection>, report: OwnerReport) {
    let bytes = section.as_ref().map_or_else(Vec::new, |section| {
        section
            .decoded()
            .expect("the bake's coded planes decode")
            .to_bytes()
    });
    let report_text = format!("{report:?}");
    let expected = expected_pin(label);
    println!(
        "PIN_BASELINE {label} id42={} owners={:?} report={} report_digest={}",
        blake3::hash(&bytes).to_hex(),
        section.as_ref().map(|section| &section.owners),
        report_text,
        blake3::hash(report_text.as_bytes()).to_hex()
    );
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        expected.0,
        "{label}: id 42 and owners"
    );
    assert_eq!(
        blake3::hash(report_text.as_bytes()).to_hex().as_str(),
        expected.1,
        "{label}: owner drop report"
    );
}

fn no_cut(layers: u32, dark_middle: bool) -> (Option<ShadowmaskAtlasSection>, OwnerReport) {
    let mut atlas = Atlas::new(layers as usize);
    atlas.width = CHART;
    atlas.layer_count = layers;
    atlas.layout.blocks = (0..layers)
        .map(|layer| CellBlock {
            cell_id: layer,
            width: CHART,
            height: CHART,
            layer,
            x: 0,
            y: 0,
        })
        .collect();
    atlas.layout.chart_blocks = (0..layers).collect();
    for (face, placement) in atlas.placements.iter_mut().enumerate() {
        placement.x = 0;
        placement.layer = face as u32;
    }
    let light = light_over(0, 2.0);
    let owner = eligible(&light, 0, false);
    let shared = atlas.shared();
    let mut fill = OwnershipFill::new(&shared, layers, &[], vec![], None);
    let probe = fill.probe.clone();
    for layer in 0..layers {
        let visibility = if dark_middle && layer == 1 { 0.0 } else { 1.0 };
        let mut item = partition(&atlas, atlas.lit(layer as usize, 16, visibility));
        item.target_layer = layer;
        // Ordered consumption can migrate workers between every item.
        std::thread::scope(|scope| {
            scope
                .spawn(|| fill.consume(&owner, &item, &shared))
                .join()
                .unwrap()
        });
    }
    let result = fill.finish(&shared);
    if let Some(section) = &result.0 {
        super::assert_masks_are_translations(&shared, section);
    }
    assert_eq!(
        probe.peak(),
        (CHART * CHART * 4) as usize,
        "one raw layer across all workers"
    );
    assert_eq!(
        probe.closed_layers(),
        layers as usize,
        "each layer closes once"
    );
    assert_eq!(probe.live(), 0, "return releases all raw bytes");
    result
}

fn cut(spans_layers: bool, all_dark: bool) -> (Option<ShadowmaskAtlasSection>, OwnerReport) {
    cut_dumped(spans_layers, all_dark, None)
}

fn cut_dumped(
    spans_layers: bool,
    all_dark: bool,
    dump: Option<crate::chart_dump::SharedDumpWriter>,
) -> (Option<ShadowmaskAtlasSection>, OwnerReport) {
    let mut fixture = CutAtlas::new();
    let layers = if spans_layers { 4 } else { 1 };
    fixture.atlas.layer_count = layers;
    if spans_layers {
        fixture.atlas.placements[1].layer = 3;
        fixture.atlas.placements[2].layer = 1;
        fixture.atlas.layout.blocks = (0..layers)
            .map(|layer| CellBlock {
                cell_id: layer,
                width: fixture.atlas.width,
                height: CHART,
                layer,
                x: 0,
                y: 0,
            })
            .collect();
        fixture.atlas.layout.chart_blocks = vec![0, 3, 1];
    }
    let lights: Vec<_> = (0..4).map(|_| light_over(0, 2.0)).collect();
    let eligible: Vec<_> = [3, 1, 2, 4]
        .into_iter()
        .zip(&lights)
        .map(|(index, light)| super::eligible(light, index, false))
        .collect();
    let mut channels = ownership::FaceChannels::default();
    for light in &eligible[..3] {
        channels.offer(ownership::Candidate {
            spec_index: light.spec_index,
            lit_texels: LIT_TEXEL_FLOOR,
            contribution: 1.0,
            promoted: false,
        });
    }
    let shared = fixture.atlas.shared();
    let mut fill = OwnershipFill::new(
        &shared,
        layers,
        std::slice::from_ref(&fixture.cut),
        vec![channels],
        None,
    );
    fill.chart_dump = dump.map(crate::chart_dump::capture::MaskCapture::new);
    let probe = fill.probe.clone();
    for layer in 0..layers {
        for (arrival, light) in eligible.iter().enumerate() {
            let visibility = if all_dark || arrival == 2 {
                0.0
            } else if arrival == 1 {
                0.5
            } else {
                1.0
            };
            let mut texels = Vec::new();
            for face in 0..2 {
                if fixture.atlas.placements[face].layer != layer {
                    continue;
                }
                let origin = [0, 4][face];
                for gy in 0..INTERIOR {
                    for gx in origin..origin + SUB_W {
                        texels.push((fixture.idx(face, gx, gy), visibility));
                    }
                }
            }
            if fixture.atlas.placements[2].layer == layer && arrival == 0 {
                texels.extend(fixture.atlas.lit(2, 16, visibility));
            }
            let mut item = partition(&fixture.atlas, texels);
            item.target_layer = layer;
            fill.consume(light, &item, &shared);
        }
    }
    let result = fill.finish(&shared);
    if let Some(section) = &result.0 {
        // P10: a carried block's layout is fixed after its last cut face.
        super::assert_masks_are_translations(&shared, section);
    }
    let layer_bytes = fixture.atlas.width as usize * CHART as usize * 4;
    let carry = if spans_layers { layer_bytes } else { 0 };
    assert!(
        probe.peak() <= layer_bytes + carry,
        "only the first cut block stays raw: peak {} bound {}",
        probe.peak(),
        layer_bytes + carry
    );
    assert_eq!(probe.closed_layers(), layers as usize);
    assert_eq!(probe.live(), 0);
    result
}

#[test]
fn layer_budget_synthetic_pins_match_pre_change() {
    for (label, (section, report)) in [
        ("one-layer", no_cut(1, false)),
        ("three-layers", no_cut(3, false)),
        ("dark-middle-layer", no_cut(3, true)),
        ("cut-same-layer", cut(false, false)),
        ("cut-first-to-last", cut(true, false)),
        ("cut-owners-baked-nothing", cut(true, true)),
    ] {
        golden(label, section, report);
    }
}

#[test]
fn raw_fill_probes_do_not_mix_simultaneous_bakes() {
    std::thread::scope(|scope| {
        let one = scope.spawn(|| no_cut(1, false));
        let three = scope.spawn(|| no_cut(3, false));
        let _ = one.join().unwrap();
        let _ = three.join().unwrap();
    });
}

// The cut's first sub-face's block is carried from layer 0 to the cut's last
// layer: the dump records it once, when the carry is encoded, holding the
// values BC5 encodes; a chart no light owns is not recorded.
#[test]
fn chart_dump_records_a_carried_cut_chart_once_from_its_final_values() {
    use crate::chart_dump::reader::DumpReader;
    use crate::chart_dump::{ChartDumpConfig, DumpKind, SharedDumpWriter};

    let dir = tempfile::tempdir().unwrap();
    let config = ChartDumpConfig {
        dir: dir.path().to_path_buf(),
        texels: true,
    };
    let writer = SharedDumpWriter::create(&config, DumpKind::Mask).unwrap();
    let (section, _) = cut_dumped(true, false, Some(writer.clone()));
    let raw = take_last_raw_fill();
    writer.finish().expect("each chart recorded once");
    let section = section.expect("the cut owns channels");

    let reader = DumpReader::open(dir.path(), DumpKind::Mask).unwrap();
    let owned: Vec<u32> = (0..section.owners.len() as u32)
        .filter(|&face| section.owners[face as usize] != NO_FACE_OWNERS)
        .collect();
    let recorded: Vec<u32> = reader.entries.keys().map(|key| key.face).collect();
    assert_eq!(recorded, owned);
    assert!(recorded.contains(&0), "the carried sub-face is recorded");

    let width = CutAtlas::new().atlas.width as usize;
    let plane = width * CHART as usize * 4;
    for entry in reader.entries.values() {
        let meta = &entry.meta;
        assert_eq!(meta.owners, section.owners[meta.key.face as usize]);
        let texels = reader
            .read_plane(entry, crate::chart_dump::format::PlaneKind::Mask)
            .unwrap()
            .unwrap();
        let p = &meta.placement;
        let mut expected = Vec::new();
        for y in 0..meta.height as usize {
            let start = p.layer as usize * plane
                + ((p.layer_y as usize + y) * width + p.layer_x as usize) * 4;
            expected.extend_from_slice(&raw[start..start + meta.width as usize * 4]);
        }
        assert_eq!(texels, expected, "{}: the values BC5 encodes", meta.key);
    }
}
