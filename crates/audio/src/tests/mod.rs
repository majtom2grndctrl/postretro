// Device-free tests for the audio subsystem: mock and capturing kira backends
// driving play, stop, the voice budget, reclamation, and volume.
// See: context/lib/testing_guide.md

use super::*;
use crate::volume::linear_volume_to_decibels;
use kira::backend::mock::MockBackend;
use kira::backend::{Backend, Renderer};

mod spatial;

/// A test backend that captures kira's [`Renderer`] so a test can pull the
/// mixer's processed output samples into a readable buffer.
///
/// kira's own `MockBackend` discards rendered frames into a private `Vec`
/// with no accessor, so it cannot verify *output amplitude* — only that
/// control-plane commands don't panic. The `Renderer::process(&mut [f32],
/// channels)` API is public, though, and `start` hands the renderer to the
/// backend by value, so a custom backend can hold it and expose the samples.
/// This is the load-bearing readout for the volume→amplitude AC: it is the
/// only way to observe that lowering a bus's volume reduces the signal that
/// would reach the OS. `Audio<B>` being generic over the backend is what
/// lets a test construct `Audio<CapturingBackend>` and drive it device-free.
pub(super) struct CapturingBackend {
    renderer: Option<Renderer>,
}

impl Backend for CapturingBackend {
    type Settings = u32; // sample rate
    type Error = ();

    fn setup(sample_rate: u32, _internal_buffer_size: usize) -> Result<(Self, u32), ()> {
        // kira drives the renderer at `sample_rate`; the backend itself
        // doesn't need to retain it, so just echo it back to the manager.
        Ok((Self { renderer: None }, sample_rate))
    }

    fn start(&mut self, renderer: Renderer) -> Result<(), ()> {
        self.renderer = Some(renderer);
        Ok(())
    }
}

impl CapturingBackend {
    /// Drain queued control-plane commands (so freshly-played sounds become
    /// audible) and render `frame_count` stereo frames, returning the peak
    /// absolute sample amplitude across both channels. A return of `0.0`
    /// means total silence.
    pub(super) fn capture_peak(&mut self, frame_count: usize) -> f32 {
        let renderer = self.renderer.as_mut().expect("renderer started");
        // Flush play/volume commands into the mixer before rendering.
        renderer.on_start_processing();
        let mut out = vec![0.0_f32; frame_count * 2];
        renderer.process(&mut out, 2);
        out.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }

    /// Drain queued commands and render `frame_count` stereo frames,
    /// returning the RMS (root-mean-square) sample amplitude across both
    /// channels. RMS is the energy metric for the volume→amplitude check:
    /// unlike peak, it integrates over the whole window, so a bus volume
    /// tween that ramps in over its first few milliseconds still reads as a
    /// proportionally quieter signal rather than being masked by the brief
    /// pre-ramp transient at the very start of the clip.
    /// Drain queued commands and render `frame_count` stereo frames, returning
    /// the RMS of the left and right channels separately. The panning readout.
    pub(super) fn capture_channel_rms(&mut self, frame_count: usize) -> (f32, f32) {
        let renderer = self.renderer.as_mut().expect("renderer started");
        renderer.on_start_processing();
        let mut out = vec![0.0_f32; frame_count * 2];
        renderer.process(&mut out, 2);
        let rms = |channel: usize| {
            let sum_sq: f64 = out
                .iter()
                .skip(channel)
                .step_by(2)
                .map(|s| (*s as f64) * (*s as f64))
                .sum();
            ((sum_sq / frame_count as f64).sqrt()) as f32
        };
        (rms(0), rms(1))
    }

    pub(super) fn capture_rms(&mut self, frame_count: usize) -> f32 {
        let renderer = self.renderer.as_mut().expect("renderer started");
        renderer.on_start_processing();
        let mut out = vec![0.0_f32; frame_count * 2];
        renderer.process(&mut out, 2);
        let sum_sq: f64 = out.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        ((sum_sq / out.len() as f64).sqrt()) as f32
    }
}

/// Build an `Audio` on the capturing backend at the fixture's real 8 kHz
/// sample rate, with the engine's capacities, bus tree, and fixture sounds
/// loaded. The 8 kHz rate matches the `test_tone.wav` fixture so rendered
/// frames carry the tone at full resolution (kira's `MockBackend` defaults
/// to 1 Hz, which would resample the clip away to near-nothing).
pub(super) fn capturing_audio() -> Audio<CapturingBackend> {
    let mono = mono::MonoFoldHandle::default();
    let settings = AudioManagerSettings::<CapturingBackend> {
        capacities: Audio::CAPACITIES,
        main_track_builder: Audio::main_track_builder(&mono),
        backend_settings: 8_000,
        ..Default::default()
    };
    let mut manager =
        AudioManager::<CapturingBackend>::new(settings).expect("capturing backend starts");
    let listener = manager
        .add_listener([0.0_f32, 0.0, 0.0], Audio::IDENTITY_ORIENTATION)
        .expect("listener allocates");
    let buses = BusTree::build(&mut manager).expect("bus tree builds");

    let mut audio = Audio::from_parts(manager, listener, buses, mono);
    audio.load_level_sounds(&dev_content_root());
    audio
}

/// Absolute path to the repo's `content/dev`, so the committed sound fixtures
/// resolve regardless of where `cargo test` runs from. Mirrors the helper in
/// `assets.rs`.
pub(super) fn dev_content_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev")
        .canonicalize()
        .expect("content/dev exists relative to the crate manifest")
}

/// Build an `Audio` on the always-available mock backend with the engine's
/// real capacities and bus tree, plus the level's fixture sounds loaded.
/// Exercises the production code paths without a sound device. Returns the
/// `Audio` and its `MockBackend` manager pieces — kira's mock backend lives
/// inside the manager, reached via `manager.backend_mut()`.
pub(super) fn mock_audio() -> Audio<MockBackend> {
    let mono = mono::MonoFoldHandle::default();
    let settings = AudioManagerSettings::<MockBackend> {
        capacities: Audio::CAPACITIES,
        main_track_builder: Audio::main_track_builder(&mono),
        ..Default::default()
    };
    let mut manager =
        AudioManager::<MockBackend>::new(settings).expect("mock backend always starts");
    let listener = manager
        .add_listener([0.0_f32, 0.0, 0.0], Audio::IDENTITY_ORIENTATION)
        .expect("listener allocates under mock backend");
    let buses = BusTree::build(&mut manager).expect("bus tree builds under mock backend");

    let mut audio = Audio::from_parts(manager, listener, buses, mono);
    audio.load_level_sounds(&dev_content_root());
    audio
}

/// A one-shot SFX request for the committed static fixture.
pub(super) fn sfx_request() -> SoundRequest {
    SoundRequest {
        bus: "sfx".to_string(),
        sound: "sfx/test_tone".to_string(),
        looping: false,
        anchor: None,
    }
}

#[test]
fn init_under_mock_backend_builds_bus_tree_and_loads_fixtures() {
    // The fault-tolerant init contract: under a device-free backend the
    // manager starts, the Master → SFX/Music/UI tree is created, and the
    // level's fixture sounds register — all without a sound device. Proven
    // by constructing through the same path `Audio::new` uses (manager +
    // listener + bus tree + level load) and observing the resulting state.
    let audio = mock_audio();

    // Bus tree present: every bus starts at zero active voices.
    for bus in BusId::ALL {
        assert_eq!(
            audio.active_voices(bus),
            0,
            "bus {bus:?} exists and starts idle",
        );
    }
    // Fixtures loaded into the registry under their content-relative keys.
    assert!(
        audio.registry().contains("sfx/test_tone"),
        "static SFX fixture is registered after init",
    );
    assert!(
        audio.registry().contains("music/test_loop"),
        "streaming music fixture is registered after init",
    );
}

#[test]
fn play_produces_non_silent_output_under_capturing_backend() {
    // `play` must actually route a signal into the mixer: rendering the
    // mixer output after playing the tone yields non-silent frames. This is
    // the device-free analog of "sound reaches the OS".
    let mut audio = capturing_audio();
    let handle = audio.play(sfx_request());
    assert!(
        handle.is_some(),
        "fixture plays under the capturing backend"
    );

    // 4096 frames at 8 kHz is ~0.5s, covering the 0.25s tone fixture.
    let peak = audio.manager.backend_mut().capture_peak(4096);
    assert!(
        peak > 0.01,
        "playing the tone yields audible output (peak {peak} > 0.01)",
    );
}

#[test]
fn lowering_bus_volume_reduces_output_amplitude() {
    // Load-bearing AC: lowering a bus's volume measurably reduces the output
    // amplitude of sounds routed to it. Measure the RMS energy of the tone
    // played on SFX at unity gain, then attenuate the SFX bus, replay, and
    // measure again — the attenuated RMS must be measurably lower.
    //
    // The bus volume is set *before* playing and the mixer is warmed up with
    // a silent render window, so kira's default 10 ms volume tween fully
    // settles before any audio plays. That keeps the comparison about the
    // steady-state gain rather than the brief tween ramp.
    let frames = 4096;

    let mut loud = capturing_audio();
    // Warm-up render at unity gain establishes steady state with no sound.
    loud.manager.backend_mut().capture_rms(frames);
    loud.play(sfx_request()).expect("fixture plays");
    let loud_rms = loud.manager.backend_mut().capture_rms(frames);

    let mut quiet = capturing_audio();
    // -24 dB ≈ 0.063× linear: a large, unambiguous attenuation. Set before
    // play and let the tween settle during the silent warm-up window.
    quiet.set_bus_volume(BusId::Sfx, -24.0);
    quiet.manager.backend_mut().capture_rms(frames);
    quiet.play(sfx_request()).expect("fixture plays");
    let quiet_rms = quiet.manager.backend_mut().capture_rms(frames);

    assert!(
        loud_rms > 0.001,
        "baseline output carries audible energy (rms {loud_rms})",
    );
    // Measurably lower — well beyond float noise. The expected linear factor
    // is ~0.063, so the attenuated RMS should sit far below half the
    // baseline even allowing for envelope and resampling variation.
    assert!(
        quiet_rms < loud_rms * 0.5,
        "lowering SFX volume reduces output amplitude: quiet rms {quiet_rms} \
         should be well under half of loud rms {loud_rms}",
    );
}

#[test]
fn lowering_main_volume_reduces_output_amplitude() {
    // Mirror of the bus-volume AC for the main track: lowering the overall
    // output volume measurably reduces the amplitude of whatever plays. The
    // main track is the parent of every bus, so attenuating it scales the
    // SFX tone the same way a bus attenuation would. Set the volume before
    // playing and warm the mixer up with a silent render window so kira's
    // default 10 ms tween settles before any audio — the comparison stays
    // about steady-state gain, not the tween ramp.
    let frames = 4096;

    let mut loud = capturing_audio();
    loud.manager.backend_mut().capture_rms(frames);
    loud.play(sfx_request()).expect("fixture plays");
    let loud_rms = loud.manager.backend_mut().capture_rms(frames);

    let mut quiet = capturing_audio();
    // -24 dB ≈ 0.063× linear: a large, unambiguous attenuation.
    quiet.set_main_volume(-24.0);
    quiet.manager.backend_mut().capture_rms(frames);
    quiet.play(sfx_request()).expect("fixture plays");
    let quiet_rms = quiet.manager.backend_mut().capture_rms(frames);

    assert!(
        loud_rms > 0.001,
        "baseline output carries audible energy (rms {loud_rms})",
    );
    // Well below half the baseline even allowing for envelope and resampling
    // variation; expected linear factor is ~0.063.
    assert!(
        quiet_rms < loud_rms * 0.5,
        "lowering main volume reduces output amplitude: quiet rms {quiet_rms} \
         should be well under half of loud rms {loud_rms}",
    );
}

#[test]
fn play_drops_request_when_bus_is_at_its_voice_cap() {
    // Fill the SFX bus to its voice cap via `play`, then assert the next
    // `play` is dropped (`None`) and active voices never exceed the cap.
    let mut audio = mock_audio();
    let cap = BusId::Sfx.voice_cap();

    for i in 0..cap {
        assert!(
            audio.play(sfx_request()).is_some(),
            "play {i} within cap succeeds",
        );
    }
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        cap,
        "the bus fills exactly to its cap",
    );

    // One past the cap is dropped, and the count stays pinned at the cap.
    assert!(
        audio.play(sfx_request()).is_none(),
        "a play past the voice cap is dropped",
    );
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        cap,
        "active voices never exceed the cap",
    );
}

#[test]
fn shutdown_drops_cleanly_after_playing() {
    // `shutdown` consumes `Audio`, tearing down the manager (and its
    // listener/tracks/voices). Playing first, then shutting down, must not
    // panic — the device-free analog of a clean engine exit.
    let mut audio = mock_audio();
    audio.play(sfx_request()).expect("fixture plays");
    audio.shutdown();
}

#[test]
fn parse_bus_is_case_insensitive_and_rejects_unknown() {
    assert_eq!(parse_bus("sfx"), Some(BusId::Sfx));
    assert_eq!(parse_bus("Music"), Some(BusId::Music));
    assert_eq!(parse_bus("UI"), Some(BusId::UI));
    assert_eq!(parse_bus("master"), None);
    assert_eq!(parse_bus(""), None);
}

#[test]
fn orientation_from_identity_forward_is_identity() {
    // Camera looking down -Z with +Y up is kira's unrotated reference: the
    // resulting quaternion should be (near) identity.
    let q = orientation_from_forward_up([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    assert!((q[0]).abs() < 1e-5, "x ~ 0, got {}", q[0]);
    assert!((q[1]).abs() < 1e-5, "y ~ 0, got {}", q[1]);
    assert!((q[2]).abs() < 1e-5, "z ~ 0, got {}", q[2]);
    assert!((q[3].abs() - 1.0).abs() < 1e-5, "w ~ ±1, got {}", q[3]);
}

#[test]
fn orientation_is_finite_for_degenerate_inputs() {
    // Zero forward and forward-parallel-to-up must not produce NaNs.
    for q in [
        orientation_from_forward_up([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        orientation_from_forward_up([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
    ] {
        assert!(q.iter().all(|c| c.is_finite()), "orientation {q:?} finite");
    }
}

#[test]
fn play_returns_handle_and_holds_a_voice() {
    let mut audio = mock_audio();
    assert_eq!(audio.active_voices(BusId::Sfx), 0);

    let handle = audio.play(sfx_request());
    assert!(handle.is_some(), "loaded SFX fixture plays");
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        1,
        "playing a sound holds one SFX voice",
    );
}

#[test]
fn stop_releases_the_voice() {
    let mut audio = mock_audio();
    let handle = audio.play(sfx_request()).expect("fixture plays");
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    audio.stop(handle);
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "stop releases the held voice",
    );
}

#[test]
fn stop_on_unknown_handle_is_a_noop() {
    let mut audio = mock_audio();
    // An id never minted by this Audio resolves to nothing.
    audio.stop(SoundHandle::from_raw_for_test(9999));
    assert_eq!(audio.active_voices(BusId::Sfx), 0);
}

#[test]
fn unknown_bus_returns_none_without_holding_a_voice() {
    let mut audio = mock_audio();
    let handle = audio.play(SoundRequest {
        bus: "reverb".to_string(),
        sound: "sfx/test_tone".to_string(),
        looping: false,
        anchor: None,
    });
    assert!(handle.is_none(), "unknown bus is dropped");
    for bus in BusId::ALL {
        assert_eq!(audio.active_voices(bus), 0, "no voice acquired on any bus");
    }
}

#[test]
fn unknown_sound_returns_none_without_holding_a_voice() {
    let mut audio = mock_audio();
    let handle = audio.play(SoundRequest {
        bus: "sfx".to_string(),
        sound: "sfx/does_not_exist".to_string(),
        looping: false,
        anchor: None,
    });
    assert!(handle.is_none(), "unknown sound is dropped");
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "a missing sound never consumes a voice",
    );
}

#[test]
fn looping_request_holds_its_voice_across_a_sweep() {
    // A looping sound never reaches `Stopped`, so the finished-voice sweep in
    // `update` must leave it holding its voice. Advance playback well past the
    // 0.25s fixture, then sweep: the loop is still held.
    let mut audio = mock_audio();
    let handle = audio.play(SoundRequest {
        bus: "sfx".to_string(),
        sound: "sfx/test_tone".to_string(),
        looping: true,
        anchor: None,
    });
    assert!(handle.is_some(), "looping fixture plays");
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    advance_playback(&mut audio, 8);
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);

    assert_eq!(
        audio.active_voices(BusId::Sfx),
        1,
        "a looping sound keeps its voice across the sweep",
    );
}

#[test]
fn finished_sweep_reclaims_a_stopped_one_shot() {
    // A non-looping sound that has run to its end reports `Stopped`; the sweep
    // inside `update` must release its voice or the bus leaks capacity.
    let mut audio = mock_audio();
    let _handle = audio.play(sfx_request()).expect("fixture plays");
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    // Drive kira past the clip end. At the mock backend's 1 Hz sample rate,
    // each `process()` advances two seconds of audio time — a handful clears
    // the 0.25s fixture — and `on_start_processing` flushes the play command.
    advance_playback(&mut audio, 8);

    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "the sweep reclaims the finished one-shot's voice",
    );
}

/// A listener looking down -Z, world up — the kira reference pose.
pub(super) fn forward_listener() -> ListenerState {
    ListenerState {
        position: [0.0, 0.0, 0.0],
        forward: [0.0, 0.0, -1.0],
        up: [0.0, 1.0, 0.0],
        attached: None,
    }
}

/// Step the mock renderer so queued play commands take effect and playback
/// advances. `on_start_processing` drains command buffers; `process` advances
/// audio time. Interleaved `steps` times.
pub(super) fn advance_playback(audio: &mut Audio<MockBackend>, steps: usize) {
    for _ in 0..steps {
        audio.manager.backend_mut().on_start_processing();
        audio.manager.backend_mut().process();
    }
}

#[test]
fn bus_volume_zero_silences_the_bus_and_master_zero_silences_everything() {
    let frames = 4096;
    let mut audio = capturing_audio();
    audio.set_bus_volume(BusId::Sfx, linear_volume_to_decibels(0.0));
    audio.manager.backend_mut().capture_rms(frames);
    audio.play(sfx_request()).expect("fixture plays");
    assert_eq!(audio.manager.backend_mut().capture_peak(frames), 0.0);

    let mut master_off = capturing_audio();
    master_off.set_main_volume(linear_volume_to_decibels(0.0));
    master_off.manager.backend_mut().capture_rms(frames);
    master_off.play(sfx_request()).expect("fixture plays");
    assert_eq!(master_off.manager.backend_mut().capture_peak(frames), 0.0);
}
