// On-demand proof that the lightmap penumbra floor leaves probe-side sections alone, and its encode cost.
// See: context/lib/build_pipeline.md §PRL section IDs
//
// Compile the same map `--no-cache` with a pre-change `prl-build` and with this
// one (floored, or `--no-penumbra-floor`), then:
//   POSTRETRO_PENUMBRA_BASELINE_PRL=<before.prl> POSTRETRO_PENUMBRA_AFTER_PRL=<after.prl> \
//   cargo test -p postretro-level-compiler --bin prl-build penumbra_floor_baseline_probe -- --ignored --nocapture
// asserts the probe sections byte-identical (either floor setting). With
// `penumbra_floor_baseline_weight_maps` instead, it asserts ids 24/25 identical
// (a `--no-penumbra-floor` after, on a map with animated baked lights).
// Both print every other section that differs.
//
// BC6H encode error of one `--uncompressed-irradiance` bake, optionally checked
// against the BC6H bake of the same build:
//   POSTRETRO_PENUMBRA_F16_PRL=<uncompressed.prl> [POSTRETRO_PENUMBRA_BC6H_PRL=<bc6h.prl>] \
//   cargo test -p postretro-level-compiler --bin prl-build penumbra_floor_bc6h -- --ignored --nocapture
//
// BC5 shadowmask encode error and the owner report, floor off vs on, compiled
// in process (optionally checked against CLI bakes `<dir>/<map>.{off,floor}.prl`):
//   [POSTRETRO_PENUMBRA_PRL_DIR=<dir>] \
//   cargo test -p postretro-level-compiler --bin prl-build penumbra_floor_bc5 -- --ignored --nocapture

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::SectionId;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapBlockIndex, LightmapSection,
};
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_OWNER_CHANNELS, SHADOWMASK_OWNER_EMPTY, ShadowmaskAtlasSection,
    ShadowmaskOwnerBounds,
};
use postretro_level_format::{read_container, read_section_data};
use postretro_test_log_capture::{CapturedRecord, LogCapture};

use crate::bc6h::{decode_bc6h_block, encode_bc6h_rgb_from_f32_rgba, f16_bits_to_f32};

const BASELINE_ENV: &str = "POSTRETRO_PENUMBRA_BASELINE_PRL";
const AFTER_ENV: &str = "POSTRETRO_PENUMBRA_AFTER_PRL";
const F16_ENV: &str = "POSTRETRO_PENUMBRA_F16_PRL";
const BC6H_ENV: &str = "POSTRETRO_PENUMBRA_BC6H_PRL";
const PRL_DIR_ENV: &str = "POSTRETRO_PENUMBRA_PRL_DIR";

/// Probe and scatter bakes have no texel, so the floor must not move them:
/// SH base and bounce (34), SH deltas (27), direct SH (35, 41, 45), billboard
/// scatter (47, 48), and the cluster payloads that carry copies of 27/34/35/41/45.
const PROBE_SECTIONS: [SectionId; 8] = [
    SectionId::OctahedralShVolume,
    SectionId::DeltaShVolumes,
    SectionId::DirectShVolume,
    SectionId::DirectShDeltaVolumes,
    SectionId::AnimatedDirectShDeltaVolumes,
    SectionId::BillboardDirectScatterVolume,
    SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
    SectionId::ClusterShPayloads,
];

/// With the floor off, the weight-map bake is the pre-change centre-point bake.
const WEIGHT_MAP_SECTIONS: [SectionId; 2] = [
    SectionId::AnimatedLightChunks,
    SectionId::AnimatedLightWeightMaps,
];

/// Relative-error floor, so unlit texels compare absolutely (as the reseed check).
const IRRADIANCE_REL_FLOOR: f32 = 0.05;

/// The shadowmask fixtures `shadowmask-atlas-compress-at-rest` recorded BC5 error on.
const BC5_FIXTURES: [&str; 3] = [
    "shadowmask-groups-capture",
    "soft_shadow_test",
    "gate-heavily-lit",
];

type Sections = BTreeMap<u32, Vec<u8>>;

fn sections(path: &str) -> Sections {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    meta.sections
        .iter()
        .map(|entry| {
            let data = read_section_data(&mut cursor, &meta, entry.section_id)
                .expect("read_section_data")
                .expect("listed section is present");
            (entry.section_id, data)
        })
        .collect()
}

fn name(id: u32) -> String {
    SectionId::from_u32(id).map_or_else(|| format!("unknown {id}"), |s| format!("{s:?} ({id})"))
}

/// Byte positions that differ over the common length, and both lengths.
fn byte_diff(a: &[u8], b: &[u8]) -> (usize, usize, usize) {
    let differing = a.iter().zip(b).filter(|(x, y)| x != y).count();
    (differing + a.len().abs_diff(b.len()), a.len(), b.len())
}

/// Print every section present in either PRL that is absent or differs, and
/// return the differing ids. The data script embeds its absolute source path,
/// so it counts as equal when only that path differs.
fn report_differences(before: &Sections, after: &Sections) -> BTreeSet<u32> {
    let ids: BTreeSet<u32> = before.keys().chain(after.keys()).copied().collect();
    let script = SectionId::DataScript as u32;
    let mut differing = BTreeSet::new();
    for id in ids {
        match (before.get(&id), after.get(&id)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) if id == script => {
                let code = |bytes: &[u8]| {
                    DataScriptSection::from_bytes(bytes)
                        .expect("id 28 decodes")
                        .compiled_bytes
                };
                if code(a) == code(b) {
                    println!("  {}: source path only", name(id));
                } else {
                    println!("  {}: compiled bytes differ", name(id));
                    differing.insert(id);
                }
            }
            (Some(a), Some(b)) => {
                let (bytes, la, lb) = byte_diff(a, b);
                println!(
                    "  {}: {bytes} differing bytes ({la} -> {lb} bytes)",
                    name(id)
                );
                differing.insert(id);
            }
            (a, b) => {
                println!(
                    "  {}: present before {}, after {}",
                    name(id),
                    a.is_some(),
                    b.is_some()
                );
                differing.insert(id);
            }
        }
    }
    let present: Vec<String> = before.keys().map(|&id| id.to_string()).collect();
    println!("  sections before: {}", present.join(" "));
    differing
}

fn two_prls() -> Option<(String, String)> {
    match (std::env::var(BASELINE_ENV), std::env::var(AFTER_ENV)) {
        (Ok(before), Ok(after)) => Some((before, after)),
        _ => {
            eprintln!("skipped: set {BASELINE_ENV} and {AFTER_ENV} to compare two PRLs");
            None
        }
    }
}

/// Per-face shadowmask owners, empties dropped.
fn face_owners(s: &Sections) -> Vec<Vec<u16>> {
    let Some(id42) = s.get(&(SectionId::ShadowmaskAtlas as u32)) else {
        return Vec::new();
    };
    let id22 = &s[&(SectionId::Lightmap as u32)];
    let index = LightmapBlockIndex::from_prefix(id22, id22.len() as u64).expect("id 22 index");
    let bounds = ShadowmaskOwnerBounds::from_section_header(id42).expect("id 42 header");
    ShadowmaskAtlasSection::from_bytes(id42, &index, bounds)
        .expect("id 42 decodes")
        .owners
        .iter()
        .map(|owners| {
            owners
                .iter()
                .copied()
                .filter(|&o| o != SHADOWMASK_OWNER_EMPTY)
                .collect()
        })
        .collect()
}

/// A static spec light by its id-42 index: the `index`-th non-dynamic AlphaLight.
fn spec_light_label(s: &Sections, index: u16) -> String {
    let lights = AlphaLightsSection::from_bytes(&s[&(SectionId::AlphaLights as u32)])
        .expect("id 18 decodes")
        .lights;
    lights
        .iter()
        .filter(|l| !l.is_dynamic)
        .nth(usize::from(index))
        .map_or_else(
            || format!("spec light {index} (unresolved)"),
            |l| {
                format!(
                    "spec light {index} ({:?} at [{:.2}, {:.2}, {:.2}] m)",
                    l.light_type, l.origin[0], l.origin[1], l.origin[2]
                )
            },
        )
}

/// Print every (face, light) owner pairing lost or gained; return the lost count.
fn report_owner_changes(before: &Sections, after: &Sections) -> usize {
    let (a, b) = (face_owners(before), face_owners(after));
    if a.is_empty() && b.is_empty() {
        println!("  shadowmask owners: no id 42 on either side");
        return 0;
    }
    assert_eq!(a.len(), b.len(), "id 42 face counts differ");
    let (mut lost, mut gained) = (0, 0);
    for (face, (oa, ob)) in a.iter().zip(&b).enumerate() {
        for &light in oa.iter().filter(|l| !ob.contains(l)) {
            println!(
                "  OWNER LOST: face {face} no longer owns {}",
                spec_light_label(before, light)
            );
            lost += 1;
        }
        for &light in ob.iter().filter(|l| !oa.contains(l)) {
            println!(
                "  owner gained: face {face} now owns {}",
                spec_light_label(after, light)
            );
            gained += 1;
        }
    }
    println!(
        "  shadowmask owners: {} faces, {lost} owner(s) lost, {gained} gained",
        a.len()
    );
    lost
}

#[test]
#[ignore = "compares two on-demand PRLs; see the file header"]
fn penumbra_floor_baseline_probe_sections_are_byte_identical() {
    let Some((before_path, after_path)) = two_prls() else {
        return;
    };
    let (before, after) = (sections(&before_path), sections(&after_path));
    println!("{before_path} -> {after_path}");
    let differing = report_differences(&before, &after);
    report_owner_changes(&before, &after);
    let mut failed = Vec::new();
    for section in PROBE_SECTIONS {
        let id = section as u32;
        let state = match (before.contains_key(&id), differing.contains(&id)) {
            (false, false) => "absent on both sides",
            (true, false) => "identical",
            (_, true) => {
                failed.push(name(id));
                "DIFFERS"
            }
        };
        println!("  probe {}: {state}", name(id));
    }
    assert!(
        failed.is_empty(),
        "probe-side sections changed: {}",
        failed.join(", ")
    );
}

#[test]
#[ignore = "compares two on-demand PRLs; see the file header"]
fn penumbra_floor_baseline_weight_maps_are_byte_identical() {
    let Some((before_path, after_path)) = two_prls() else {
        return;
    };
    let (before, after) = (sections(&before_path), sections(&after_path));
    println!("{before_path} -> {after_path}");
    let differing = report_differences(&before, &after);
    println!(
        "  every section identical: {}",
        if differing.is_empty() { "yes" } else { "no" }
    );
    for section in WEIGHT_MAP_SECTIONS {
        let id = section as u32;
        assert!(
            before.contains_key(&id),
            "{} absent: bake a map with animated baked lights",
            name(id)
        );
        assert!(!differing.contains(&id), "{} changed", name(id));
        println!("  weight maps {}: identical", name(id));
    }
}

/// RGB per texel of one block, row-major, as stored.
fn block_rgb(section: &LightmapSection, block: usize) -> Vec<[f32; 3]> {
    let b = &section.blocks[block];
    let (w, h) = (usize::from(b.width), usize::from(b.height));
    let mut texels = vec![[0.0; 3]; w * h];
    if section.irradiance_format == IRRADIANCE_FORMAT_RGBA16F {
        for (texel, bytes) in texels.iter_mut().zip(b.irradiance.as_chunks::<8>().0) {
            for (c, value) in texel.iter_mut().enumerate() {
                *value = f16_bits_to_f32(u16::from_le_bytes([bytes[2 * c], bytes[2 * c + 1]]));
            }
        }
        return texels;
    }
    decode_bc6h(&b.irradiance, w, &mut texels);
    texels
}

fn decode_bc6h(payload: &[u8], w: usize, texels: &mut [[f32; 3]]) {
    for (bi, encoded) in payload.as_chunks::<16>().0.iter().enumerate() {
        let (bx, by) = ((bi % (w / 4)) * 4, (bi / (w / 4)) * 4);
        let decoded = decode_bc6h_block(encoded).expect("the bake writes Mode 11");
        for (k, texel) in decoded.into_iter().enumerate() {
            texels[(by + k / 4) * w + bx + k % 4] = texel;
        }
    }
}

fn lightmap(s: &Sections) -> LightmapSection {
    LightmapSection::from_bytes(&s[&(SectionId::Lightmap as u32)]).expect("id 22 decodes")
}

#[test]
#[ignore = "measures one on-demand uncompressed PRL; see the file header"]
fn penumbra_floor_bc6h_irradiance_encode_error() {
    let Ok(f16_path) = std::env::var(F16_ENV) else {
        eprintln!("skipped: set {F16_ENV} to an --uncompressed-irradiance PRL");
        return;
    };
    let exact = lightmap(&sections(&f16_path));
    assert_eq!(exact.irradiance_format, IRRADIANCE_FORMAT_RGBA16F);
    let shipped = std::env::var(BC6H_ENV).ok().map(|path| {
        let section = lightmap(&sections(&path));
        assert_eq!(section.irradiance_format, IRRADIANCE_FORMAT_BC6H);
        assert_eq!(
            section.blocks.len(),
            exact.blocks.len(),
            "block counts differ"
        );
        section
    });

    let (mut samples, mut sum_abs, mut sum_rel, mut max_abs, mut max_rel) =
        (0u64, 0.0f64, 0.0f64, 0.0f32, 0.0f32);
    let (mut lit_samples, mut lit_sum_abs) = (0u64, 0.0f64);
    let mut over_tolerance = 0u64;
    let (mut blocks_matching, mut blocks) = (0usize, 0usize);
    for (index, block) in exact.blocks.iter().enumerate() {
        let (w, h) = (usize::from(block.width), usize::from(block.height));
        let source = block_rgb(&exact, index);
        let rgba: Vec<f32> = source
            .iter()
            .flat_map(|t| [t[0], t[1], t[2], 1.0])
            .collect();
        let encoded = encode_bc6h_rgb_from_f32_rgba(&rgba, w as u32, h as u32);
        if let Some(shipped) = &shipped {
            blocks += 1;
            blocks_matching += usize::from(shipped.blocks[index].irradiance == encoded);
        }
        let mut decoded = vec![[0.0f32; 3]; w * h];
        decode_bc6h(&encoded, w, &mut decoded);
        for (a, b) in source.iter().zip(&decoded) {
            for c in 0..3 {
                let abs = (a[c] - b[c]).abs();
                let rel = abs / a[c].max(IRRADIANCE_REL_FLOOR);
                samples += 1;
                sum_abs += f64::from(abs);
                sum_rel += f64::from(rel);
                max_abs = max_abs.max(abs);
                max_rel = max_rel.max(rel);
                over_tolerance += u64::from(rel > crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL);
                if a[c] > 0.0 {
                    lit_samples += 1;
                    lit_sum_abs += f64::from(abs);
                }
            }
        }
    }
    println!(
        "{f16_path}: {} blocks, {samples} channel samples ({lit_samples} non-zero)",
        exact.blocks.len()
    );
    println!(
        "  BC6H abs error max {max_abs:.5}, mean {:.6} (mean over non-zero {:.6})",
        sum_abs / samples.max(1) as f64,
        lit_sum_abs / lit_samples.max(1) as f64
    );
    println!(
        "  BC6H rel error (floor {IRRADIANCE_REL_FLOOR}) max {max_rel:.5}, mean {:.6}; {over_tolerance} samples past the {} round-trip tolerance",
        sum_rel / samples.max(1) as f64,
        crate::bc6h::BC6H_ROUNDTRIP_TOLERANCE_REL
    );
    if shipped.is_some() {
        println!("  re-encode equals the shipped BC6H block: {blocks_matching} of {blocks}");
        assert_eq!(
            blocks_matching, blocks,
            "the BC6H PRL is not this bake's encoding"
        );
    }
}

/// Raw shadowmask fill, captured log, and output sections of one in-process compile.
struct InProcessBake {
    raw: Vec<u8>,
    logs: Vec<CapturedRecord>,
    sections: Sections,
    prl: PathBuf,
}

/// Compile `map` through the production pipeline on one worker, so the
/// shadowmask fill and every log line land on the thread that reads them.
fn compile_in_process(map: &str, floor: bool, out_dir: &Path) -> InProcessBake {
    let prl = out_dir.join(format!("{map}.{}.prl", if floor { "floor" } else { "off" }));
    let baked = out_dir.join("baked");
    std::fs::create_dir_all(baked.join("materials")).expect("baked root");
    let mut argv = vec![
        crate::fixture_pipeline::fixture_path(map)
            .to_str()
            .expect("UTF-8 path")
            .to_owned(),
        "--no-cache".to_owned(),
        "--verbose".to_owned(),
        "-o".to_owned(),
        prl.to_str().expect("UTF-8 path").to_owned(),
        "--baked-root".to_owned(),
        baked.to_str().expect("UTF-8 path").to_owned(),
    ];
    if !floor {
        argv.push("--no-penumbra-floor".to_owned());
    }
    let args = crate::parse_args_from(argv.into_iter()).expect("arguments parse");
    let (raw, logs) = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("one-worker pool")
        .install(|| {
            let capture = LogCapture::start();
            let started = Instant::now();
            let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
                crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
            );
            crate::pipeline::run(
                &args,
                None,
                started,
                reporter,
                Arc::new(crate::governor::Governor::new(1, false)),
            )
            .expect("map compiles");
            let raw = crate::shadowmask_bake::take_last_raw_fill_if_any();
            (raw, capture.records())
        });
    let sections = sections(prl.to_str().expect("UTF-8 path"));
    InProcessBake {
        raw,
        logs,
        sections,
        prl,
    }
}

/// `(layers, edge)` from the verbose peak line "... on L bake layers of E²)".
fn bake_layers(logs: &[CapturedRecord]) -> (usize, usize) {
    let line = logs
        .iter()
        .map(|r| r.message.as_str())
        .find(|m| m.contains(" bake layers of "))
        .expect("the verbose lightmap peak line names the bake layers");
    let (head, tail) = line.split_once(" bake layers of ").unwrap();
    let layers = head
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .expect("layer count");
    let edge = tail
        .trim_end_matches(')')
        .trim_end_matches('²')
        .parse()
        .expect("layer edge");
    (layers, edge)
}

struct Bc5Error {
    max: u8,
    mean: f64,
    samples: usize,
    inexact: usize,
}

/// Encode the raw fill layer by layer as the bake does (R/G left, B/A right of a
/// `2E × E` image, mask encoder), decode, and compare on the used channels.
/// Block origins are 4-aligned and texels outside every block stay zero, so
/// this is each block's own encode error, over every bake-layer texel.
fn bc5_error(raw: &[u8], layers: usize, edge: usize, used: &[usize]) -> Bc5Error {
    assert_eq!(
        raw.len(),
        layers * edge * edge * 4,
        "raw fill is L × E² × 4"
    );
    let width = 2 * edge;
    let (mut max, mut sum, mut samples, mut inexact) = (0u8, 0u64, 0usize, 0usize);
    for layer in raw.chunks_exact(edge * edge * 4) {
        let mut image = vec![0u8; width * edge * 4];
        for (i, texel) in layer.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i % edge, i / edge);
            image[(y * width + x) * 4..][..2].copy_from_slice(&texel[..2]);
            image[(y * width + edge + x) * 4..][..2].copy_from_slice(&texel[2..]);
        }
        let encoded = crate::bc5::encode_bc5_rg_masks(&image, width as u32, edge as u32);
        let rg = crate::bc5::decode_bc5_rg(&encoded, width as u32, edge as u32);
        for (i, texel) in layer.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i % edge, i / edge);
            let decoded = [
                rg[(y * width + x) * 2],
                rg[(y * width + x) * 2 + 1],
                rg[(y * width + edge + x) * 2],
                rg[(y * width + edge + x) * 2 + 1],
            ];
            for &c in used {
                let error = texel[c].abs_diff(decoded[c]);
                max = max.max(error);
                sum += u64::from(error);
                samples += 1;
                inexact += usize::from(error > 0);
            }
        }
    }
    Bc5Error {
        max,
        mean: sum as f64 / samples.max(1) as f64,
        samples,
        inexact,
    }
}

/// Owner-report lines: the verbose peak summary and every drop warning, the
/// latter with its lit-texel count cut off so off and on compare by site.
fn owner_report(logs: &[CapturedRecord]) -> (String, BTreeSet<String>) {
    let mut peak = String::from("(no peak line)");
    let mut drops = BTreeSet::new();
    for message in logs.iter().map(|r| r.message.as_str()) {
        if let Some(rest) = message.split_once("peak per-face owner demand: ") {
            peak = rest.1.to_owned();
        } else if message.starts_with("[ShadowmaskAtlas]")
            && (message.contains("lost its specular mask channel")
                || message.contains("is not among the estimated"))
        {
            let site = message.split(" (").next().unwrap_or(message);
            drops.insert(site.to_owned());
        }
    }
    (peak, drops)
}

#[test]
#[ignore = "measurement: compiles content/dev/maps fixtures in process, floor off and on"]
fn penumbra_floor_bc5_shadowmask_error_and_owner_report() {
    let out_dir = std::env::temp_dir().join(format!(
        "postretro-penumbra-floor-bc5-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&out_dir).expect("scratch dir");
    let cli_dir = std::env::var(PRL_DIR_ENV).ok().map(PathBuf::from);
    for map in BC5_FIXTURES {
        let mut drops_by_setting = Vec::new();
        let mut bakes = Vec::new();
        for floor in [false, true] {
            let bake = compile_in_process(map, floor, &out_dir);
            let setting = if floor { "floor on " } else { "floor off" };
            let owners = face_owners(&bake.sections);
            let used: Vec<usize> = (0..SHADOWMASK_OWNER_CHANNELS)
                .filter(|&c| owners.iter().any(|o| o.len() > c))
                .collect();
            let (peak, drops) = owner_report(&bake.logs);
            if bake.raw.is_empty() {
                println!("{map} {setting}: no shadowmask fill; owner demand {peak}");
            } else {
                let (layers, edge) = bake_layers(&bake.logs);
                let e = bc5_error(&bake.raw, layers, edge, &used);
                println!(
                    "{map} {setting}: {edge}²×{layers} bake layers, {} used channel(s), {} samples: BC5 max {}/255, mean {:.6}/255, {} inexact ({:.5}%)",
                    used.len(),
                    e.samples,
                    e.max,
                    e.mean,
                    e.inexact,
                    100.0 * e.inexact as f64 / e.samples.max(1) as f64
                );
                println!("{map} {setting}: owner demand {peak}");
            }
            if let Some(dir) = &cli_dir {
                let cli = dir.join(format!("{map}.{}.prl", if floor { "floor" } else { "off" }));
                let id42 = SectionId::ShadowmaskAtlas as u32;
                let cli_sections = sections(cli.to_str().expect("UTF-8 path"));
                println!(
                    "{map} {setting}: id 42 equals {}: {}",
                    cli.display(),
                    cli_sections.get(&id42) == bake.sections.get(&id42)
                );
            }
            drops_by_setting.push(drops);
            bakes.push(bake);
        }
        for site in drops_by_setting[1].difference(&drops_by_setting[0]) {
            println!("{map}: NEWLY DROPPED with the floor on: {site}");
        }
        for site in drops_by_setting[0].difference(&drops_by_setting[1]) {
            println!("{map}: no longer dropped with the floor on: {site}");
        }
        println!("{map}: owner table, floor off -> on:");
        report_owner_changes(&bakes[0].sections, &bakes[1].sections);
        for bake in &bakes {
            let _ = std::fs::remove_file(&bake.prl);
        }
    }
    let _ = std::fs::remove_dir_all(&out_dir);
}
