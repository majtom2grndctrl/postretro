//! Synthetic ownership baselines for layer-close and cut-carry lifetime pins.
use super::*;

// Frozen before production edits at 7e7614d36.
fn expected_pin(label: &str) -> (&'static str, &'static str) {
    match label {
        "one-layer" => (
            "5d4b0a3657c6dbbd327ff4b0d83d5b1a7daf73e496167ecbc3192ace7389f856",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "three-layers" => (
            "2f425ea1e66b10a17e5daddedccec005ed06098c9a80655b9fcc3deefdb7f007",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "dark-middle-layer" => (
            "c706c93112c32431d6b806171a5a202398999a4753e02d2cb04cfe6dac1479ee",
            "5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800",
        ),
        "cut-same-layer" => (
            "702f29d09be12b267380de12199b4384bc4a18eec31c69244f63c5342b0af0ec",
            "0f272f137ea21210d2692c8680b9eb0db62508e8e9573a3e41a339c730ded9cb",
        ),
        "cut-first-to-last" => (
            "aaa684851e03d6d07fff775393bd73366a3876ec07884b75bde196ae2e66b59b",
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
    let bytes = section
        .as_ref()
        .map_or_else(Vec::new, ShadowmaskAtlasSection::to_bytes);
    let report_text = format!("{report:?}");
    let expected = expected_pin(label);
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
    println!(
        "PIN_BASELINE {label} id42={} owners={:?} report={} report_digest={}",
        blake3::hash(&bytes).to_hex(),
        section.as_ref().map(|section| &section.owners),
        report_text,
        blake3::hash(report_text.as_bytes()).to_hex()
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
