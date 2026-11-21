// Material-stage bake: stats, sidecar reuse and repair, the fluid tail, and texel lifetimes.
// See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use super::probe_test_metrics as metrics;
use super::*;
use crate::surface_materials::stats::{
    BOUNCE_SATURATION, BounceMaterials, EmissiveGlow, MaterialStats,
};
use log::Level;
use postretro_level_format::prm::PrmReadError;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use postretro_test_log_capture::LogCapture;

struct Fixture {
    root: PathBuf,
    textures: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "postretro-material-stage-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let textures = root.join("textures");
        std::fs::create_dir_all(&textures).unwrap();
        Self {
            cache: root.join("cache"),
            root,
            textures,
        }
    }

    fn png(&self, name: &str, width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) {
        let path = self.textures.join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let image = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        std::fs::write(path, bytes.into_inner()).unwrap();
    }

    fn bake(
        &self,
        names: &[String],
    ) -> (
        HashMap<String, [u8; 32]>,
        Vec<MaterialStats>,
        StageTextureBytes,
    ) {
        let mut bytes = StageTextureBytes::new();
        let bake =
            bake_materials_into(names, &self.textures, &self.cache, &mut bytes, true).unwrap();
        (bake.keys, bake.stats, bytes)
    }

    fn glow(&self, names: &[String]) -> Vec<EmissiveGlow> {
        bake_materials_into(
            names,
            &self.textures,
            &self.cache,
            &mut StageTextureBytes::new(),
            true,
        )
        .unwrap()
        .glow
    }

    fn sidecars(&self) -> std::collections::BTreeMap<String, Vec<u8>> {
        if !self.cache.exists() {
            return Default::default();
        }
        std::fs::read_dir(&self.cache)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_str().unwrap().to_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn assert_same_stats(left: &[MaterialStats], right: &[MaterialStats]) {
    let means = |stats: &[MaterialStats]| {
        stats
            .iter()
            .map(MaterialStats::snapped_mean)
            .collect::<Vec<_>>()
    };
    assert_eq!(means(left), means(right));
}

#[test]
fn material_stage_stats_match_reused_sidecars_and_preserve_sidecar_bytes() {
    let fixture = Fixture::new();
    // Every channel byte occurs, with NPOT dimensions; mip zero must preserve it.
    fixture.png("stone/panel", 257, 3, |x, y| {
        [
            x as u8,
            (255 - x.min(255)) as u8,
            (x + y) as u8,
            (x + y * 7) as u8,
        ]
    });
    fixture.png("stone/panel_e", 257, 3, |x, y| {
        [
            (x + y) as u8,
            (x * 17 + y) as u8,
            (255 - x.min(255)) as u8,
            (x ^ y) as u8,
        ]
    });
    let names = vec!["stone/panel".to_owned()];
    let ((keys, cold, cold_bytes), cold_metrics) = metrics::observe(|| fixture.bake(&names));
    assert_eq!(cold_metrics.decode_attempts.len(), 2);
    assert!(
        cold_metrics
            .decode_attempts
            .values()
            .all(|&count| count == 1)
    );
    assert_eq!(cold_metrics.decode_attempts, cold_metrics.decode_successes);
    assert!(cold[0].snapped_mean().is_some());
    let sidecars = fixture.sidecars();
    // Independently reconstruct source texels and pin both mip-zero payloads,
    // including alpha, before the reused-sidecar statistics path reads them.
    let (_, slots) = PrmFile::from_bytes_partial(sidecars.values().next().unwrap());
    let mut expected_diffuse = Vec::new();
    let mut expected_emissive = Vec::new();
    for y in 0..3_u32 {
        for x in 0..257_u32 {
            expected_diffuse.extend_from_slice(&[
                x as u8,
                (255 - x.min(255)) as u8,
                (x + y) as u8,
                (x + y * 7) as u8,
            ]);
            expected_emissive.extend_from_slice(&[
                (x + y) as u8,
                (x * 17 + y) as u8,
                (255 - x.min(255)) as u8,
                (x ^ y) as u8,
            ]);
        }
    }
    assert_eq!(
        &slots[0].as_ref().unwrap().payload[..expected_diffuse.len()],
        &expected_diffuse
    );
    assert_eq!(
        &slots[3].as_ref().unwrap().payload[..expected_emissive.len()],
        &expected_emissive
    );
    let ((warm_keys, warm, warm_bytes), warm_metrics) = metrics::observe(|| fixture.bake(&names));
    assert!(warm_metrics.decode_attempts.is_empty());
    assert!(warm_metrics.decode_successes.is_empty());
    assert_eq!(warm_metrics.stats_calls, 1);
    assert_eq!(keys, warm_keys);
    assert_same_stats(&cold, &warm);
    assert_eq!(cold_bytes.summary(), warm_bytes.summary());
    assert_eq!(sidecars, fixture.sidecars());
    let legacy_cache = fixture.root.join("legacy-cache");
    let (legacy_keys, legacy_bytes) =
        bake_world_texture_mips(&names, &fixture.textures, &legacy_cache).unwrap();
    assert_eq!(keys, legacy_keys);
    assert_eq!(cold_bytes.summary(), legacy_bytes.summary());
    for (name, expected) in sidecars {
        assert_eq!(std::fs::read(legacy_cache.join(name)).unwrap(), expected);
    }
}

// Aliases resolve one source bundle, and the glow test reads that bundle's
// `_e` slot, never the name: every alias, backslashed or not, takes one mean.
#[test]
fn material_stage_aliases_preserve_name_order_and_share_one_glow_mean() {
    const SLASH: &str = "panels/glow_panel";
    const SLASH_ALIAS: &str = "textures/PANELS/GLOW_PANEL";
    const BACKSLASH: &str = "textures/PANELS\\GLOW_PANEL";
    const BACKSLASH_ALIAS: &str = "PANELS\\glow_panel";
    const PLAIN: &str = "panels/plain_panel";
    for reversed in [false, true] {
        let fixture = Fixture::new();
        for stem in [SLASH, PLAIN] {
            fixture.png(
                stem,
                2,
                1,
                |x, _| if x == 0 { [255, 0, 0, 255] } else { [255; 4] },
            );
        }
        fixture.png(&format!("{SLASH}_e"), 2, 1, |x, _| {
            if x == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        let mut names: Vec<_> = [SLASH, SLASH_ALIAS, PLAIN, BACKSLASH, BACKSLASH_ALIAS]
            .into_iter()
            .map(str::to_owned)
            .collect();
        if reversed {
            names.reverse();
        }
        for phase in ["cold", "warm"] {
            let ((keys, stats, bytes), observed) = metrics::observe(|| fixture.bake(&names));
            if phase == "cold" {
                assert_eq!(observed.decode_attempts.len(), 3);
                assert!(observed.decode_attempts.values().all(|&count| count == 1));
                assert_eq!(observed.decode_attempts, observed.decode_successes);
            } else {
                assert!(observed.decode_attempts.is_empty());
            }
            assert_eq!(fixture.sidecars().len(), 2, "one sidecar per bundle");
            let glow_key = keys[SLASH];
            for alias in [SLASH_ALIAS, BACKSLASH, BACKSLASH_ALIAS] {
                assert_eq!(keys[alias], glow_key, "{alias} shares the bundle");
            }
            assert_ne!(keys[PLAIN], glow_key);
            assert_eq!(bytes.summary().material_count(), 2);
            assert_eq!(bytes.summary().entries().next().unwrap().0, names[0]);
            let means: HashMap<_, _> = names
                .iter()
                .zip(&stats)
                .map(|(name, stat)| (name.as_str(), stat.snapped_mean()))
                .collect();
            for name in [SLASH, SLASH_ALIAS, BACKSLASH, BACKSLASH_ALIAS] {
                assert_eq!(
                    means[name],
                    Some([255; 3]),
                    "the red glow texel leaves {name}'s mean ({phase}, reversed={reversed})"
                );
            }
            assert_eq!(
                means[PLAIN],
                Some([255, 188, 188]),
                "no `_e`: the full mean ({phase}, reversed={reversed})"
            );
            let glow: HashMap<_, _> = names
                .iter()
                .zip(fixture.glow(&names))
                .map(|(name, glow)| (name.as_str(), glow))
                .collect();
            for name in [SLASH, SLASH_ALIAS, BACKSLASH, BACKSLASH_ALIAS] {
                assert_eq!(
                    glow[name],
                    EmissiveGlow {
                        has_emissive: true,
                        mean_color: [1.0, 0.0, 0.0],
                    },
                    "{name}"
                );
            }
            assert_eq!(glow[PLAIN], EmissiveGlow::default());
        }
    }
}

#[test]
fn material_stage_rebakes_rejected_sidecars_before_deriving_stats() {
    let fixture = Fixture::new();
    fixture.png("panels/glow_panel", 4, 4, |x, _| {
        [255, (x * 60) as u8, 16, 255]
    });
    let names = vec!["panels/glow_panel".to_owned()];
    let (keys, expected, _) = fixture.bake(&names);
    assert!(expected[0].snapped_mean().is_some());
    let path = fixture
        .cache
        .join(format!("{}.prm", cache_filename_for_key(&keys[&names[0]])));
    let intact = std::fs::read(&path).unwrap();
    for corruption in 0..4 {
        match corruption {
            0 => std::fs::remove_file(&path).unwrap(),
            1 => std::fs::write(&path, &intact[..12]).unwrap(),
            2 => {
                // Mutate a parsed header, leaving the rest valid but its bundle hash stale.
                let (header, slots) = PrmFile::from_bytes_partial(&intact);
                let mut header = header.unwrap();
                header.bundle_hash[0] ^= 1;
                let stale = PrmFile {
                    header,
                    slots: slots.map(|slot| slot.ok()),
                }
                .to_bytes()
                .unwrap();
                std::fs::write(&path, stale).unwrap();
            }
            3 => {
                // Header remains valid; the declared diffuse format tag is invalid.
                let mut invalid = intact.clone();
                invalid[45] = 255;
                let (header, slots) = PrmFile::from_bytes_partial(&invalid);
                assert!(header.is_ok());
                assert!(slots[0].is_err());
                std::fs::write(&path, invalid).unwrap();
            }
            _ => unreachable!(),
        }
        let ((repaired_keys, repaired, _), observed) = metrics::observe(|| fixture.bake(&names));
        assert_eq!(observed.decode_attempts.len(), 1, "corruption {corruption}");
        assert!(observed.decode_attempts.values().all(|&count| count == 1));
        assert_eq!(observed.decode_attempts, observed.decode_successes);
        assert_eq!(keys, repaired_keys);
        assert_same_stats(&expected, &repaired);
        assert_eq!(std::fs::read(&path).unwrap(), intact);
    }
}

#[test]
fn material_stage_repairs_stats_incompatible_cached_emissive_dimensions() {
    let fixture = Fixture::new();
    fixture.png("panels/glow_panel", 4, 2, |x, y| {
        [255, (x * 60) as u8, (y * 100) as u8, 255]
    });
    fixture.png("panels/glow_panel_e", 4, 2, |x, _| {
        if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
    });
    let names = vec!["panels/glow_panel".to_owned()];
    let (keys, expected, _) = fixture.bake(&names);
    let path = fixture
        .cache
        .join(format!("{}.prm", cache_filename_for_key(&keys[&names[0]])));
    let intact = std::fs::read(&path).unwrap();
    let (header, slots) = PrmFile::from_bytes_partial(&intact);
    let header = header.unwrap();
    let mut slots = slots.map(|slot| slot.ok());
    let emissive = slots[3].as_mut().unwrap();
    // Transposition preserves pixel count, mip count and payload length.
    // Every slot still parses, and the original source hash still matches.
    std::mem::swap(&mut emissive.width, &mut emissive.height);
    let incompatible = PrmFile { header, slots }.to_bytes().unwrap();
    let (header, slots) = PrmFile::from_bytes_partial(&incompatible);
    let header = header.unwrap();
    assert!(cache_entry_has_valid_declared_slots(&header, &slots));
    assert_eq!(slots[3].as_ref().unwrap().width, 2);
    assert_eq!(slots[3].as_ref().unwrap().height, 4);
    std::fs::write(&path, &incompatible).unwrap();

    let ((repaired_keys, repaired, _), observed) = metrics::observe(|| fixture.bake(&names));
    assert_eq!(observed.decode_attempts.len(), 2);
    assert!(observed.decode_attempts.values().all(|&count| count == 1));
    assert_eq!(observed.decode_attempts, observed.decode_successes);
    assert_eq!(keys, repaired_keys);
    assert_same_stats(&expected, &repaired);
    assert_eq!(std::fs::read(&path).unwrap(), intact);

    let ((_, reused, _), observed) = metrics::observe(|| fixture.bake(&names));
    assert!(observed.decode_attempts.is_empty());
    assert_same_stats(&expected, &reused);
}

// Regression: a hash-matching sidecar with a sourced colour slot dropped passed
// slot validation and served a wrong mean: glow strip included, or grey.
#[test]
fn material_stage_repairs_cached_sidecars_missing_a_sourced_colour_slot() {
    for (dropped, bit) in [(3, PrmSlots::EMISSIVE), (0, PrmSlots::DIFFUSE)] {
        let fixture = Fixture::new();
        fixture.png("panels/glow_panel", 4, 2, |x, y| {
            [255, (x * 60) as u8, (y * 100) as u8, 255]
        });
        fixture.png("panels/glow_panel_e", 4, 2, |x, _| {
            if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
        });
        let names = vec!["panels/glow_panel".to_owned()];
        let (keys, expected, _) = fixture.bake(&names);
        assert!(expected[0].snapped_mean().is_some());
        let path = fixture
            .cache
            .join(format!("{}.prm", cache_filename_for_key(&keys[&names[0]])));
        let intact = std::fs::read(&path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&intact);
        let mut header = header.unwrap();
        let mut slots = slots.map(|slot| slot.ok());
        // The source hash still matches and every declared slot still parses.
        slots[dropped] = None;
        header.slot_mask.remove(bit);
        let incomplete = PrmFile { header, slots }.to_bytes().unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&incomplete);
        assert!(cache_entry_has_valid_declared_slots(
            &header.unwrap(),
            &slots
        ));
        assert!(slots[dropped].is_err());
        std::fs::write(&path, &incomplete).unwrap();

        let capture = LogCapture::start();
        let ((repaired_keys, repaired, _), observed) = metrics::observe(|| fixture.bake(&names));
        capture.assert_logged_once(
            Level::Debug,
            "rebaking cached sidecar for 'panels/glow_panel'",
        );
        assert_eq!(observed.decode_attempts.len(), 2, "{bit:?}");
        assert!(observed.decode_attempts.values().all(|&count| count == 1));
        assert_eq!(observed.decode_attempts, observed.decode_successes);
        assert_eq!(keys, repaired_keys);
        assert_same_stats(&expected, &repaired);
        assert_eq!(std::fs::read(&path).unwrap(), intact);
    }
}

#[test]
fn material_stage_empty_missing_and_orphan_sources_keep_indexed_grey_records() {
    let fixture = Fixture::new();
    fixture.png("panels/orphan_e", 1, 1, |_, _| [255; 4]);
    fixture.png("panels/spec_only_s", 1, 1, |_, _| [128; 4]);
    let names = vec![
        "panels/missing".to_owned(),
        "panels/orphan".to_owned(),
        "panels/spec_only".to_owned(),
    ];
    let (keys, stats, _) = fixture.bake(&names);
    assert_eq!(keys["panels/missing"], [0; 32]);
    assert_ne!(keys["panels/orphan"], [0; 32]);
    assert_ne!(keys["panels/spec_only"], [0; 32]);
    assert_eq!(fixture.sidecars().len(), 2);
    assert_eq!(stats.len(), names.len());
    assert!(stats.iter().all(|stats| stats.snapped_mean().is_none()));
    let table = BounceMaterials::new(stats, BOUNCE_SATURATION).unwrap();
    assert!((0..names.len() as u32).all(|index| table.resolved_albedo(index).is_none()));
    let (keys, stats, bytes) = fixture.bake(&[]);
    assert!(keys.is_empty() && stats.is_empty());
    assert_eq!(bytes.summary().material_count(), 0);
}

#[test]
fn material_stage_propagates_source_decode_and_dimension_failures_without_sidecars() {
    for mismatch in [false, true] {
        let fixture = Fixture::new();
        let names = vec!["panels/glow_panel".to_owned()];
        if mismatch {
            fixture.png("panels/glow_panel", 2, 2, |_, _| [255; 4]);
            fixture.png("panels/glow_panel_e", 1, 2, |_, _| [255; 4]);
        } else {
            std::fs::create_dir_all(fixture.textures.join("panels")).unwrap();
            std::fs::write(fixture.textures.join("panels/glow_panel.png"), b"not a PNG").unwrap();
        }
        for collect_stats in [true, false] {
            let (result, observed) = metrics::observe(|| {
                bake_materials_into(
                    &names,
                    &fixture.textures,
                    &fixture.cache,
                    &mut StageTextureBytes::new(),
                    collect_stats,
                )
            });
            let error = result.unwrap_err();
            if mismatch {
                assert!(error.to_string().contains("dimensions must match"));
                assert!(observed.decode_attempts.is_empty());
            } else {
                assert!(error.to_string().contains("decoding PNG"));
                assert_eq!(observed.decode_attempts.len(), 1);
                assert!(observed.decode_successes.is_empty());
            }
            assert_eq!(observed.active_textures, 0);
            assert_eq!(observed.active_stats_borrows, 0);
            assert!(!fixture.cache.exists());
        }
    }
}

#[test]
fn material_stage_and_fluid_tail_match_full_name_byte_accounting_cold_and_warm() {
    let fixture = Fixture::new();
    for (index, name) in [
        "panels/world",
        "panels/mover",
        "panels/shared",
        "panels/fluid",
    ]
    .iter()
    .enumerate()
    {
        fixture.png(name, 2, 2, |_, _| [(index * 50) as u8, 32, 64, 255]);
    }
    fixture.png("panels/fluid_e", 2, 2, |x, _| {
        if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
    });
    let names: Vec<_> = [
        "panels/world",
        "panels/mover",
        "panels/shared",
        "panels/fluid",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let full_cache = fixture.root.join("full-cache");
    for phase in ["cold", "warm"] {
        let (full_keys, full_bytes) =
            bake_world_texture_mips(&names, &fixture.textures, &full_cache).unwrap();
        let observation = metrics::Observation::start();
        let mut split_bytes = StageTextureBytes::new();
        let MaterialBake {
            keys: mut split_keys,
            stats,
            glow,
            ..
        } = bake_materials_into(
            &names[..3],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            true,
        )
        .unwrap();
        assert_eq!(stats.len(), 3);
        assert!(stats.iter().all(|stat| stat.snapped_mean().is_some()));
        assert_eq!(glow, [EmissiveGlow::default(); 3]);
        let tail = bake_materials_into(
            &names[3..],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            false,
        )
        .unwrap();
        assert!(tail.stats.is_empty());
        // The fluid tail takes no bounce stats but still reads its `_e`
        // facts, cold and warm: id 60 covers fluid-only names.
        assert_eq!(
            tail.glow,
            [EmissiveGlow {
                has_emissive: true,
                mean_color: [1.0; 3],
            }],
            "{phase}"
        );
        let tail = tail.keys;
        let observed = observation.snapshot();
        drop(observation);
        if phase == "cold" {
            assert_eq!(observed.decode_attempts.len(), 5);
            assert!(observed.decode_attempts.values().all(|&count| count == 1));
            assert_eq!(observed.decode_attempts, observed.decode_successes);
        } else {
            assert!(observed.decode_attempts.is_empty());
        }
        split_keys.extend(tail);
        assert_eq!(split_keys, full_keys);
        assert!(split_keys.values().all(|key| *key != [0; 32]));
        assert_eq!(fixture.sidecars().len(), 4);
        assert_eq!(split_bytes.summary().material_count(), 4);
        assert_eq!(split_bytes.summary(), full_bytes.summary());
        for (filename, bytes) in fixture.sidecars() {
            assert_eq!(std::fs::read(full_cache.join(filename)).unwrap(), bytes);
        }
        let before = split_bytes.summary().clone();
        let empty = bake_materials_into(
            &[],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            false,
        )
        .unwrap();
        assert!(empty.keys.is_empty() && empty.stats.is_empty() && empty.glow.is_empty());
        assert_eq!(split_bytes.summary(), &before);
    }
}

/// Independent sRGB decode, for expected means.
fn srgb_to_linear(value: u8) -> f64 {
    let value = f64::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn snapped_linear_mean(texels: &[[u8; 3]]) -> [u8; 3] {
    std::array::from_fn(|channel| {
        let sum: f64 = texels
            .iter()
            .map(|texel| srgb_to_linear(texel[channel]))
            .sum();
        linear_to_srgb_u8((sum / texels.len() as f64) as f32)
    })
}

// The glow test reads the `_e` slot, whatever the name's prefix and strength.
#[test]
fn material_stage_emissive_textures_exclude_only_masked_texels_and_fall_back_to_the_full_average() {
    const STRIP: [u8; 3] = [255, 255, 255];
    const PANEL: [u8; 3] = [40, 80, 160];
    let fixture = Fixture::new();
    let diffuse = |x: u32, _| {
        let [r, g, b] = if x < 2 { STRIP } else { PANEL };
        [r, g, b, 255]
    };
    // One painted strip; three names that differ only in `_e`. No prefix
    // marks any of them as glowing.
    for stem in ["panels/bare", "panels/strip", "panels/all"] {
        fixture.png(stem, 8, 2, diffuse);
    }
    fixture.png("panels/strip_e", 8, 2, |x, _| {
        if x < 2 {
            [255, 64, 0, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    fixture.png("panels/all_e", 8, 2, |_, _| [0, 32, 0, 255]);
    let names: Vec<_> = ["panels/bare", "panels/strip", "panels/all"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let texels: Vec<[u8; 3]> = (0..2)
        .flat_map(|_| (0..8).map(|x| if x < 2 { STRIP } else { PANEL }))
        .collect();
    let full = snapped_linear_mean(&texels);
    assert_ne!(full, PANEL, "the strip must move the full average");
    for phase in ["cold", "warm"] {
        let (_, stats, _) = fixture.bake(&names);
        let means: Vec<_> = stats.iter().map(MaterialStats::snapped_mean).collect();
        assert_eq!(
            means,
            [Some(full), Some(PANEL), Some(full)],
            "no `_e`: full; strip `_e`: panel only; all-glow `_e`: full ({phase})"
        );
        let glow = fixture.glow(&names);
        assert_eq!(glow[0], EmissiveGlow::default(), "{phase}");
        assert!(glow[1].has_emissive && glow[2].has_emissive, "{phase}");
        let strip = [255, 64, 0].map(|channel| srgb_to_linear(channel) as f32);
        for (actual, expected) in glow[1].mean_color.iter().zip(strip) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "{phase}: {:?}",
                glow[1].mean_color
            );
        }
        assert!((glow[2].mean_color[1] - srgb_to_linear(32) as f32).abs() < 1e-6);
    }
}

#[test]
fn material_stage_texel_lifetimes_stay_with_one_texture_and_return_fixed_records() {
    let fixture = Fixture::new();
    let names: Vec<_> = (0..32)
        .map(|index| format!("panels/glow_{index}"))
        .collect();
    // Distinct content per name: every cold bake decodes its own sources.
    for (index, name) in names.iter().enumerate() {
        let index = index as u32;
        fixture.png(name, 8, 8, |x, y| {
            [(x * 29 + index) as u8, (y * 31 + index * 7) as u8, 73, 255]
        });
        fixture.png(&format!("{name}_e"), 8, 8, |x, _| {
            if x < 4 {
                [0, 0, 0, 255]
            } else {
                [255, index as u8, 0, 255]
            }
        });
    }
    // Each texture alone, cold then warm, bounds what one texture holds.
    let mut cold_singles = Vec::new();
    let mut warm_singles = Vec::new();
    for name in &names {
        let one = std::slice::from_ref(name);
        cold_singles.push(metrics::observe(|| fixture.bake(one)).1);
        warm_singles.push(metrics::observe(|| fixture.bake(one)).1);
        std::fs::remove_dir_all(&fixture.cache).unwrap();
    }
    let ((_, many, _), cold_many) = metrics::observe(|| fixture.bake(&names));
    let ((_, _, _), warm_many) = metrics::observe(|| fixture.bake(&names));
    assert_eq!(many.len(), names.len());
    // Records own no texel buffer and require no heap cleanup. Only this
    // fixed-size table grows with names; observed texel bytes do not.
    assert!(!std::mem::needs_drop::<MaterialStats>());
    let table_bytes = many.len() * std::mem::size_of::<MaterialStats>();
    assert_eq!(table_bytes, names.len() * std::mem::size_of_val(&many[0]));
    for observed in cold_singles
        .iter()
        .chain(&warm_singles)
        .chain([&cold_many, &warm_many])
    {
        // The bake runs one texture at a time on its one worker.
        assert_eq!(observed.texture_high_water, 1);
        assert_eq!(observed.active_textures, 0);
        assert_eq!(observed.stats_borrow_high_water, 1);
        assert_eq!(observed.active_stats_borrows, 0);
        assert_eq!(observed.borrowed_high_water, 8 * 8 * 4 * 2);
        assert!(observed.owned_high_water >= observed.borrowed_high_water);
        assert_eq!(observed.stats_calls, observed.texture_count);
    }
    assert_eq!(cold_many.texture_count, names.len());
    assert_eq!(cold_many.largest_decoded_rgba, 8 * 8 * 4);
    // Every distinct diffuse and glow source decodes exactly once, cold.
    assert_eq!(cold_many.decode_attempts.len(), 2 * names.len());
    assert!(cold_many.decode_attempts.values().all(|&count| count == 1));
    assert_eq!(cold_many.decode_attempts, cold_many.decode_successes);
    assert!(warm_many.decode_attempts.is_empty());
    // Nothing accumulates: many textures peak at the largest single texture.
    let peak = |runs: &[metrics::Snapshot]| runs.iter().map(|run| run.owned_high_water).max();
    assert_eq!(Some(cold_many.owned_high_water), peak(&cold_singles));
    assert_eq!(Some(warm_many.owned_high_water), peak(&warm_singles));
    assert_eq!(
        fixture.sidecars().len(),
        names.len(),
        "one sidecar per texture; stats create no additional persistent files"
    );
}

// -- Smoothness and tint in the specular slot ------------------------------

impl Fixture {
    /// `key`'s sidecar, read back through the runtime `.prm` reader.
    fn read_sidecar(&self, key: &[u8; 32]) -> (PrmHeader, [Result<PrmSlot, PrmReadError>; 4]) {
        let path = self
            .cache
            .join(format!("{}.prm", cache_filename_for_key(key)));
        let bytes = std::fs::read(path).unwrap();
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        (header.expect("sidecar header parses"), slots)
    }

    /// One grayscale sibling: every colour channel holds `value(x, y)`.
    fn gray_png(&self, name: &str, width: u32, height: u32, value: impl Fn(u32, u32) -> u8) {
        self.png(name, width, height, |x, y| {
            let v = value(x, y);
            [v, v, v, 255]
        });
    }

    /// One 8-bit grayscale sibling (PNG colour type 0), the form the
    /// committed dev fixtures take.
    fn luma_png(&self, name: &str, width: u32, height: u32, value: impl Fn(u32, u32) -> u8) {
        let path = self.textures.join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let image = image::GrayImage::from_fn(width, height, |x, y| image::Luma([value(x, y)]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let bytes = bytes.into_inner();
        // IHDR bit depth and colour type: the case must reach the bake as L8.
        assert_eq!(
            (bytes[24], bytes[25]),
            (8, 0),
            "{name}: an 8-bit grayscale PNG"
        );
        std::fs::write(path, bytes).unwrap();
    }

    /// A 4x4 slot sibling holding `value(x, y)` in `encoding`.
    fn sibling_png(&self, encoding: SiblingEncoding, name: &str, value: fn(u32, u32) -> u8) {
        match encoding {
            SiblingEncoding::Rgba8 => self.gray_png(name, 4, 4, value),
            SiblingEncoding::L8 => self.luma_png(name, 4, 4, value),
        }
    }

    /// A diffuse whose texels all differ.
    fn diffuse_png(&self, name: &str, width: u32, height: u32) {
        self.png(name, width, height, |x, y| {
            [(x * 50) as u8, (y * 50) as u8, 90, 255]
        });
    }
}

/// Each mip level of a non-BC5 slot's payload, in order.
fn mip_levels(slot: &PrmSlot) -> Vec<&[u8]> {
    let channels = match slot.format {
        PrmFormat::R8Unorm => 1,
        PrmFormat::Rg8Unorm => 2,
        PrmFormat::Rgba8Unorm | PrmFormat::Rgba8UnormSrgb => 4,
        PrmFormat::Bc5RgUnorm => unreachable!("BC5 levels are block-sized"),
    };
    let (width, height) = (u32::from(slot.width), u32::from(slot.height));
    let mut levels = Vec::new();
    let mut offset = 0;
    for level in 0..u32::from(slot.level_count) {
        let len = ((width >> level).max(1) * (height >> level).max(1)) as usize * channels;
        levels.push(&slot.payload[offset..offset + len]);
        offset += len;
    }
    assert_eq!(
        offset,
        slot.payload.len(),
        "the payload holds exactly its levels"
    );
    levels
}

/// The stored G of a slot without `_h.png`: the inverted mid-gray plane.
const PLANE_G: u8 = 255 - 128;

fn authored_specular(x: u32, _y: u32) -> u8 {
    (60 + 40 * x) as u8
}

fn authored_height(_x: u32, y: u32) -> u8 {
    (20 + 50 * y) as u8
}

/// Spans both marker-adjacent bytes: 0 stores as 1, 255 stays.
fn authored_smoothness(x: u32, y: u32) -> u8 {
    [0, 17, 128, 255][((x + y) % 4) as usize]
}

/// Spans both marker-adjacent bytes: 0 stays, 255 stores as 254.
fn authored_tint(x: u32, y: u32) -> u8 {
    [255, 100, 0, 254][((x + 2 * y) % 4) as usize]
}

/// The material every specular-slot case bakes.
const COBBLE: &str = "stone/cobble";

/// How a specular-slot case encodes its grayscale siblings.
#[derive(Clone, Copy, Debug)]
enum SiblingEncoding {
    /// RGBA8, the value in every colour channel.
    Rgba8,
    /// 8-bit grayscale.
    L8,
}

/// Which siblings a specular-slot case authors as [`COBBLE`].
#[derive(Clone, Copy, Debug)]
struct SlotCase {
    diffuse: bool,
    specular: bool,
    height: bool,
    smoothness: bool,
    tint: bool,
}

const NO_SIBLINGS: SlotCase = SlotCase {
    diffuse: false,
    specular: false,
    height: false,
    smoothness: false,
    tint: false,
};

/// An authored sibling's byte at texel (x, y).
type AuthoredTexel = fn(u32, u32) -> u8;

impl SlotCase {
    fn write(self, fixture: &Fixture) {
        self.write_encoded(fixture, SiblingEncoding::Rgba8);
    }

    fn write_encoded(self, fixture: &Fixture, encoding: SiblingEncoding) {
        if self.diffuse {
            fixture.diffuse_png(COBBLE, 4, 4);
        }
        let siblings: [(bool, &str, AuthoredTexel); 4] = [
            (self.specular, "stone/cobble_s", authored_specular),
            (self.height, "stone/cobble_h", authored_height),
            (self.smoothness, "stone/cobble_p", authored_smoothness),
            (self.tint, "stone/cobble_t", authored_tint),
        ];
        for (present, name, value) in siblings {
            if present {
                fixture.sibling_png(encoding, name, value);
            }
        }
    }

    /// Mip-0 RGBA as decided: R intensity (0 without `_s`), G inverted height
    /// (the plane without `_h`), B max(p, 1) (0 without `_p`), A min(t, 254)
    /// (255 without `_t`).
    fn expected_texel(self, x: u32, y: u32) -> [u8; 4] {
        [
            if self.specular {
                authored_specular(x, y)
            } else {
                0
            },
            if self.height {
                255 - authored_height(x, y)
            } else {
                PLANE_G
            },
            if self.smoothness {
                authored_smoothness(x, y).max(1)
            } else {
                0
            },
            if self.tint {
                authored_tint(x, y).min(254)
            } else {
                255
            },
        ]
    }

    /// Bake the case and check the four-channel slot it must produce through
    /// the runtime reader: the SPECULAR bit, `Rgba8Unorm`, mip 0 as decided,
    /// and both markers held at every mip.
    fn bake_and_check(self) {
        self.bake_and_check_encoded(SiblingEncoding::Rgba8);
    }

    fn bake_and_check_encoded(self, encoding: SiblingEncoding) {
        let fixture = Fixture::new();
        self.write_encoded(&fixture, encoding);
        let names = vec![COBBLE.to_owned()];
        let bake = bake_materials_into(
            &names,
            &fixture.textures,
            &fixture.cache,
            &mut StageTextureBytes::new(),
            true,
        )
        .unwrap();
        assert_eq!(
            bake.bundles,
            [Some(COBBLE.to_owned())],
            "{self:?}: the bundle names its collection"
        );
        let key = bake.keys[COBBLE];
        assert_ne!(key, [0u8; 32], "{self:?}");
        let (header, slots) = fixture.read_sidecar(&key);
        assert!(header.slot_mask.contains(PrmSlots::SPECULAR), "{self:?}");
        assert_eq!(
            header.slot_mask.contains(PrmSlots::DIFFUSE),
            self.diffuse,
            "{self:?}"
        );
        let slot = slots[1]
            .as_ref()
            .expect("four-channel specular slot parses");
        assert_eq!(slot.format, PrmFormat::Rgba8Unorm, "{self:?}");
        assert_eq!((slot.width, slot.height, slot.level_count), (4, 4, 3));
        let levels = mip_levels(slot);
        for (index, texel) in levels[0].as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (index as u32 % 4, index as u32 / 4);
            assert_eq!(
                texel,
                &self.expected_texel(x, y)[..],
                "{self:?}: mip 0 texel ({x}, {y})"
            );
        }
        for (level, texels) in levels.iter().enumerate() {
            for texel in texels.as_chunks::<4>().0.iter() {
                if self.smoothness {
                    assert!(texel[2] >= 1, "{self:?}: mip {level} B {texel:?}");
                } else {
                    assert_eq!(texel[2], 0, "{self:?}: mip {level} B {texel:?}");
                }
                if self.tint {
                    assert!(texel[3] <= 254, "{self:?}: mip {level} A {texel:?}");
                } else {
                    assert_eq!(texel[3], 255, "{self:?}: mip {level} A {texel:?}");
                }
                if !self.height {
                    assert_eq!(texel[1], PLANE_G, "{self:?}: mip {level} G {texel:?}");
                }
            }
        }
    }
}

#[test]
fn smoothness_bundles_bake_four_channel() {
    for case in [
        // `_p` alone: no diffuse and no other sibling.
        SlotCase {
            smoothness: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            specular: true,
            smoothness: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            height: true,
            smoothness: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            specular: true,
            height: true,
            smoothness: true,
            ..NO_SIBLINGS
        },
    ] {
        case.bake_and_check();
    }
}

#[test]
fn tint_bundles_bake_four_channel() {
    for case in [
        // `_t` alone: B stays 0 at every mip, so it shades at its prefix
        // exponent.
        SlotCase {
            tint: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            smoothness: true,
            tint: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            specular: true,
            height: true,
            smoothness: true,
            tint: true,
        },
    ] {
        case.bake_and_check();
    }

    // The prefix lives in the name, never the bytes: identical PNG bundles
    // under two prefixes share one sidecar. Each takes its own prefix exponent
    // at load, from its name; runtime readback covers that half.
    let fixture = Fixture::new();
    let names: Vec<String> = ["test/metal_inlay", "test/wood_inlay"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for stem in &names {
        fixture.diffuse_png(stem, 4, 4);
        fixture.gray_png(&format!("{stem}_t"), 4, 4, authored_tint);
    }
    let (keys, _, _) = fixture.bake(&names);
    assert_eq!(keys[&names[0]], keys[&names[1]]);
    assert_eq!(fixture.sidecars().len(), 1);
}

// The committed dev fixtures author `_s`, `_h`, `_p` and `_t` as 8-bit
// grayscale PNGs; the bake reads the gray byte as the channel value, so they
// store the texels RGBA-authored siblings do.
#[test]
fn grayscale_smoothness_and_tint_bundles_bake_four_channel() {
    for case in [
        SlotCase {
            diffuse: true,
            specular: true,
            smoothness: true,
            tint: true,
            ..NO_SIBLINGS
        },
        SlotCase {
            diffuse: true,
            specular: true,
            height: true,
            smoothness: true,
            tint: true,
        },
    ] {
        case.bake_and_check_encoded(SiblingEncoding::L8);
    }
}

// The stage's byte accounting charges a four-channel specular slot
// four bytes per texel at every mip, for a fresh bake and for a reused
// sidecar alike. A non-square bundle keeps width and height apart.
#[test]
fn byte_accounting_charges_a_four_channel_specular_slot_four_bytes_per_texel_at_every_mip() {
    let fixture = Fixture::new();
    let name = "stone/slab".to_owned();
    fixture.diffuse_png(&name, 8, 4);
    fixture.gray_png("stone/slab_p", 8, 4, authored_smoothness);
    fixture.gray_png("stone/slab_t", 8, 4, authored_tint);
    let names = vec![name.clone()];
    let (_, _, cold) = fixture.bake(&names);
    let (_, _, warm) = fixture.bake(&names);
    for (run, bytes) in [("cold", &cold), ("warm", &warm)] {
        let (accounted_name, material) = bytes
            .summary()
            .entries()
            .next()
            .expect("the bundle is accounted");
        assert_eq!(accounted_name, name, "{run}");
        let specular = material.slot(1).expect("the specular slot is accounted");
        assert_eq!(specular.format, PrmFormat::Rgba8Unorm, "{run}");
        let mips: Vec<(u32, u32, u64)> = specular
            .levels
            .iter()
            .map(|mip| (mip.width, mip.height, mip.bytes))
            .collect();
        assert_eq!(
            mips,
            [(8, 4, 128), (4, 2, 32), (2, 1, 8), (1, 1, 4)],
            "{run}: four bytes per texel at every mip"
        );
        assert_eq!(specular.total_bytes(), 172, "{run}");
    }
}

// A `_p` or `_t` that disagrees with the diffuse or an earlier slot sibling
// fails the build, naming both files; matching dimensions pass.
#[test]
fn smoothness_and_tint_dimensions_must_match_the_diffuse_and_slot_siblings() {
    for (sibling, other) in [
        ("_p", ""),
        ("_p", "_s"),
        ("_p", "_h"),
        ("_t", ""),
        ("_t", "_s"),
        ("_t", "_h"),
        ("_t", "_p"),
    ] {
        let fixture = Fixture::new();
        fixture.gray_png(&format!("stone/cobble{other}"), 4, 4, |_, _| 128);
        fixture.gray_png(&format!("stone/cobble{sibling}"), 2, 4, |_, _| 128);
        let error = bake_materials_into(
            &["stone/cobble".to_owned()],
            &fixture.textures,
            &fixture.cache,
            &mut StageTextureBytes::new(),
            true,
        )
        .expect_err("mismatched slot sibling dimensions must fail the build");
        let message = error.to_string();
        for expected in [
            format!("cobble{sibling}.png"),
            format!("cobble{other}.png"),
            "2x4".to_owned(),
            "4x4".to_owned(),
        ] {
            assert!(
                message.contains(&expected),
                "{sibling} vs {other:?}: missing {expected:?} in {message}"
            );
        }
        assert!(fixture.sidecars().is_empty(), "{sibling} vs {other:?}");
    }

    let fixture = Fixture::new();
    SlotCase {
        diffuse: true,
        specular: true,
        height: true,
        smoothness: true,
        tint: true,
    }
    .write(&fixture);
    let (keys, _, _) = fixture.bake(&[COBBLE.to_owned()]);
    assert_ne!(keys[COBBLE], [0u8; 32]);
}

// `_n` and `_s` against the diffuse alone stay unenforced. A
// mismatched `_n` builds with or without `_p` and `_t`; a mismatched `_s`
// builds when neither is present.
#[test]
fn mismatched_normal_or_specular_builds_where_dimensions_are_unenforced() {
    for (smoothness, tint) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = Fixture::new();
        SlotCase {
            diffuse: true,
            smoothness,
            tint,
            ..NO_SIBLINGS
        }
        .write(&fixture);
        fixture.png("stone/cobble_n", 8, 8, |_, _| [127, 127, 255, 255]);
        let (keys, _, _) = fixture.bake(&[COBBLE.to_owned()]);
        let (header, slots) = fixture.read_sidecar(&keys[COBBLE]);
        assert!(header.slot_mask.contains(PrmSlots::NORMAL));
        assert_eq!(slots[2].as_ref().unwrap().width, 8);
        assert_eq!(
            header.slot_mask.contains(PrmSlots::SPECULAR),
            smoothness || tint,
            "_p {smoothness}, _t {tint}"
        );
    }

    let fixture = Fixture::new();
    fixture.diffuse_png(COBBLE, 4, 4);
    fixture.gray_png("stone/cobble_s", 2, 2, |_, _| 200);
    let (keys, _, _) = fixture.bake(&[COBBLE.to_owned()]);
    let (_, slots) = fixture.read_sidecar(&keys[COBBLE]);
    let specular = slots[1].as_ref().unwrap();
    assert_eq!(specular.format, PrmFormat::R8Unorm);
    assert_eq!((specular.width, specular.height), (2, 2));
}

// `_p` and `_t` ride the specular slot alone: the diffuse, normal and emissive
// slots bake byte-identical with and without them, and each bundle keeps a
// sidecar of its own.
#[test]
fn smoothness_and_tint_leave_diffuse_normal_and_emissive_slots_unchanged() {
    let fixture = Fixture::new();
    let variants = [
        ("panels/plain", false, false),
        ("panels/with_p", true, false),
        ("panels/with_t", false, true),
        ("panels/with_both", true, true),
    ];
    for (stem, smoothness, tint) in variants {
        fixture.diffuse_png(stem, 4, 4);
        fixture.png(&format!("{stem}_n"), 4, 4, |x, y| {
            [127 + (x * 20) as u8, 127 - (y * 20) as u8, 230, 255]
        });
        fixture.png(&format!("{stem}_e"), 4, 4, |x, _| {
            if x == 0 {
                [255, 64, 0, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        fixture.gray_png(&format!("{stem}_s"), 4, 4, authored_specular);
        if smoothness {
            fixture.gray_png(&format!("{stem}_p"), 4, 4, authored_smoothness);
        }
        if tint {
            fixture.gray_png(&format!("{stem}_t"), 4, 4, authored_tint);
        }
    }
    let names: Vec<String> = variants
        .iter()
        .map(|(stem, ..)| (*stem).to_owned())
        .collect();
    let (keys, _, _) = fixture.bake(&names);
    assert_eq!(fixture.sidecars().len(), 4, "none overwrites another");

    let (_, plain) = fixture.read_sidecar(&keys["panels/plain"]);
    assert_eq!(
        plain[1].as_ref().unwrap().format,
        PrmFormat::R8Unorm,
        "neither `_p` nor `_t` keeps the narrow slot"
    );
    for name in &names[1..] {
        let (_, slots) = fixture.read_sidecar(&keys[name]);
        for (index, label) in [(0, "diffuse"), (2, "normal"), (3, "emissive")] {
            assert_eq!(
                slots[index].as_ref().unwrap(),
                plain[index].as_ref().unwrap(),
                "{name}: the {label} slot must not change"
            );
        }
        assert_eq!(slots[1].as_ref().unwrap().format, PrmFormat::Rgba8Unorm);
    }
}
