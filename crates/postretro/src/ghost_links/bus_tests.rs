// Ghost bus resolution: setGhostAudio ramps, crossing cutoffs, commits, unload and presentation.
// See: context/lib/audio.md §1 (Ghost bus)

use std::time::Duration;

use log::Level;
use postretro_foundation::GhostAudioTargets;
use postretro_scripting_core::runtime::{ModAudioProfile, ModGhostAudio};
use postretro_test_log_capture::LogCapture;

use super::*;

/// A game's own profile: band-limited, distinct from the floor in every field.
const MANIFEST: ModGhostAudio = ModGhostAudio {
    volume: 0.5,
    high_pass_hz: 250.0,
    low_pass_hz: 3000.0,
    drive: 0.2,
};

fn step(
    volume: Option<f32>,
    high: Option<f32>,
    low: Option<f32>,
    period_ms: f32,
) -> GhostAudioTargets {
    GhostAudioTargets {
        volume,
        high_pass_hz: high,
        low_pass_hz: low,
        drive: None,
        period_ms,
    }
}

fn bus() -> GhostBus {
    let mut bus = GhostBus::default();
    bus.set_manifest(MANIFEST);
    bus
}

/// Queue `targets` and apply it at the tick starting at `time`.
fn set(bus: &mut GhostBus, targets: GhostAudioTargets, time: f64) {
    bus.queue(targets);
    bus.apply_pending(time);
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// AC29: a periodMs ramp presents its midpoint halfway through; unset fields
// keep the manifest's values.
#[test]
fn a_set_ramp_presents_its_midpoint_halfway_through() {
    let mut bus = bus();
    set(&mut bus, step(Some(0.9), None, Some(5000.0), 1500.0), 10.0);
    let mid = bus.resolve(10.75);
    assert!(close(mid.volume, 0.7), "{mid:?}");
    assert!(close(mid.low_pass_hz, 4000.0), "{mid:?}");
    assert_eq!((mid.high_pass_hz, mid.drive), (250.0, 0.2));
    let end = bus.resolve(12.0);
    assert_eq!((end.volume, end.low_pass_hz), (0.9, 5000.0));
}

// AC29: a second set mid-ramp starts from the unclamped value at its applying
// tick; periodMs 0 lands on that tick.
#[test]
fn a_second_set_mid_ramp_starts_from_the_value_then_and_period_zero_lands_at_once() {
    let mut bus = bus();
    set(&mut bus, step(Some(1.0), None, None, 1000.0), 0.0);
    set(&mut bus, step(Some(0.0), None, None, 1000.0), 0.5);
    let ramp = bus.script_fields().volume.expect("volume ramp");
    assert!(close(ramp.from, 0.75), "from the 0.5 s value: {ramp:?}");
    assert!(close(bus.resolve(1.0).volume, 0.375));

    set(&mut bus, step(None, None, Some(1000.0), 0.0), 2.0);
    assert_eq!(
        bus.resolve(2.0).low_pass_hz,
        1000.0,
        "periodMs 0 lands at once"
    );
}

// Two calls in one tick apply in call order: the later wins per field and
// still ramps from the value before the tick.
#[test]
fn two_sets_in_one_tick_apply_in_call_order_from_the_pre_tick_value() {
    let mut bus = bus();
    bus.queue(step(Some(0.1), None, None, 0.0));
    bus.queue(step(Some(0.8), None, None, 1000.0));
    bus.apply_pending(4.0);
    let ramp = bus.script_fields().volume.expect("volume ramp");
    assert_eq!((ramp.from, ramp.to), (0.5, 0.8));
}

// P29: targets that would cross the cutoffs, judged against the targets after
// the step (a ramp in flight by its target), warn and drop the whole call.
#[test]
fn a_set_whose_targets_cross_the_cutoffs_warns_and_drops() {
    let capture = LogCapture::start();
    let mut bus = bus();
    set(&mut bus, step(None, Some(2000.0), None, 2000.0), 0.0);
    // Mid ramp the high-pass is still below 2500 Hz, but its target is not.
    set(&mut bus, step(Some(0.9), None, Some(1500.0), 0.0), 0.5);
    capture.assert_logged_once(Level::Warn, CROSSING_STEP_WARNING);
    let fields = bus.script_fields();
    assert!(
        fields.low_pass_hz.is_none(),
        "the low-pass ramp never started"
    );
    assert!(
        fields.volume.is_none(),
        "the whole call drops, volume included"
    );
    assert_eq!(
        fields.high_pass_hz.unwrap().to,
        2000.0,
        "the earlier ramp stands"
    );

    // One call naming both cutoffs crossed drops the same way.
    set(&mut bus, step(None, Some(900.0), Some(800.0), 0.0), 1.0);
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains(CROSSING_STEP_WARNING))
            .count(),
        2
    );
    assert!(bus.script_fields().low_pass_hz.is_none());
}

// P36: a reload under a script-set cutoff that crosses the new manifest
// presents the floor cutoffs and warns once, however many frames resolve.
#[test]
fn crossed_resolved_cutoffs_present_the_floor_cutoffs_and_warn_once() {
    let capture = LogCapture::start();
    let mut bus = bus();
    set(&mut bus, step(None, Some(2500.0), None, 0.0), 0.0);
    assert_eq!(bus.resolve(0.0).high_pass_hz, 2500.0);
    bus.set_manifest(ModGhostAudio {
        low_pass_hz: 2000.0,
        ..MANIFEST
    });
    for frame in 0..5 {
        let resolved = bus.resolve(1.0 + f64::from(frame));
        assert_eq!(
            (resolved.high_pass_hz, resolved.low_pass_hz),
            (
                ModGhostAudio::FLOOR.high_pass_hz,
                ModGhostAudio::FLOOR.low_pass_hz
            ),
        );
        assert_eq!(
            (resolved.volume, resolved.drive),
            (0.5, 0.2),
            "only the cutoffs fall back"
        );
    }
    capture.assert_logged_once(Level::Warn, CROSSED_PROFILE_WARNING);

    // Parted again, the authored cutoffs return; crossing anew warns anew.
    bus.set_manifest(MANIFEST);
    assert_eq!(bus.resolve(6.0).low_pass_hz, 3000.0);
    bus.set_manifest(ModGhostAudio {
        low_pass_hz: 2000.0,
        ..MANIFEST
    });
    bus.resolve(7.0);
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains(CROSSED_PROFILE_WARNING))
            .count(),
        2
    );
}

// P17: a committed reload that changes `audio.ghost` re-resolves the bus;
// fields a script set keep their values and ramps.
#[test]
fn a_manifest_commit_re_resolves_unset_fields_and_script_fields_stand() {
    let mut bus = bus();
    set(&mut bus, step(Some(0.9), None, None, 1000.0), 0.0);
    bus.set_manifest(ModGhostAudio {
        volume: 0.1,
        low_pass_hz: 6000.0,
        ..MANIFEST
    });
    let resolved = bus.resolve(0.5);
    assert!(
        close(resolved.volume, 0.7),
        "the script ramp runs on: {resolved:?}"
    );
    assert_eq!(
        resolved.low_pass_hz, 6000.0,
        "an unset field follows the reload"
    );
}

// P13: unload and demotion drop the script fields and queued calls; the bus
// returns to the manifest profile.
#[test]
fn dropping_script_fields_returns_the_bus_to_the_manifest_profile() {
    let mut bus = bus();
    set(&mut bus, step(Some(0.9), Some(500.0), None, 0.0), 0.0);
    bus.queue(step(Some(0.2), None, None, 0.0));
    bus.drop_script_fields();
    bus.apply_pending(1.0);
    assert_eq!(bus.resolve(1.0), ghost_audio_values(MANIFEST));
    assert!(bus.script_fields().is_empty());
}

// The bus hears only changes: steady state hands it nothing, a ramp in flight
// re-targets it each frame over that frame, and a commit forces a hand-off.
#[test]
fn presentation_sends_only_changes_and_ramps_each_frame_toward_the_script_value() {
    let frame = Duration::from_millis(16);
    let mut bus = bus();
    let (first, _) = bus.present(0.0, frame).expect("first presentation");
    assert_eq!(first, audio_ghost_profile(ghost_audio_values(MANIFEST)));
    assert!(first.is_valid());
    assert!(
        bus.present(0.1, frame).is_none(),
        "steady state sends nothing"
    );

    set(&mut bus, step(Some(1.0), None, None, 1000.0), 1.0);
    let (mid, ramp) = bus
        .present(1.5, frame)
        .expect("a ramp in flight moves the bus");
    assert!(close(mid.volume, 0.75));
    assert_eq!(ramp, frame);
    let (_, hitch) = bus
        .present(1.6, Duration::from_secs(2))
        .expect("still ramping");
    assert!(
        hitch <= Duration::from_millis(100),
        "a hitch snaps, never slides"
    );
    bus.present(3.0, frame);
    assert!(bus.present(3.5, frame).is_none(), "settled at its target");

    bus.set_manifest(MANIFEST);
    assert!(bus.present(3.6, frame).is_some(), "a commit re-presents");
}

// AC36 (host half), P13: after `setGhostAudio`, a level unload returns the bus
// to the manifest profile. P17: a commit changing `audio.ghost` re-resolves it
// with the script values standing. Driven through the App's own seams.
#[test]
fn app_unload_returns_the_bus_to_the_manifest_and_a_commit_keeps_script_fields() {
    let mut app = crate::startup::lifecycle::tests::test_app();
    app.apply_mod_audio_profile(ModAudioProfile {
        ghost: MANIFEST,
        ..ModAudioProfile::default()
    });
    app.queue_ghost_audio(step(Some(0.9), None, None, 0.0));
    app.apply_pending_ghost_audio_commands();
    assert_eq!(app.ghost_bus.resolve(0.0).volume, 0.9);

    app.apply_mod_audio_profile(ModAudioProfile {
        ghost: ModGhostAudio {
            low_pass_hz: 6000.0,
            ..MANIFEST
        },
        ..ModAudioProfile::default()
    });
    let reloaded = app.ghost_bus.resolve(0.0);
    assert_eq!((reloaded.volume, reloaded.low_pass_hz), (0.9, 6000.0));

    app.unload_level();
    assert!(app.ghost_bus.script_fields().is_empty());
    assert_eq!(app.ghost_bus.resolve(0.0).volume, MANIFEST.volume);
}

/// Run `source` as a level script in one runtime, the way level load does
/// (TypeScript bundled through `scripts-build`), returning its reactions.
fn level_reactions(source: &str, luau: bool) -> Vec<postretro_entities::NamedReaction> {
    use postretro_level_format::data_script::DataScriptSection;
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};

    let dir = tempfile::tempdir().expect("script dir");
    let entry = dir
        .path()
        .join(if luau { "level.luau" } else { "level.ts" });
    std::fs::write(&entry, source).expect("level script writes");
    let bundled = if luau {
        source.to_owned()
    } else {
        postretro_script_compiler::bundle_entry(&entry).expect("level script bundles")
    };
    let runtime = ScriptRuntime::new(
        &PrimitiveRegistry::new(),
        &ScriptRuntimeConfig::default(),
        &postretro_entities::ScriptCtx::new(),
    )
    .expect("script runtime constructs");
    runtime
        .run_data_script(
            &DataScriptSection {
                compiled_bytes: bundled.into_bytes(),
                source_path: entry.to_string_lossy().into_owned(),
            },
            dir.path(),
        )
        .reactions
}

// AC43: the brief's swell resolves `setGhostAudio` from `postretro/audio` in
// both runtimes, each lowering it to the same target-less reaction body, which
// the reaction parser accepts as the call's targets.
#[test]
fn set_ghost_audio_resolves_from_postretro_audio_in_both_runtimes_alike() {
    let ts = level_reactions(
        r#"
        import { defineReaction } from "postretro";
        import { setGhostAudio } from "postretro/audio";
        export function setupLevel() {
          const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));
          return { reactions: [swell] };
        }
        "#,
        false,
    );
    let luau = level_reactions(
        r#"
        local Postretro = require("postretro")
        local Audio = require("postretro/audio")
        function setupLevel()
          local swell = Postretro.defineReaction("echo.swell", Audio.setGhostAudio({ volume = 0.9, lowPassHz = 5000, periodMs = 1500 }))
          return { reactions = { swell } }
        end
        "#,
        true,
    );
    assert_eq!(ts, luau, "both runtimes lower the swell alike");
    let [swell] = ts.as_slice() else {
        panic!("one reaction: {ts:?}");
    };
    let postretro_entities::ReactionDescriptor::Primitive(body) = &swell.descriptor else {
        panic!("a primitive body: {swell:?}");
    };
    assert_eq!(body.primitive, postretro_foundation::SET_GHOST_AUDIO);
    assert!(
        body.tag.is_none() && body.kind.is_none(),
        "it names no target"
    );
    assert_eq!(
        postretro_sim::ghost_audio_command::parse_set_ghost_audio(&body.args),
        Ok(step(Some(0.9), None, Some(5000.0), 1500.0)),
    );
}
