//! `light_area` warm rebuilds through the real compiler CLI, with every probe
//! arm live: keys an area light ignores re-key no stage, and
//! `--soft-shadow-samples` re-keys only the texel paths.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline (Area lights)

mod coloured_bounce_support;

use std::collections::BTreeMap;

use coloured_bounce_support::fixtures::{box_brush, mover_entity, write_texture};
use coloured_bounce_support::{Bake, Fixture, cache_tally, current_compiler};

const TALLY_LOG: &str = "info,prl_build::delta_sh_cache=debug";
const GLASS: &str = "bounce/glass";

/// Keys a converted `light` or `light_spot` leaves on an area light; each
/// warns and is ignored.
const INERT_KEYS: &str = "\"delay\" \"0\"\n\"_light_size\" \"16\"\n\"_cone\" \"20\"\n\
                          \"_cone2\" \"30\"\n\"angle\" \"90\"\n\"mangle\" \"0 -90 0\"\n\
                          \"target\" \"t1\"\n";

/// A sealed white room with two ceiling panels over a post, each a glass slab
/// beside a white frame: a static `light_area` and an animated one, so the weight-map,
/// animated direct-SH and bounce-delta stages bake too. A ping-pong lift sits
/// under both, so the mover light gate and probe channel bake. `keys` are
/// extra `"key" "value"` lines on both lights.
fn area_map(keys: &str) -> String {
    let white = "bounce/white";
    let world = [
        box_brush([-144, -144, -16], [144, 144, 0], white),
        box_brush([-144, -144, 128], [144, 144, 144], white),
        box_brush([-144, -144, 0], [-128, 144, 128], white),
        box_brush([128, -144, 0], [144, 144, 128], white),
        box_brush([-128, -144, 0], [128, -128, 128], white),
        box_brush([-128, 128, 0], [128, 144, 128], white),
        // A post under the panels, so the lightmap holds penumbrae.
        box_brush([-16, 48, 0], [0, 64, 96], white),
    ]
    .join("\n");
    let panel = |x0: i32, animated: &str| {
        let slab = box_brush([x0, -32, 120], [x0 + 48, 32, 128], GLASS);
        let frame = box_brush([x0 + 48, -32, 120], [x0 + 56, 32, 128], white);
        format!(
            "{{\n\"classname\" \"light_area\"\n\"light\" \"300\"\n\"_color\" \"255 224 192\"\n\
             \"_falloff_range\" \"256\"\n{animated}{keys}{slab}\n{frame}\n}}\n"
        )
    };
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n\
         {{\n\"classname\" \"player_spawn\"\n\"origin\" \"-64 64 32\"\n}}\n\
         {}{}{}",
        panel(-96, ""),
        panel(32, "\"_animated\" \"1\"\n"),
        mover_entity(white),
    )
}

fn fixture(map: &str) -> Fixture {
    // The cache checks below count lightmap layer and section reads, which
    // the reach cull's verify mode never makes.
    let mut fixture = Fixture::new(map, true);
    fixture.reach_verify = false;
    write_texture(fixture.root(), "glass", [255, 255, 255]);
    write_texture(fixture.root(), "glass_e", [255, 255, 255]);
    fixture
}

/// A warm build into the fixture's one cache at `samples` texel samples.
fn bake(fixture: &Fixture, label: &str, samples: &str) -> Bake {
    let cache_dir = fixture.root().join("cache").to_string_lossy().into_owned();
    fixture.bake(
        &current_compiler(),
        label,
        TALLY_LOG,
        &[
            "--jobs",
            "2",
            "--soft-shadow-samples",
            samples,
            "--cache-dir",
            cache_dir.as_str(),
        ],
    )
}

/// Probe-side stage families every warm rebuild below must read from the
/// cache: one of each group must hit. SH bounce reads its gathered volume or
/// its patch layers; the rest log one entry or memo line per build.
const PROBE_STAGES: [&[&str]; 7] = [
    &["sh_base_volume", "sh_patch_layer"],
    &["direct_sh_volume"],
    &[
        "billboard_direct_scatter",
        "animated_billboard_direct_scatter",
    ],
    &["animated_direct_sh_delta_light"],
    &["indirect_delta_sh_subblock"],
    &["mover_probe_direct"],
    &["mover_probe_bounce"],
];

/// Each family in [`PROBE_STAGES`] hits at least once and misses nowhere.
fn probe_stages_hit(tally: &BTreeMap<String, (usize, usize)>, case: &str, log: &str) {
    for family in PROBE_STAGES {
        let matching: Vec<_> = tally
            .iter()
            .filter(|(stage, _)| family.contains(&stage.as_str()))
            .collect();
        assert!(
            matching.iter().any(|(_, (hits, _))| *hits > 0),
            "{case}: {family:?} must read from the cache: {tally:?}\n{log}"
        );
        for (stage, (_, misses)) in matching {
            assert_eq!(*misses, 0, "{case}: {stage} must stay warm: {tally:?}");
        }
    }
}

// Acceptance row 31: editing only `delay`, `_light_size` or a spot key on an
// area light re-keys no bake cache entry. With every probe arm live, a warm
// rebuild after adding them to a static and an animated area light reads
// every lightmap, surface-radiance, SH, scatter, weight-map, bounce-delta and
// mover entry from the cache, misses nothing, and packs the same bytes.
#[test]
fn inert_keys_on_area_lights_rebuild_every_stage_from_the_cache() {
    let fixture = fixture(&area_map(""));
    let base = bake(&fixture, "base", "4");
    fixture.edit_map(|_| area_map(INERT_KEYS));
    let warm = bake(&fixture, "inert", "4");

    let tally = cache_tally(&warm.log);
    for (stage, (_, misses)) in &tally {
        assert_eq!(*misses, 0, "{stage} re-keyed: {tally:?}\n{}", warm.log);
    }
    assert!(
        ["lightmap_section", "lightmap_layer"]
            .iter()
            .any(|stage| tally.get(*stage).is_some_and(|entry| entry.0 > 0)),
        "the lightmap reads from the cache: {tally:?}"
    );
    assert!(
        tally
            .get("animated_lm_weight_maps")
            .is_some_and(|entry| entry.0 > 0),
        "the weight maps read from the cache: {tally:?}"
    );
    probe_stages_hit(&tally, "inert keys", &warm.log);
    assert!(
        warm.log
            .contains("`_light_size` does not apply to an area light"),
        "the inert keys reached the compiler:\n{}",
        warm.log
    );
    assert_eq!(
        base.sections, warm.sections,
        "inert keys change no PRL section"
    );
}

// Acceptance row 37: raising `--soft-shadow-samples` on an area-light map
// re-bakes its lightmap layers and the weight-map stage, and a warm rebuild
// still reads its surface-radiance, direct-SH, scatter, mover-channel and
// bounce-delta entries from the cache. Those sections keep their bytes: the
// warm 16-sample PRL matches a cold (`--no-cache`) 16-sample build section for
// section, so no probe path reads the count without keying it.
#[test]
fn soft_shadow_samples_rekey_only_the_texel_paths_of_area_lights() {
    let fixture = fixture(&area_map(""));
    let base = bake(&fixture, "four", "4");
    // The receiver floor takes at least nine strata, so 4 and 8 would bake
    // alike; 16 moves the texels.
    let raised = bake(&fixture, "sixteen", "16");

    let tally = cache_tally(&raised.log);
    let missed: Vec<&str> = tally
        .iter()
        .filter(|(_, (_, misses))| *misses > 0)
        .map(|(stage, _)| stage.as_str())
        .collect();
    // The section memo misses and its layers re-bake (layer entries log
    // below info); the weight-map stage and the mover light gate's texel
    // tier, which reads the baked visibility, are texel paths too, as is its
    // animated tier, which reads the new weights.
    for stage in [
        "lightmap_section",
        "animated_lm_weight_maps",
        "mover_light_texels",
    ] {
        assert!(
            missed.contains(&stage),
            "{stage} re-keys on the texel sample count: {tally:?}
{}",
            raised.log
        );
    }
    for stage in &missed {
        assert!(
            [
                "lightmap_section",
                "lightmap_layer",
                "shadowmask_atlas",
                "animated_lm_weight_maps",
                "mover_light_texels",
                "mover_light_animated",
            ]
            .contains(stage),
            "{stage} re-keyed on a texel sample count: {tally:?}
{}",
            raised.log
        );
    }
    probe_stages_hit(&tally, "--soft-shadow-samples", &raised.log);

    // A cold build at the same count is the reference: cached probe stages
    // can only equal it if the count never reaches them unkeyed.
    let cold = fixture.bake(
        &current_compiler(),
        "sixteen_cold",
        TALLY_LOG,
        &["--jobs", "2", "--soft-shadow-samples", "16", "--no-cache"],
    );
    assert_eq!(
        raised.sections, cold.sections,
        "the warm 16-sample PRL differs from a cold 16-sample build"
    );

    // Texel outputs move; every probe-side section keeps its bytes.
    let texel_paths: [u32; 3] = [22, 25, 42];
    for (id, payload) in &base.sections {
        if texel_paths.contains(id) {
            continue;
        }
        assert!(
            raised.sections.get(id) == Some(payload),
            "section {id} changed with the texel sample count"
        );
    }
    assert_ne!(
        base.sections.get(&22),
        raised.sections.get(&22),
        "the lightmap re-baked at the new count"
    );
}
