// On-demand check that chart-local reseeding changes only the lightmap family of a PRL.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)
//
// Compile the same map `--release` at a baseline commit and at this one, then:
//   POSTRETRO_RESEED_BASELINE_PRL=<before.prl> POSTRETRO_RESEED_AFTER_PRL=<after.prl> \
//   cargo test -p postretro-level-compiler --bin prl-build reseeded_prl -- --ignored --nocapture

use std::collections::BTreeMap;
use std::io::Cursor;

use postretro_level_format::SectionId;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::{read_container, read_section_data};

use crate::bc6h::{decode_bc6h_block_for_tests, f16_bits_to_f32};

const BASELINE_ENV: &str = "POSTRETRO_RESEED_BASELINE_PRL";
const AFTER_ENV: &str = "POSTRETRO_RESEED_AFTER_PRL";

/// Reseeding envelope recorded with the campaign-test baseline (`19fb3fc40`
/// vs chart-local seeds, both `--release`): 124,250 of 18,578,896 texels
/// (0.67%) changed, the largest by 0.267 linear irradiance on one channel.
/// Only soft-shadow penumbrae move. 496 dim penumbra-edge texels (0.05–0.09)
/// flip to or from exactly zero, because the 4-ray probe early-out classifies
/// them as fully shadowed under one sample rotation and not the other, so the
/// tolerance is absolute, not relative.
const IRRADIANCE_TOLERANCE_ABS: f32 = 0.3;
const CHANGED_TEXEL_FRACTION_MAX: f64 = 0.01;
/// Relative-change floor for the printed distribution, so unlit texels
/// compare absolutely.
const IRRADIANCE_REL_FLOOR: f32 = 0.05;

fn sections(path: &str) -> BTreeMap<u32, Vec<u8>> {
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

/// Decoded RGB irradiance of one block, row-major.
fn block_irradiance(section: &LightmapSection, block: usize) -> Vec<[f32; 3]> {
    let b = &section.blocks[block];
    let (w, h) = (usize::from(b.width), usize::from(b.height));
    let mut texels = vec![[0.0; 3]; w * h];
    if section.irradiance_format == 0 {
        for (i, texel) in b.irradiance.as_chunks::<8>().0.iter().enumerate() {
            for c in 0..3 {
                texels[i][c] =
                    f16_bits_to_f32(u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]));
            }
        }
        return texels;
    }
    for (bi, encoded) in b.irradiance.as_chunks::<16>().0.iter().enumerate() {
        let (bx, by) = ((bi % (w / 4)) * 4, (bi / (w / 4)) * 4);
        let decoded = decode_bc6h_block_for_tests(encoded);
        for (k, texel) in decoded.iter().enumerate() {
            let (x, y) = (bx + k % 4, by + k / 4);
            for c in 0..3 {
                texels[y * w + x][c] = f16_bits_to_f32(texel[c]);
            }
        }
    }
    texels
}

#[test]
#[ignore = "compares two on-demand --release PRLs; see the file header"]
fn reseeded_prl_differs_from_baseline_only_in_lightmap_family() {
    // A blanket `--ignored` run has no PRLs to compare: skip, don't fail.
    let (Ok(before_path), Ok(after_path)) = (std::env::var(BASELINE_ENV), std::env::var(AFTER_ENV))
    else {
        eprintln!("skipped: set {BASELINE_ENV} and {AFTER_ENV} to compare two PRLs");
        return;
    };
    let before = sections(&before_path);
    let after = sections(&after_path);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "section sets differ"
    );

    let id = |s: SectionId| s as u32;
    let lightmap_family = [
        id(SectionId::Lightmap),
        id(SectionId::ShadowmaskAtlas),
        id(SectionId::AnimatedLightWeightMaps),
    ];
    let chunk_stamped = [
        id(SectionId::AnimatedLightChunks),
        id(SectionId::Bvh),
        id(SectionId::Geometry),
    ];
    let differing: Vec<u32> = before
        .keys()
        .copied()
        .filter(|k| before[k] != after[k])
        .collect();
    println!("differing sections: {differing:?}");
    // The data script embeds its absolute source path, which differs when the
    // baseline compiled in another checkout; its compiled bytes may not.
    let script = id(SectionId::DataScript);
    if differing.contains(&script) {
        let decode = |s: &BTreeMap<u32, Vec<u8>>| {
            DataScriptSection::from_bytes(&s[&script]).expect("id 28 decodes")
        };
        assert_eq!(
            decode(&before).compiled_bytes,
            decode(&after).compiled_bytes,
            "the compiled data script changed"
        );
    }
    for k in differing.iter().filter(|&&k| k != script) {
        assert!(
            lightmap_family.contains(k) || chunk_stamped.contains(k),
            "section {k} changed; reseeding may change only the lightmap family"
        );
    }

    // A chunk flipped between lit and unlit only if the culled chunk table
    // changed; only then may the chunk ranges and block ids it stamps move.
    let chunks = id(SectionId::AnimatedLightChunks);
    let chunk_flip = differing.contains(&chunks);
    if chunk_flip {
        let count = |s: &BTreeMap<u32, Vec<u8>>| {
            AnimatedLightChunksSection::from_bytes(&s[&chunks])
                .expect("id 24 decodes")
                .chunks
                .len()
        };
        println!(
            "animated chunks: {} before, {} after",
            count(&before),
            count(&after)
        );
    }
    let geometry = |s: &BTreeMap<u32, Vec<u8>>| {
        GeometrySection::from_bytes(&s[&id(SectionId::Geometry)]).expect("id 17 decodes")
    };
    let (geo_before, geo_after) = (geometry(&before), geometry(&after));
    assert_eq!(geo_before.vertices.len(), geo_after.vertices.len());
    for (i, (a, b)) in geo_before
        .vertices
        .iter()
        .zip(&geo_after.vertices)
        .enumerate()
    {
        let mut b = b.clone();
        if chunk_flip {
            b.animated_block = a.animated_block;
        }
        assert_eq!(*a, b, "vertex {i} changed beyond its animated block");
    }
    assert_eq!(geo_before.indices, geo_after.indices);
    assert_eq!(geo_before.faces, geo_after.faces);
    if differing.contains(&id(SectionId::Bvh)) {
        assert!(chunk_flip, "BVH changed without a chunk flip");
        let bvh = |s: &BTreeMap<u32, Vec<u8>>| {
            BvhSection::from_bytes(&s[&id(SectionId::Bvh)]).expect("id 19 decodes")
        };
        let (a, b) = (bvh(&before), bvh(&after));
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.leaves.len(), b.leaves.len());
        for (la, lb) in a.leaves.iter().zip(&b.leaves) {
            let mut lb = *lb;
            lb.chunk_range_start = la.chunk_range_start;
            lb.chunk_range_count = la.chunk_range_count;
            assert_eq!(*la, lb, "a BVH leaf changed beyond its chunk range");
        }
    }

    // Block extents and placements: the same blocks, and every vertex kept
    // its block and lightmap UV (checked above).
    let lightmap = |s: &BTreeMap<u32, Vec<u8>>| {
        LightmapSection::from_bytes(&s[&id(SectionId::Lightmap)]).expect("id 22 decodes")
    };
    let (lm_before, lm_after) = (lightmap(&before), lightmap(&after));
    let records = |s: &LightmapSection| -> Vec<(u32, u16, u16)> {
        s.blocks
            .iter()
            .map(|b| (b.cell_id, b.width, b.height))
            .collect()
    };
    assert_eq!(
        records(&lm_before),
        records(&lm_after),
        "block extents changed"
    );

    let (mut texels, mut changed, mut worst_rel, mut worst_abs) = (0u64, 0u64, 0.0f32, 0.0f32);
    let mut rels = Vec::new();
    for block in 0..lm_before.blocks.len() {
        let (a, b) = (
            block_irradiance(&lm_before, block),
            block_irradiance(&lm_after, block),
        );
        for (ta, tb) in a.iter().zip(&b) {
            texels += 1;
            let mut texel_rel = 0.0f32;
            for c in 0..3 {
                let abs = (ta[c] - tb[c]).abs();
                let rel = abs / ta[c].max(tb[c]).max(IRRADIANCE_REL_FLOOR);
                worst_abs = worst_abs.max(abs);
                texel_rel = texel_rel.max(rel);
            }
            if texel_rel > 0.0 {
                changed += 1;
                rels.push(texel_rel);
            }
            worst_rel = worst_rel.max(texel_rel);
        }
    }
    rels.sort_by(f32::total_cmp);
    let pct = |p: f64| {
        rels.get(((rels.len() as f64 - 1.0) * p) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "irradiance: {changed} of {texels} texels changed; relative change p50 {:.4} p99 {:.4} max {:.4}; max absolute {:.4}",
        pct(0.5),
        pct(0.99),
        worst_rel,
        worst_abs
    );
    assert!(
        worst_abs <= IRRADIANCE_TOLERANCE_ABS,
        "irradiance moved {worst_abs:.4}, past the recorded {IRRADIANCE_TOLERANCE_ABS}"
    );
    let fraction = changed as f64 / texels.max(1) as f64;
    assert!(
        fraction <= CHANGED_TEXEL_FRACTION_MAX,
        "{changed} of {texels} texels changed, past the recorded {CHANGED_TEXEL_FRACTION_MAX}"
    );
}
