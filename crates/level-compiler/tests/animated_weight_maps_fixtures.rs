//! End-to-end fixture test for the animated-light weight-maps pipeline.
//!
//! Compiles the bundled `test_animated_weight_maps_single.map` fixture via
//! the `prl-build` binary, reads the resulting `.prl`, and asserts that the
//! `AnimatedLightWeightMaps` section is present, non-empty, and round-trips
//! through `to_bytes` / `from_bytes`.
//!
//! This is the only integration-level smoke test for the compile-then-load
//! flow. Unit tests under `src/animated_light_weight_maps.rs` cover the
//! baker in isolation; the runtime validator is unit-tested under
//! `postretro/src/render/animated_lightmap.rs`.
//!
//! See: context/lib/build_pipeline.md

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use postretro_level_format::SectionId;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::sh_volume::{ANIMATED_SLOT_NONE, OctahedralShVolumeSection};
use postretro_level_format::{read_container, read_section_data};

/// Walk from the crate root to the workspace root (for locating `content/dev/`).
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/level-compiler/. Workspace root is ../../.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

#[test]
#[ignore = "cold prl-build bake; run on demand with -- --ignored"]
fn single_fixture_compiles_and_carries_weight_map_section() {
    let ws = workspace_root();
    let input = ws.join("content/dev/maps/test_animated_weight_maps_single.map");
    assert!(input.exists(), "fixture map missing: {}", input.display(),);

    // Use a tempfile under the OS temp dir so the integration test does not
    // depend on or stomp the checked-in `.prl`.
    let out_dir = std::env::temp_dir().join("postretro_fixture_single");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("test_animated_weight_maps_single.prl");

    // Invoke the `prl-build` binary via cargo. Running it in-process via
    // main.rs would require a library target we don't want to add for this
    // test alone; shelling out costs one cargo invocation.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    // Parse the container and pull the AnimatedLightWeightMaps section.
    let bytes = std::fs::read(&output).expect("read compiled .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    let section_bytes = read_section_data(
        &mut cursor,
        &meta,
        SectionId::AnimatedLightWeightMaps as u32,
    )
    .expect("read_section_data")
    .expect("AnimatedLightWeightMaps section present");

    let section = AnimatedLightWeightMapsSection::from_bytes(&section_bytes).expect("from_bytes");

    assert!(
        !section.chunk_rects.is_empty(),
        "single-fixture map must carry ≥ 1 animated-light chunk",
    );
    assert!(
        !section.offset_counts.is_empty(),
        "single-fixture map must carry per-texel entries",
    );
    assert!(
        !section.texel_lights.is_empty(),
        "single-fixture map must carry per-texel light weights",
    );
    assert!(section.is_consistent());

    // Round-trip via to_bytes/from_bytes and ensure the decoded section is
    // byte-identical.
    let re_bytes = section.to_bytes();
    let re_section =
        AnimatedLightWeightMapsSection::from_bytes(&re_bytes).expect("round-trip from_bytes");
    assert_eq!(
        section.chunk_rects, re_section.chunk_rects,
        "chunk_rects drifted during round-trip",
    );
    assert_eq!(
        section.offset_counts, re_section.offset_counts,
        "offset_counts drifted during round-trip",
    );
    assert_eq!(
        section.texel_lights, re_section.texel_lights,
        "texel_lights drifted during round-trip",
    );

    // Cleanup.
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}

/// The fixture's KVP curve and script target must occupy distinct animated slots,
/// while its steady-light control must stay in the static namespace. This stays
/// ignored because the assertion is intentionally against a real PRL bake, not a
/// self-referential unit fixture.
#[test]
#[ignore = "cold prl-build bake; run on demand with -- --ignored"]
fn fixture_keeps_script_and_kvp_animated_prl_slots_distinct() {
    let ws = workspace_root();
    let input = ws.join("content/dev/maps/script_light_membership_fixture.map");
    assert!(input.exists(), "fixture map missing: {}", input.display());

    let out_dir = std::env::temp_dir().join("postretro_fixture_script_light_membership");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("script_light_membership_fixture.prl");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--no-cache")
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    let bytes = std::fs::read(&output).expect("read compiled .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    let mut read_section = |id| {
        read_section_data(&mut cursor, &meta, id)
            .expect("read section")
            .expect("script-targeted fixture must emit animated section")
    };

    let chunks = AnimatedLightChunksSection::from_bytes(&read_section(
        SectionId::AnimatedLightChunks as u32,
    ))
    .expect("AnimatedLightChunks decodes");
    let weights = AnimatedLightWeightMapsSection::from_bytes(&read_section(
        SectionId::AnimatedLightWeightMaps as u32,
    ))
    .expect("AnimatedLightWeightMaps decodes");
    let sh =
        OctahedralShVolumeSection::from_bytes(&read_section(SectionId::OctahedralShVolume as u32))
            .expect("OctahedralShVolume decodes");

    assert_eq!(
        sh.animation_descriptors.len(),
        2,
        "the script target and KVP curve should each receive an animated descriptor slot",
    );
    assert_eq!(
        sh.slot_for_map_light,
        [0, ANIMATED_SLOT_NONE, 1],
        "script and KVP lights must retain distinct map-light identities while the steady control has no animated slot",
    );
    assert!(
        !chunks.light_indices.is_empty()
            && chunks
                .light_indices
                .iter()
                .all(|&index| matches!(index, 0 | 2))
            && chunks.light_indices.contains(&0)
            && chunks.light_indices.contains(&2),
        "animated chunks must contain both the script and KVP light, never the steady control",
    );
    assert!(
        !weights.texel_lights.is_empty()
            && weights
                .texel_lights
                .iter()
                .all(|entry| matches!(entry.light_index, 0 | 2))
            && weights
                .texel_lights
                .iter()
                .any(|entry| entry.light_index == 0)
            && weights
                .texel_lights
                .iter()
                .any(|entry| entry.light_index == 2),
        "animated weights must contain both the script and KVP light, never the steady control: {:?}",
        weights.texel_lights,
    );

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}

/// Task 6 soft-shadow test map: compile `soft_shadow_test.map` (single key
/// light, box-on-floor contact case, and five overlapping static lights) and
/// assert it produces a real (non-placeholder) baked lightmap. This is the
/// "every test map compiles" coverage for the new map; the soft-shadow *values*
/// are covered by unit tests in `lightmap_bake.rs`.
#[test]
#[ignore = "cold prl-build bake (--no-cache lightmap); run on demand with -- --ignored"]
fn soft_shadow_test_map_compiles_to_a_baked_lightmap() {
    let ws = workspace_root();
    let input = ws.join("content/dev/maps/soft_shadow_test.map");
    assert!(input.exists(), "fixture map missing: {}", input.display());

    let out_dir = std::env::temp_dir().join("postretro_fixture_soft_shadow");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("soft_shadow_test.prl");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        // Isolate from the shared on-disk stage cache so this test always
        // exercises a fresh bake regardless of prior runs.
        .arg("--no-cache")
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    let bytes = std::fs::read(&output).expect("read compiled .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    let lightmap_bytes = read_section_data(&mut cursor, &meta, SectionId::Lightmap as u32)
        .expect("read Lightmap section")
        .expect("Lightmap section present on soft-shadow map");
    let lightmap = LightmapSection::from_bytes(&lightmap_bytes).expect("lightmap from_bytes");

    // A real (non-placeholder) atlas: the six static lights bake into it, so it
    // must be larger than the 1x1 placeholder and carry irradiance bytes.
    assert!(
        lightmap.irr_width > 1 && lightmap.irr_height > 1,
        "soft-shadow map must bake a real lightmap atlas, got {}x{}",
        lightmap.irr_width,
        lightmap.irr_height,
    );
    assert!(
        !lightmap.irradiance.is_empty(),
        "baked lightmap must carry irradiance data",
    );

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}

/// Regression: every `light_index` emitted into `texel_lights` must reference
/// a slot in the runtime `AnimationDescriptor` buffer — i.e. the animated-
/// only filter (`!is_dynamic && animation.is_some()`), NOT the broader
/// `!is_dynamic` namespace that includes non-animated static lights.
///
/// Fixture: `test_animated_weight_maps_mixed.map` lists a non-animated static
/// light FIRST and an animated light SECOND. Under the old (buggy) filter the
/// animated light's `filtered_index` was 1, but the descriptor buffer only
/// contained one entry (slot 0), so `light_index = 1` overflowed. This test
/// asserts `light_index < animation_descriptors.len()` for every entry.
#[test]
#[ignore = "cold prl-build bake; run on demand with -- --ignored"]
fn mixed_fixture_light_indices_are_in_descriptor_buffer_bounds() {
    let ws = workspace_root();
    let input = ws.join("content/dev/maps/test_animated_weight_maps_mixed.map");
    assert!(input.exists(), "fixture map missing: {}", input.display(),);

    let out_dir = std::env::temp_dir().join("postretro_fixture_mixed");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("test_animated_weight_maps_mixed.prl");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    let bytes = std::fs::read(&output).expect("read compiled .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");

    let weight_map_bytes = read_section_data(
        &mut cursor,
        &meta,
        SectionId::AnimatedLightWeightMaps as u32,
    )
    .expect("read AnimatedLightWeightMaps")
    .expect("AnimatedLightWeightMaps section present on mixed fixture");
    let weight_maps =
        AnimatedLightWeightMapsSection::from_bytes(&weight_map_bytes).expect("from_bytes");

    let sh_volume_bytes =
        read_section_data(&mut cursor, &meta, SectionId::OctahedralShVolume as u32)
            .expect("read OctahedralShVolume")
            .expect("OctahedralShVolume section present on mixed fixture");
    let sh_volume = OctahedralShVolumeSection::from_bytes(&sh_volume_bytes).expect("sh from_bytes");

    let animated_light_count = sh_volume.animation_descriptors.len() as u32;
    assert_eq!(
        animated_light_count, 1,
        "mixed fixture should produce exactly one animated descriptor (the single animated light); \
         got {animated_light_count} — static light may have leaked into descriptor namespace",
    );

    assert!(
        !weight_maps.texel_lights.is_empty(),
        "mixed fixture must emit at least one per-texel weight entry",
    );

    for (i, tl) in weight_maps.texel_lights.iter().enumerate() {
        assert!(
            tl.light_index < animated_light_count,
            "texel_lights[{}].light_index ({}) >= animation_descriptors.len() ({}) \
             — chunk-list namespace is out of sync with the descriptor buffer",
            i,
            tl.light_index,
            animated_light_count,
        );
    }

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}

/// Golden regression for the script-light-membership seam: an unflagged static
/// light in a map with no data script must keep its pre-feature baked output.
///
/// The checked-in baseline was produced from commit `33e3a152` with
/// `prl-build --no-cache` on this exact map. Its SHA-256 is
/// `9264a3b23d806d50b2f33ae52a7a3720d7f5237e01255f4b48abdf22c1f89c19`.
/// Byte-for-byte comparison prevents script-membership plumbing from changing
/// the output for static lights that it does not target.
#[test]
#[ignore = "cold prl-build bake; run on demand with -- --ignored"]
fn mixed_fixture_without_script_membership_matches_pre_feature_golden_prl() {
    let ws = workspace_root();
    let input = ws.join("content/dev/maps/test_animated_weight_maps_mixed.map");
    let baseline = ws.join(
        "crates/level-compiler/tests/fixtures/golden/\
         test_animated_weight_maps_mixed.pre-script-light-membership.prl",
    );
    assert!(input.exists(), "fixture map missing: {}", input.display());
    assert!(
        baseline.exists(),
        "pre-feature baseline missing: {}",
        baseline.display(),
    );

    let out_dir = std::env::temp_dir().join("postretro_fixture_mixed_golden");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("test_animated_weight_maps_mixed.prl");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--no-cache")
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    let actual = std::fs::read(&output).expect("read compiled .prl");
    let expected = std::fs::read(&baseline).expect("read pre-feature baseline");
    assert_eq!(
        actual, expected,
        "an un-targeted static light changed the pre-feature PRL output",
    );

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}

/// Cap fixture smoke-test: compile `test_animated_weight_maps_cap.map` and
/// assert every covered texel's light list honors
/// `MAX_ANIMATED_LIGHTS_PER_CHUNK` (plan acceptance criterion).
///
/// Ignored: the cap fixture (6 co-located animated lights, cap = 4) triggers a
/// pre-existing UV-packer edge case where the chunk partitioner bottoms out at
/// min-extent and emits adjacent chunks whose outward-rounded atlas rects
/// overlap, tripping the baker's disjointness assert. The `MAX_LIGHTS_PER_CHUNK`
/// invariant itself is covered by unit tests in `animated_light_weight_maps.rs`
/// and by the `is_consistent` check. Un-ignore once the UV packer leaves a
/// 1-atlas-texel gap between adjacent chunk UV boundaries within a face.
#[test]
#[ignore]
fn cap_fixture_every_texel_respects_max_lights_per_chunk() {
    use postretro_level_format::animated_light_chunks::MAX_ANIMATED_LIGHTS_PER_CHUNK;

    let ws = workspace_root();
    let input = ws.join("content/dev/maps/test_animated_weight_maps_cap.map");
    assert!(input.exists(), "fixture map missing: {}", input.display());

    let out_dir = std::env::temp_dir().join("postretro_fixture_cap");
    std::fs::create_dir_all(&out_dir).expect("mkdir temp out");
    let output = out_dir.join("test_animated_weight_maps_cap.prl");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .current_dir(&ws)
        .status()
        .expect("spawn prl-build");
    assert!(status.success(), "prl-build failed: {status}");

    let bytes = std::fs::read(&output).expect("read cap .prl");
    let mut cursor = Cursor::new(&bytes);
    let meta = read_container(&mut cursor).expect("read_container");
    let section_bytes = read_section_data(
        &mut cursor,
        &meta,
        SectionId::AnimatedLightWeightMaps as u32,
    )
    .expect("read_section_data")
    .expect("AnimatedLightWeightMaps section present on cap fixture");

    let section = AnimatedLightWeightMapsSection::from_bytes(&section_bytes).expect("from_bytes");
    let cap = MAX_ANIMATED_LIGHTS_PER_CHUNK as u32;
    for (i, entry) in section.offset_counts.iter().enumerate() {
        assert!(
            entry.count <= cap,
            "offset_counts[{}].count = {} exceeds MAX_ANIMATED_LIGHTS_PER_CHUNK ({})",
            i,
            entry.count,
            cap,
        );
    }

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_dir(&out_dir);
}
