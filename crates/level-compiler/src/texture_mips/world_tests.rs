// Material-stage bake: stats, sidecar reuse and repair, the fluid tail, and texel lifetimes.
// See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use super::probe_test_metrics as metrics;
use super::*;
use crate::surface_materials::stats::{BOUNCE_SATURATION, BounceMaterials, MaterialStats};
use log::Level;
use postretro_render_data::material::derive_material;
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
        let (keys, stats) =
            bake_materials_into(names, &self.textures, &self.cache, &mut bytes, true).unwrap();
        (keys, stats, bytes)
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

#[test]
fn material_stage_aliases_preserve_name_order_and_current_glow_strength() {
    const SLASH: &str = "panels/neon_panel";
    const SLASH_ALIAS: &str = "textures/PANELS/NEON_PANEL";
    const BACKSLASH: &str = "textures/PANELS\\NEON_PANEL";
    const BACKSLASH_ALIAS: &str = "PANELS\\neon_panel";
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
            fixture.png(&format!("{stem}_e"), 2, 1, |x, _| {
                if x == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 0, 255]
                }
            });
        }
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
                assert_eq!(observed.decode_attempts.len(), 2);
                assert!(observed.decode_attempts.values().all(|&count| count == 1));
                assert_eq!(observed.decode_attempts, observed.decode_successes);
            } else {
                assert!(observed.decode_attempts.is_empty());
            }
            assert_eq!(
                fixture.sidecars().len(),
                1,
                "content-identical sources share a sidecar"
            );
            assert!(keys.values().all(|key| *key == keys[&names[0]]));
            assert_eq!(bytes.summary().material_count(), 1);
            assert_eq!(bytes.summary().entries().next().unwrap().0, names[0]);
            let means: HashMap<_, _> = names
                .iter()
                .zip(&stats)
                .map(|(name, stat)| (name.as_str(), stat.snapped_mean()))
                .collect();
            for name in [SLASH, SLASH_ALIAS] {
                assert_eq!(
                    means[name],
                    Some([255; 3]),
                    "{name}, {phase}, reversed={reversed}"
                );
            }
            for name in [PLAIN, BACKSLASH, BACKSLASH_ALIAS] {
                assert_eq!(
                    means[name],
                    Some([255, 188, 188]),
                    "{name}, {phase}, reversed={reversed}"
                );
            }
            // Preserve runtime derivation: backslashes resolve source paths but
            // do not delimit the material prefix, so their glow strength is zero.
            let mut warned = HashSet::new();
            for name in [SLASH, SLASH_ALIAS] {
                assert!(derive_material(name, &mut warned).emissive_strength() > 0.0);
            }
            for name in [PLAIN, BACKSLASH, BACKSLASH_ALIAS] {
                assert!(
                    derive_material(name, &mut warned).emissive_strength().abs() < f32::EPSILON
                );
            }
            assert_eq!(means[SLASH], means[SLASH_ALIAS]);
            assert_eq!(means[BACKSLASH], means[BACKSLASH_ALIAS]);
            assert_ne!(
                means[SLASH], means[BACKSLASH],
                "shared sidecar must not merge different per-name glow strengths"
            );
        }
    }
}

#[test]
fn material_stage_rebakes_rejected_sidecars_before_deriving_stats() {
    let fixture = Fixture::new();
    fixture.png("panels/neon_panel", 4, 4, |x, _| {
        [255, (x * 60) as u8, 16, 255]
    });
    let names = vec!["panels/neon_panel".to_owned()];
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
    fixture.png("panels/neon_panel", 4, 2, |x, y| {
        [255, (x * 60) as u8, (y * 100) as u8, 255]
    });
    fixture.png("panels/neon_panel_e", 4, 2, |x, _| {
        if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
    });
    let names = vec!["panels/neon_panel".to_owned()];
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
        fixture.png("panels/neon_panel", 4, 2, |x, y| {
            [255, (x * 60) as u8, (y * 100) as u8, 255]
        });
        fixture.png("panels/neon_panel_e", 4, 2, |x, _| {
            if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
        });
        let names = vec!["panels/neon_panel".to_owned()];
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
            "rebaking cached sidecar for 'panels/neon_panel'",
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
        let names = vec!["panels/neon_panel".to_owned()];
        if mismatch {
            fixture.png("panels/neon_panel", 2, 2, |_, _| [255; 4]);
            fixture.png("panels/neon_panel_e", 1, 2, |_, _| [255; 4]);
        } else {
            std::fs::create_dir_all(fixture.textures.join("panels")).unwrap();
            std::fs::write(fixture.textures.join("panels/neon_panel.png"), b"not a PNG").unwrap();
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
        let (mut split_keys, stats) = bake_materials_into(
            &names[..3],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            true,
        )
        .unwrap();
        assert_eq!(stats.len(), 3);
        assert!(stats.iter().all(|stat| stat.snapped_mean().is_some()));
        let (tail, tail_stats) = bake_materials_into(
            &names[3..],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            false,
        )
        .unwrap();
        assert!(tail_stats.is_empty());
        let observed = observation.snapshot();
        drop(observation);
        if phase == "cold" {
            assert_eq!(observed.decode_attempts.len(), 4);
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
        let (keys, stats) = bake_materials_into(
            &[],
            &fixture.textures,
            &fixture.cache,
            &mut split_bytes,
            false,
        )
        .unwrap();
        assert!(keys.is_empty() && stats.is_empty());
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

#[test]
fn material_stage_glow_names_exclude_only_masked_texels_and_fall_back_to_the_full_average() {
    const STRIP: [u8; 3] = [255, 255, 255];
    const PANEL: [u8; 3] = [40, 80, 160];
    let fixture = Fixture::new();
    let diffuse = |x: u32, _| {
        let [r, g, b] = if x < 2 { STRIP } else { PANEL };
        [r, g, b, 255]
    };
    // One painted strip; three glow-prefixed names that differ only in `_e`.
    for stem in ["panels/neon_bare", "panels/neon_strip", "panels/neon_all"] {
        fixture.png(stem, 8, 2, diffuse);
    }
    fixture.png("panels/neon_strip_e", 8, 2, |x, _| {
        if x < 2 {
            [255, 64, 0, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    fixture.png("panels/neon_all_e", 8, 2, |_, _| [0, 32, 0, 255]);
    let names: Vec<_> = ["panels/neon_bare", "panels/neon_strip", "panels/neon_all"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut warned = HashSet::new();
    for name in &names {
        assert!(
            derive_material(name, &mut warned).emissive_strength() > 0.0,
            "{name} must be a glowing material"
        );
    }
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
    }
}

#[test]
fn material_stage_texel_lifetimes_stay_with_one_texture_and_return_fixed_records() {
    let fixture = Fixture::new();
    let names: Vec<_> = (0..32)
        .map(|index| format!("panels/neon_{index}"))
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
