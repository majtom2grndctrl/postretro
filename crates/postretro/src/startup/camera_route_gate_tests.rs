//! Grep gate for camera-route playback and recording: they build only with
//! `dev-tools`, the engine never generates a route, and no player setting
//! names them. The walk override, its tallies and its levers stay in test
//! code by the spike's gates in `drain_cap_tests`
//! (`walk_drain_cap_override_lives_only_in_the_test_walk_harness`,
//! `walk_tallies_and_their_seams_are_test_only`,
//! `in_hand_cap_override_seam_is_test_only`). This file uses
//! `drain_cap_tests`'s grep helpers, which therefore must stay `#[cfg(test)]`.
//! See: context/lib/development_guide.md §6.4 · context/lib/testing_guide.md §3

use std::path::{Path, PathBuf};

use crate::session::level_streaming::drain_cap_tests::{
    DEV_TOOLS, TEST, TEST_OR_DEV_TOOLS, TEST_WITH_DEV_TOOLS, player_facing_offenders, strip_gated,
    ungated_mentions, workspace_root,
};

/// Spellings that reach the binary's playback module or its renderer seams.
const PLAYBACK_ITEMS: &[&str] = &[
    "crate::playback",
    "playback::",
    ".playback",
    "Playback::",
    "RouteView",
    "RouteRun",
    "Recorder",
    "PlaybackDrainCounters",
    "playback_drain_counters",
    "set_frame_mark",
    "enable_playback_gpu_timing",
    "take_playback_gpu_samples",
    "finish_playback_gpu_timing",
    "PlaybackGpu",
];

/// The renderer's frame-mark and playback-ring seams.
const RENDERER_PLAYBACK_ITEMS: &[&str] = &[
    "set_frame_mark",
    "frame_mark::",
    "FrameMark",
    "playback_ring",
    "playback_timing",
    "PlaybackTiming",
    "enable_playback",
    "playback_post_submit",
    "PlaybackDrainCounters",
    "playback_drain_counters",
    "PlaybackGpu",
];

/// The shared route generator's entry points and internals.
const GENERATOR_ITEMS: &[&str] = &[
    "exact_route",
    "ExactRoute",
    "smoothed_route",
    "SmoothedRoute",
    "Smoothing",
    "walk_path",
    "camera_adjacency",
    "camera_graph",
    "Walk::build",
    "eye_point",
    "SplitMix64",
];

fn engine_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// Playback and recording build only with dev-tools: the module is declared
// behind the feature, and every other production file names its items only
// inside dev-tools-gated items. The ungated flag module, which only detects
// the flags and warns, reaches playback inside a gated block alone.
#[test]
fn camera_route_playback_and_recording_build_only_with_dev_tools() {
    let engine = engine_src();
    let main = read(&engine.join("main.rs"));
    assert!(main.contains("mod playback;"));
    assert!(!strip_gated("main.rs", &main, &[DEV_TOOLS]).contains("mod playback;"));

    let gates = [DEV_TOOLS, TEST, TEST_OR_DEV_TOOLS, TEST_WITH_DEV_TOOLS];
    let ungated = ungated_mentions(&engine, &gates, PLAYBACK_ITEMS);
    assert!(ungated.is_empty(), "{ungated:#?}");

    let flags = read(&engine.join("startup/camera_route_flags.rs"));
    assert!(
        flags.contains("self.playback"),
        "the scan reaches the flag module's gated present choice"
    );
    let production = strip_gated("startup/camera_route_flags.rs", &flags, &gates);
    let named: Vec<&&str> = PLAYBACK_ITEMS
        .iter()
        .filter(|item| production.contains(**item))
        .collect();
    assert!(named.is_empty(), "camera_route_flags.rs: {named:?}");
}

// In the renderer, the frame-mark state, the playback timestamp ring and the
// drain counters build only with dev-tools. `open_frame_mark` exists in every
// build; without the feature its body pushes no debug group and reports no
// mark opened, so `close_frame_mark` pops nothing.
#[test]
fn renderer_frame_marks_and_playback_ring_build_only_with_dev_tools() {
    let renderer = workspace_root().join("crates/renderer/src");
    let render_mod = read(&renderer.join("render/mod.rs"));
    let uploads = read(&renderer.join("render/uploads/mod.rs"));
    for (label, source, declarations) in [
        (
            "render/mod.rs",
            &render_mod,
            &["mod renderer_playback;"][..],
        ),
        (
            "render/uploads/mod.rs",
            &uploads,
            &[
                "mod frame_mark;",
                "mod playback_ring;",
                "mod playback_timing;",
            ][..],
        ),
    ] {
        let production = strip_gated(label, source, &[DEV_TOOLS]);
        for declaration in declarations {
            assert!(source.contains(declaration), "{label}: {declaration}");
            assert!(
                !production.contains(declaration),
                "{label}: `{declaration}` builds without dev-tools"
            );
        }
    }

    let ungated = ungated_mentions(&renderer, &[DEV_TOOLS], RENDERER_PLAYBACK_ITEMS);
    assert!(ungated.is_empty(), "{ungated:#?}");

    assert!(
        uploads.contains("encoder.push_debug_group(label);"),
        "with dev-tools, open_frame_mark pushes the frame's debug group"
    );
    let production = strip_gated("render/uploads/mod.rs", &uploads, &[DEV_TOOLS]);
    let start = production
        .find("fn open_frame_mark(")
        .expect("open_frame_mark exists in every build");
    let end = start
        + production[start..]
            .find("fn close_frame_mark(")
            .expect("close_frame_mark follows it");
    let body = &production[start..end];
    assert!(
        !body.contains("push_debug_group") && !body.contains("self.frame_mark"),
        "open_frame_mark marks nothing without dev-tools:\n{body}"
    );
    assert!(
        body.contains("false"),
        "open_frame_mark reports no mark opened without dev-tools:\n{body}"
    );
}

// The shared route generator is called by `postretro-tool` and the walk
// harness, which is test code; the engine's non-test code, in any feature
// set, reads pose files and the bake identity but never generates a route.
#[test]
fn only_the_tool_and_the_walk_harness_call_the_route_generator() {
    let engine = engine_src();
    let in_engine = ungated_mentions(&engine, &[TEST, TEST_WITH_DEV_TOOLS], GENERATOR_ITEMS);
    assert!(in_engine.is_empty(), "{in_engine:#?}");

    let tool = workspace_root().join("crates/tool/src/camera_route/mod.rs");
    let tool = strip_gated("tool camera_route/mod.rs", &read(&tool), &[TEST]);
    for call in ["exact_route_seeded(", "smoothed_route_seeded("] {
        assert!(tool.contains(call), "the tool's generate verb calls {call}");
    }

    // `walk_drain_cap_override_lives_only_in_the_test_walk_harness` holds
    // the harness module itself behind `#[cfg(test)]`.
    let harness = read(&engine.join("session/lightmap_residency/walk_measurement/mod.rs"));
    for call in ["ExactRoute::of_walk(", "Walk::build("] {
        assert!(harness.contains(call), "the walk harness calls {call}");
    }
}

// No player setting or options UI names camera-route playback, recording or
// the pose file. Animation playback rates and speeds in the SDK typedefs
// (`playbackRate`) are not camera playback. The drain cap's own gate is
// `no_player_setting_names_the_drain_cap`.
#[test]
fn no_player_setting_names_camera_route_playback_or_recording() {
    let needles = [
        "camera_route",
        "camera-route",
        "cameraroute",
        "camera route",
        "playback",
        "pose file",
        "pose_file",
        "posefile",
        "pose-file",
        "record_camera",
        "record-camera",
        "recordcamera",
    ];
    let offenders = player_facing_offenders(
        &needles,
        &["playbackrate", "playback rate", "playback speed"],
    );
    assert!(offenders.is_empty(), "{offenders:#?}");
}
