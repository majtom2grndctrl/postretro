// Playback's launch surface: the record header names the drain cap in force
// at route frame 0 and the launch present mode, `--present-mode` reaches the
// renderer without a route, and a cap-slider move marks every later row.
// See: context/lib/rendering_pipeline.md §12 · context/lib/development_guide.md §6.4

use std::time::Instant;

use postretro_renderer::PresentChoice;

use crate::playback::Playback;
use crate::playback::lifecycle::tests::{frame, playing, record};
use crate::startup::lifecycle::tests::test_app;

fn launch(list: &[&str]) -> Vec<String> {
    std::iter::once("postretro")
        .chain(list.iter().copied())
        .map(str::to_string)
        .collect()
}

#[test]
fn playback_record_header_names_the_seeded_cap_at_route_frame_0() {
    // AC 22: the seed set at launch is the cap route frame 0 admits against.
    let mut run = playing(1, 3, &[]);
    let level = &mut run.app.session.as_mut().unwrap().level_streaming;
    level.apply_play_drain_cap_seed(Ok("4".to_string()));
    let now = Instant::now();
    for _ in 0..4 {
        frame(&mut run.app, now, true, true);
    }
    let header = &record(&run)["header"];
    assert_eq!(header["cap"]["source"], "seed");
    assert_eq!(header["cap"]["decoded_bytes_per_drain"], 4 * 1024 * 1024);
}

#[test]
fn playback_record_header_names_the_seeded_cap_when_the_hold_aborts() {
    // The cap is recorded from the first row, hold rows included.
    let mut run = playing(5, 3, &[]);
    let level = &mut run.app.session.as_mut().unwrap().level_streaming;
    level.apply_play_drain_cap_seed(Ok("4".to_string()));
    frame(&mut run.app, Instant::now(), true, true);
    assert!(
        run.app
            .playback_abort(Some("level unload requested".to_string()))
    );
    let header = &record(&run)["header"];
    assert_eq!(header["outcome"], "aborted");
    assert_eq!(header["cap"]["source"], "seed");
    assert_eq!(header["cap"]["decoded_bytes_per_drain"], 4 * 1024 * 1024);
}

#[test]
fn playback_record_header_names_the_production_cap_without_a_seed() {
    let mut run = playing(0, 2, &[]);
    let now = Instant::now();
    frame(&mut run.app, now, true, true);
    frame(&mut run.app, now, true, true);
    let header = &record(&run)["header"];
    assert_eq!(header["cap"]["source"], "production");
    assert_eq!(
        header["cap"]["decoded_bytes_per_drain"],
        crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN
    );
}

#[test]
fn playback_record_header_names_the_launch_present_mode() {
    // AC 23: the header's present mode is the launch choice.
    for (extra, expected) in [
        (&[][..], "vsync"),
        (&["--present-mode", "no-vsync"][..], "no-vsync"),
    ] {
        let mut run = playing(0, 1, extra);
        frame(&mut run.app, Instant::now(), true, true);
        assert_eq!(
            record(&run)["header"]["present_mode"],
            expected,
            "{extra:?}"
        );
    }
}

#[test]
fn present_mode_reaches_the_renderer_with_or_without_a_route() {
    let without_route = Playback::from_args(
        &launch(&["--present-mode=no-vsync", "map.prl"]),
        Instant::now(),
    )
    .unwrap();
    assert!(without_route.active.is_none());
    assert_eq!(without_route.present_choice(), PresentChoice::NoVsync);

    let mut app = test_app();
    assert_eq!(app.launch_present_choice(), PresentChoice::Vsync);
    app.playback = without_route;
    assert_eq!(app.launch_present_choice(), PresentChoice::NoVsync);

    let error = Playback::from_args(&launch(&["--present-mode", "adaptive"]), Instant::now())
        .err()
        .expect("an unknown mode stops the launch");
    assert!(error.contains("--present-mode"), "{error}");
}

#[test]
fn renderer_builds_its_surface_with_the_launch_present_mode() {
    // The choice goes to renderer creation, never to a toggle after it.
    let main = include_str!("../main.rs");
    assert!(main.contains("Renderer::new(&window, self.launch_present_choice())"));
    let init = include_str!("../../../renderer/src/render/renderer_init.rs");
    assert!(init.contains("present_mode: present.wgpu_present_mode(),"));
}

#[test]
fn a_cap_slider_move_marks_rows_where_the_frame_loop_applies_it() {
    // The slider's write-back is where its change lands.
    let frame_loop = include_str!("../frame_loop/mod.rs");
    let apply = frame_loop
        .find("session.level_streaming.apply_drain_cap_lever(&drain_cap);")
        .expect("the frame loop applies a moved slider");
    let mark = frame_loop[apply..]
        .find("crate::playback::ChangeKind::DrainCap")
        .expect("and marks the playback rows");
    assert!(mark < 200, "the mark sits beside the apply");
    let guard = frame_loop[..apply]
        .rfind("if drain_cap != drain_cap_before {")
        .unwrap();
    assert!(apply - guard < 120, "both only when the slider moved");
}
