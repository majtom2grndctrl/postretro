//! PNG edits, sidecar reuse, stage-cache contents and material-stage ordering
//! through the real compiler CLI.
//! See: context/lib/build_pipeline.md §Compiler pipeline, §Build Cache

mod coloured_bounce_support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use coloured_bounce_support::fixtures;
use coloured_bounce_support::{
    Bake, DELTA_START, FOUR_SHADOW_SAMPLES, Fixture, Run, SH_START, TAIL_START,
    TEXTURE_VALIDATION_START, cache_tally, corrupt_pixels, current_compiler, grey_compiler,
    sidecars, write_png,
};

/// Per-entry delta cache lines are debug-level; the tallies read them.
const TALLY_LOG: &str = "info,prl_build::delta_sh_cache=debug";
const LABEL: &str = "room";

/// Every stage that caches an entry for the red-wall room. Each existed, and
/// cached its entries, before coloured bounce; that change added none.
const PRE_CHANGE_CACHED_STAGES: [&str; 14] = [
    "animated_billboard_direct_scatter",
    "animated_lm_weight_maps",
    "billboard_direct_scatter",
    "cell_residency_set",
    "cell_visibility",
    "chunk_light_list",
    "direct_sh_volume",
    "indirect_delta_sh_subblock",
    "lightmap_layer",
    "lightmap_section",
    "navmesh",
    "reflection_probes",
    "sh_group",
    "shadowmask_atlas",
];

/// The two stages whose keys fold the material table: the SH base's
/// per-light patch layers and the indirect deltas.
const SH_STAGES: [&str; 2] = ["sh_patch_layer", "indirect_delta_sh_subblock"];

fn cache_dir(fixture: &Fixture) -> PathBuf {
    fixture.root().join("cache")
}

fn run_with(fixture: &Fixture, compiler: &Path, cache: bool, workers: u32, extra: &[&str]) -> Run {
    let workers = workers.to_string();
    let cache_dir = cache_dir(fixture).to_string_lossy().into_owned();
    let mut flags = vec!["--jobs", workers.as_str()];
    flags.extend(FOUR_SHADOW_SAMPLES);
    if cache {
        flags.extend(["--cache-dir", cache_dir.as_str()]);
    } else {
        flags.push("--no-cache");
    }
    flags.extend(extra);
    fixture.run(compiler, LABEL, TALLY_LOG, &flags)
}

fn run(fixture: &Fixture, cache: bool, workers: u32, extra: &[&str]) -> Run {
    run_with(fixture, &current_compiler(), cache, workers, extra)
}

fn bake(fixture: &Fixture, cache: bool, workers: u32) -> Bake {
    run(fixture, cache, workers, &[]).into_bake()
}

/// The bounce-bearing sections: base SH (34) and indirect deltas (27).
fn lighting(bake: &Bake) -> BTreeMap<u32, Vec<u8>> {
    bake.required(&[34, 27])
}

fn tally(log: &str, stage: &str) -> (usize, usize) {
    cache_tally(log).get(stage).copied().unwrap_or_default()
}

fn all_miss(bake: &Bake, totals: Option<&Bake>) {
    for stage in SH_STAGES {
        let (hits, misses) = tally(&bake.log, stage);
        assert!(
            hits == 0 && misses > 0,
            "{stage} must rebake every entry:\n{}",
            bake.log
        );
        if let Some(previous) = totals {
            let (old_hits, old_misses) = tally(&previous.log, stage);
            assert_eq!(
                misses,
                old_hits + old_misses,
                "{stage} entry count stays fixed"
            );
        }
    }
}

fn all_hit(bake: &Bake, previous: &Bake) {
    for stage in SH_STAGES {
        let (hits, misses) = tally(&bake.log, stage);
        let (old_hits, old_misses) = tally(&previous.log, stage);
        assert!(
            hits > 0 && misses == 0,
            "{stage} must reuse every entry:\n{}",
            bake.log
        );
        assert_eq!(
            hits,
            old_hits + old_misses,
            "{stage} entry count stays fixed"
        );
    }
}

/// Top-level cache entries by file name. Entries are content digests; the
/// only directory a build may add is the prune's per-map `records/`.
fn cache_entries(dir: &Path) -> BTreeSet<String> {
    let mut entries = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_dir() {
            assert_eq!(name, "records", "unexpected cache subdirectory");
            continue;
        }
        assert!(
            name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "cache holds only digest-named stage entries, found {name}"
        );
        entries.insert(name);
    }
    entries
}

#[test]
#[ignore = "focused cold-bake/material-cache integration gate"]
fn mean_edits_rekey_every_group_and_entry_but_auxiliary_and_fluid_edits_hit() {
    let fixture = Fixture::red_wall();
    let first = bake(&fixture, true, 1);
    all_miss(&first, None);
    let warm = bake(&fixture, true, 1);
    all_hit(&warm, &first);
    assert_eq!(lighting(&first), lighting(&warm));
    assert_eq!(first.sidecars, warm.sidecars);

    // One 255→254 red texel out of 256 leaves the 8-bit mean at 255.
    write_png(&fixture.texture("red"), |i| {
        [if i == 0 { 254 } else { 255 }, 0, 0, 255]
    });
    let tiny = bake(&fixture, true, 1);
    all_hit(&tiny, &warm);
    assert_eq!(lighting(&tiny), lighting(&warm));
    assert_ne!(
        tiny.sidecars, warm.sidecars,
        "changed source still produces a new sidecar"
    );

    for (name, colour) in [
        ("red_n", [128, 128, 255, 255]),
        ("red_s", [64; 4]),
        ("water", [127, 80, 20, 255]),
    ] {
        write_png(&fixture.texture(name), |_| colour);
        let edited = bake(&fixture, true, 2);
        all_hit(&edited, &tiny);
        assert_eq!(
            lighting(&edited),
            lighting(&tiny),
            "{name} does not affect static-world bounce"
        );
    }
    // A regional repaint moves the mean, so every group and indirect entry misses.
    write_png(&fixture.texture("red"), |i| {
        if i < 128 {
            [0, 0, 255, 255]
        } else {
            [255, 0, 0, 255]
        }
    });
    let repainted = bake(&fixture, true, 2);
    all_miss(&repainted, Some(&tiny));
    for id in [34, 27] {
        assert_ne!(
            repainted.sections[&id], tiny.sections[&id],
            "mean repaint must change id {id}"
        );
    }
    let repeated = bake(&fixture, true, 1);
    all_hit(&repeated, &repainted);
    assert_eq!(lighting(&repainted), lighting(&repeated));
}

#[test]
#[ignore = "focused cold-bake/glow-cache integration gate"]
fn glow_mask_changes_rekey_but_nonzero_emissive_intensity_changes_hit() {
    let fixture = Fixture::red_wall();
    fixture.edit_map(|map| map.replace("bounce/red", "bounce/neon_panel"));
    write_png(&fixture.texture("neon_panel"), |i| {
        if i < 128 { [255; 4] } else { [64, 16, 16, 255] }
    });
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i < 128 {
            [1, 0, 0, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let first = bake(&fixture, true, 1);
    all_miss(&first, None);
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i < 128 {
            [255, 120, 20, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let brighter = bake(&fixture, true, 2);
    all_hit(&brighter, &first);
    assert_eq!(lighting(&first), lighting(&brighter));
    assert_ne!(first.sidecars, brighter.sidecars);
    // One extra marked dark texel leaves the retained constant colour unchanged.
    write_png(&fixture.texture("neon_panel_e"), |i| {
        if i <= 128 {
            [255, 120, 20, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let unchanged_mean = bake(&fixture, true, 1);
    all_hit(&unchanged_mean, &brighter);
    assert_eq!(lighting(&brighter), lighting(&unchanged_mean));
    write_png(&fixture.texture("neon_panel_e"), |_| [0, 0, 0, 255]);
    let unmasked = bake(&fixture, true, 1);
    all_miss(&unmasked, Some(&first));
    assert_ne!(unmasked.sections[&27], first.sections[&27]);
}

#[test]
#[ignore = "focused sidecar/cache/worker determinism gate"]
fn sidecar_reuse_and_rebake_preserve_bytes_with_and_without_stage_cache() {
    let mut cold_reference = None;
    for cache in [false, true] {
        let fixture = Fixture::red_wall();
        let first = bake(&fixture, cache, 1);
        assert!(!first.sidecars.is_empty());
        let reused = bake(&fixture, cache, 2);
        assert_eq!(
            lighting(&first),
            lighting(&reused),
            "sidecars reused with stage cache {cache}"
        );
        assert_eq!(first.sidecars, reused.sidecars);
        if cache {
            all_miss(&first, None);
            all_hit(&reused, &first);
        } else {
            for stage in SH_STAGES {
                assert_eq!(
                    tally(&first.log, stage),
                    (0, 0),
                    "no-cache builds construct no stage cache key"
                );
            }
        }
        std::fs::remove_dir_all(fixture.materials(LABEL)).unwrap();
        let rebuilt = bake(&fixture, cache, 2);
        assert_eq!(
            lighting(&first),
            lighting(&rebuilt),
            "rebuilt sidecars retain lighting bytes"
        );
        assert_eq!(
            first.sidecars, rebuilt.sidecars,
            "sidecar format and bytes stay fixed"
        );
        if cache {
            all_hit(&rebuilt, &reused);
            let cold: &Bake = cold_reference.as_ref().unwrap();
            assert_eq!(
                rebuilt.sections[&27], cold.sections[&27],
                "warm indirect deltas are exact"
            );
            assert_eq!(
                rebuilt.sections[&34], cold.sections[&34],
                "warm id 34 bakes the cold bytes"
            );
            // Warm sidecars must also work when the stage cache is empty.
            std::fs::remove_dir_all(cache_dir(&fixture)).unwrap();
            let empty_cache = bake(&fixture, true, 2);
            all_miss(&empty_cache, Some(&first));
            assert_eq!(lighting(&empty_cache), lighting(&first));
            assert_eq!(empty_cache.sidecars, first.sidecars);
            // A rejected sidecar is regenerated before a populated SH cache hits.
            let name = empty_cache.sidecars.keys().next().unwrap();
            std::fs::write(fixture.materials(LABEL).join(name), b"truncated prm").unwrap();
            let repaired = bake(&fixture, true, 1);
            all_hit(&repaired, &empty_cache);
            assert_eq!(lighting(&repaired), lighting(&first));
            assert_eq!(repaired.sidecars, first.sidecars);
        } else {
            cold_reference = Some(first);
        }
    }
}

// The build writes no cache entry for means: every entry a cold build writes
// belongs to one logged miss of a stage that cached before coloured bounce,
// and a warm build reads them all and writes nothing new.
#[test]
#[ignore = "focused cold/warm stage-cache contents gate"]
fn cold_and_warm_builds_cache_entries_only_for_pre_change_stages() {
    let fixture = Fixture::red_wall();
    // `-v` logs the lightmap layer partitions' own cache lines.
    let cold = run(&fixture, true, 1, &["-v"]).into_bake();
    let cold_tally = cache_tally(&cold.log);
    assert_eq!(
        cold_tally.keys().map(String::as_str).collect::<Vec<_>>(),
        PRE_CHANGE_CACHED_STAGES,
        "cold build caches exactly the pre-change stages:\n{}",
        cold.log
    );
    for (stage, (hits, _)) in &cold_tally {
        assert_eq!(*hits, 0, "{stage} has nothing to hit in a fresh cache");
    }
    let cold_entries = cache_entries(&cache_dir(&fixture));
    let cold_misses: usize = cold_tally.values().map(|(_, misses)| misses).sum();
    assert_eq!(
        cold_entries.len(),
        cold_misses,
        "one entry per pre-change stage miss, and no other entry"
    );

    let warm = run(&fixture, true, 1, &["-v"]).into_bake();
    let warm_tally = cache_tally(&warm.log);
    for (stage, (hits, misses)) in &warm_tally {
        assert!(
            PRE_CHANGE_CACHED_STAGES.contains(&stage.as_str()),
            "warm build reads an unknown stage {stage}"
        );
        assert_eq!(*misses, 0, "{stage} must be warm:\n{}", warm.log);
        assert_eq!(*hits, cold_tally[stage].1, "{stage} reads every entry");
    }
    for stage in PRE_CHANGE_CACHED_STAGES {
        // A lightmap section hit serves the layer without reading partitions.
        if stage != "lightmap_layer" {
            assert!(warm_tally.contains_key(stage), "{stage} must be read warm");
        }
    }
    assert_eq!(
        cache_entries(&cache_dir(&fixture)),
        cold_entries,
        "a warm build writes no entry"
    );
    assert_eq!(lighting(&cold), lighting(&warm));
}

/// One-shot pre-change oracle: a stage cache the saved pre-colour compiler
/// wrote misses every SH group and indirect delta entry, while the stages
/// coloured bounce left alone hit that same cache. A fresh cache from either
/// compiler holds the same number of entries.
#[test]
#[ignore = "one-shot pre-change oracle: needs POSTRETRO_GREY_PRL_BUILD"]
fn pre_colour_stage_cache_misses_both_sh_stages_while_unchanged_stages_hit() {
    let Some(grey) =
        grey_compiler("pre_colour_stage_cache_misses_both_sh_stages_while_unchanged_stages_hit")
    else {
        return;
    };
    let fixture = Fixture::red_wall();
    let old = run_with(&fixture, &grey, true, 1, &[]).into_bake();
    let old_entries = cache_entries(&cache_dir(&fixture));
    let (_, old_groups) = tally(&old.log, "sh_group");
    assert!(old_groups > 0, "pre-colour build cached SH groups");

    let current = bake(&fixture, true, 1);
    let current_tally = cache_tally(&current.log);
    assert_eq!(
        tally(&current.log, "sh_group"),
        (0, old_groups),
        "every pre-colour group entry misses"
    );
    let (delta_hits, delta_misses) = tally(&current.log, "indirect_delta_sh_subblock");
    assert!(
        delta_hits == 0 && delta_misses > 0,
        "every pre-colour indirect delta entry misses:\n{}",
        current.log
    );
    // Reflection captures key on the SH bytes, so they follow SH; every other
    // cached stage must find the pre-colour entry, proving the cache was read.
    for (stage, (hits, misses)) in &current_tally {
        if SH_STAGES.contains(&stage.as_str()) || stage == "reflection_probes" {
            continue;
        }
        assert!(
            *hits > 0 && *misses == 0,
            "{stage} is unchanged by colour and must hit the old cache"
        );
    }
    assert!(current_tally.len() > SH_STAGES.len() + 1);

    let fresh = Fixture::red_wall();
    run(&fresh, true, 1, &[]).into_bake();
    assert_eq!(
        cache_entries(&cache_dir(&fresh)).len(),
        old_entries.len(),
        "coloured bounce caches no entry the pre-colour build did not"
    );
}

#[test]
#[ignore = "focused compiler refusal-before-materials gate"]
fn budget_and_invalid_animation_fail_before_corrupt_world_texture_is_read() {
    let fixture = Fixture::red_wall();
    corrupt_pixels(&fixture.texture("red"));
    let refused = run(&fixture, false, 1, &["--working-set-max-size", "0"]);
    let log = &refused.log;
    assert!(!refused.succeeded());
    // A zero budget fails both SH gates; the cheaper prediction refuses by
    // its own name (research P24).
    assert!(
        log.contains("SH base working-set gate refused before patch shading")
            || log.contains("SH delta working-set gate refused before dense baking"),
        "{log}"
    );
    assert!(
        !log.contains(TEXTURE_VALIDATION_START),
        "a refusing gate must precede texture-root reads: {log}"
    );
    assert!(
        !log.contains("decoding PNG"),
        "gate must beat bad source: {log}"
    );
    assert!(!log.contains(SH_START), "{log}");
    assert!(sidecars(&fixture.materials(LABEL)).is_empty());

    fixture.edit_map(|map| {
        map.replacen(
            "\"_color\" \"255 255 255\"",
            "\"_color\" \"255 255 255\"\n\"color_curve\" \"[0, 1, 0, 0] [500, 0, 1, 0]\"\n\"period_ms\" \"500\"",
            1,
        )
    });
    let invalid = run(&fixture, false, 1, &[]);
    let log = &invalid.log;
    assert!(!invalid.succeeded());
    assert!(log.contains("color animation is only valid on"), "{log}");
    assert!(
        !log.contains(TEXTURE_VALIDATION_START),
        "invalid animation must precede texture-root reads: {log}"
    );
    assert!(!log.contains(SH_START), "{log}");
    assert!(sidecars(&fixture.materials(LABEL)).is_empty());
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    /// Valid PNG header, undecodable pixels.
    Decode,
    /// An `_e` at half the diffuse's dimensions.
    EmissiveSize,
}

#[test]
#[ignore = "focused world/mover-versus-fluid material failure ordering gate"]
fn world_and_mover_faults_precede_sh_while_fluid_only_faults_follow_both_sh_bakes() {
    for (name, fault, late) in [
        ("red", Fault::Decode, false),
        ("red", Fault::EmissiveSize, false),
        ("lift", Fault::Decode, false),
        ("lift", Fault::EmissiveSize, false),
        ("water", Fault::Decode, true),
        ("water", Fault::EmissiveSize, true),
    ] {
        let case = format!("{name} {fault:?}");
        let fixture = Fixture::red_wall();
        // The mover's name is interned at Geometry, so it belongs to the
        // material stage even though no static-world face uses it.
        fixture.edit_map(|map| map + &fixtures::mover_entity("bounce/lift"));
        fixtures::write_texture(fixture.root(), "lift", [40, 160, 40]);
        let expected = match fault {
            Fault::Decode => {
                corrupt_pixels(&fixture.texture(name));
                format!("{name}.png")
            }
            Fault::EmissiveSize => {
                image::RgbaImage::from_pixel(8, 8, image::Rgba([255; 4]))
                    .save(fixture.texture(&format!("{name}_e")))
                    .unwrap();
                format!("{name}_e.png is 8x8")
            }
        };
        let output = run(&fixture, false, 1, &[]);
        let log = &output.log;
        assert!(!output.succeeded(), "{case} must fail the build");
        assert!(log.contains(&expected), "{case} must name its fault: {log}");
        if matches!(fault, Fault::Decode) {
            assert!(log.contains("decoding PNG"), "{case}: {log}");
        }
        assert_eq!(
            log.contains(SH_START),
            late,
            "{case}: world and mover materials fail before SH: {log}"
        );
        assert_eq!(
            log.contains(DELTA_START),
            late,
            "{case}: fluid-only faults follow delta SH: {log}"
        );
        assert_eq!(
            log.contains(TAIL_START),
            late,
            "{case}: fluid-only sources belong to the texture tail: {log}"
        );
        if late {
            assert!(
                !sidecars(&fixture.materials(LABEL)).is_empty(),
                "{case}: world materials already baked"
            );
        }
    }
}
