// The Ghost bus in the mixer: its profile chain, its place under SFX, its own
// voice pool, and slot release for positional ghost voices.
// See: context/lib/audio.md §1 (Ghost bus)

use std::sync::Arc;
use std::time::Duration;

use kira::Frame;
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};

use super::spatial::{at, sfx_sub_tracks};
use super::*;

/// Filters wide open, unity volume, no drive: the bus adds nothing of its own,
/// so a test isolates what SFX does to it.
const OPEN: GhostBusProfile = GhostBusProfile {
    volume: 1.0,
    high_pass_hz: GhostBusProfile::MIN_CUTOFF_HZ,
    low_pass_hz: GhostBusProfile::MAX_CUTOFF_HZ,
    drive: 0.0,
};

/// A band-limited profile like a game's own (the brief's example cutoffs).
const DEV_LIKE: GhostBusProfile = GhostBusProfile {
    volume: 0.7,
    high_pass_hz: 250.0,
    low_pass_hz: 3000.0,
    drive: 0.0,
};

/// Synthesized tones render at this rate so a tone well above the low-pass
/// cutoff still sits below Nyquist.
const TONE_RATE: u32 = 48_000;

/// Long enough for every profile ramp (at least 10 ms) to settle.
const SETTLE_FRAMES: usize = 2_400;

/// Skips the filters' onset transient before measuring.
const ONSET_FRAMES: usize = 480;

/// The measured window: 0.1 s, many cycles of the lowest test tone.
const WINDOW_FRAMES: usize = 4_800;

fn ghost(request: SoundRequest) -> SoundRequest {
    SoundRequest {
        bus: "ghost".to_string(),
        ..request
    }
}

fn ghost_at(point: [f32; 3]) -> SoundRequest {
    ghost(at(point))
}

/// Register a 0.25 s sine at `hz` and `amplitude` under `key`.
fn register_tone(audio: &mut AudioCore<CapturingBackend>, key: &str, hz: f64, amplitude: f64) {
    let frames: Arc<[Frame]> = (0..TONE_RATE as usize / 4)
        .map(|n| {
            let s =
                amplitude * (n as f64 / f64::from(TONE_RATE) * hz * std::f64::consts::TAU).sin();
            Frame {
                left: s as f32,
                right: s as f32,
            }
        })
        .collect();
    audio.registry.insert_static_for_test(
        key,
        StaticSoundData {
            sample_rate: TONE_RATE,
            frames,
            settings: StaticSoundSettings::default(),
            slice: None,
        },
    );
}

fn unpositioned(bus: &str, sound: &str) -> SoundRequest {
    SoundRequest {
        bus: bus.to_string(),
        sound: sound.to_string(),
        looping: false,
        anchor: None,
    }
}

/// Play `sound` on `bus` into a settled mixer and return the RMS of a window
/// past its onset, then render past its end so the next play starts clean.
fn measure(audio: &mut AudioCore<CapturingBackend>, bus: &str, sound: &str) -> f32 {
    audio.play(unpositioned(bus, sound)).expect("tone plays");
    let backend = audio.manager.backend_mut();
    backend.capture_rms(ONSET_FRAMES);
    let rms = backend.capture_rms(WINDOW_FRAMES);
    backend.capture_rms(TONE_RATE as usize / 4);
    rms
}

/// A 48 kHz mixer with the ghost bus settled at `profile` and tones at
/// 60 Hz, 1 kHz and 10 kHz registered as `low`, `mid` and `high`.
fn tone_mixer(profile: GhostBusProfile) -> AudioCore<CapturingBackend> {
    let mut audio = capturing_audio_at(TONE_RATE);
    register_tone(&mut audio, "low", 60.0, 0.25);
    register_tone(&mut audio, "mid", 1_000.0, 0.25);
    register_tone(&mut audio, "high", 10_000.0, 0.25);
    audio.set_ghost_bus_profile(profile, Duration::ZERO);
    audio.manager.backend_mut().capture_rms(SETTLE_FRAMES);
    audio
}

/// Ghost-bus level relative to the same tone straight through SFX.
fn ghost_gain(audio: &mut AudioCore<CapturingBackend>, tone: &str) -> f32 {
    let dry = measure(audio, "sfx", tone);
    assert!(dry > 0.01, "the {tone} tone is audible on SFX ({dry})");
    measure(audio, "ghost", tone) / dry
}

#[test]
fn ghost_bus_is_silent_until_a_profile_applies() {
    let mut audio = capturing_audio_at(TONE_RATE);
    register_tone(&mut audio, "mid", 1_000.0, 0.25);
    audio.manager.backend_mut().capture_rms(SETTLE_FRAMES);
    assert_eq!(measure(&mut audio, "ghost", "mid"), 0.0);
}

// AC: a tone through the Ghost bus under a band-limited profile measures
// attenuated below the high-pass and above the low-pass, and changes after a
// profile set.
#[test]
fn ghost_bus_attenuates_below_the_high_pass_and_above_the_low_pass_and_follows_a_new_profile() {
    let mut audio = tone_mixer(DEV_LIKE);
    let low = ghost_gain(&mut audio, "low");
    let mid = ghost_gain(&mut audio, "mid");
    let high = ghost_gain(&mut audio, "high");
    assert!(mid > 0.25, "the passband carries the tone (gain {mid})");
    assert!(
        low < mid * 0.25,
        "60 Hz under a 250 Hz high-pass is cut (low {low}, mid {mid})"
    );
    assert!(
        high < mid * 0.25,
        "10 kHz over a 3 kHz low-pass is cut (high {high}, mid {mid})"
    );

    // Open the low-pass past the high tone: it now passes.
    audio.set_ghost_bus_profile(
        GhostBusProfile {
            low_pass_hz: 18_000.0,
            ..DEV_LIKE
        },
        Duration::ZERO,
    );
    audio.manager.backend_mut().capture_rms(SETTLE_FRAMES);
    let opened = ghost_gain(&mut audio, "high");
    assert!(
        opened > high * 4.0,
        "the new profile reaches the bus (high before {high}, after {opened})"
    );
}

// AC: the floor bus profile applies no distortion. Its drive is 0, and at drive
// 0 the bus is linear: a tenth of the input is exactly a tenth of the output.
#[test]
fn zero_drive_ghost_bus_is_linear_and_a_driven_bus_is_not() {
    let level_ratio = |drive: f32| {
        let mut audio = tone_mixer(GhostBusProfile { drive, ..OPEN });
        register_tone(&mut audio, "loud", 1_000.0, 0.9);
        register_tone(&mut audio, "quiet", 1_000.0, 0.09);
        measure(&mut audio, "ghost", "loud") / measure(&mut audio, "ghost", "quiet")
    };
    let undriven = level_ratio(0.0);
    assert!(
        (undriven - 10.0).abs() < 0.01,
        "drive 0 adds no distortion: output scales with input ({undriven})"
    );
    let driven = level_ratio(1.0);
    assert!(driven < 9.0, "full drive squashes the loud tone ({driven})");
}

#[test]
fn invalid_ghost_profile_keeps_the_current_mix() {
    let mut audio = tone_mixer(DEV_LIKE);
    let before = ghost_gain(&mut audio, "mid");
    audio.set_ghost_bus_profile(
        GhostBusProfile {
            high_pass_hz: 19_000.0,
            low_pass_hz: 18_000.0,
            ..DEV_LIKE
        },
        Duration::ZERO,
    );
    audio.manager.backend_mut().capture_rms(SETTLE_FRAMES);
    let after = ghost_gain(&mut audio, "mid");
    assert!(
        (after - before).abs() < before * 0.05,
        "crossed cutoffs are refused (before {before}, after {after})"
    );
}

// A profile change reversed mid-ramp turns back from the current value.
#[test]
fn ghost_volume_ramp_reversed_mid_way_turns_back_without_stepping() {
    let mut audio = tone_mixer(GhostBusProfile {
        volume: 0.0,
        ..OPEN
    });
    register_tone(&mut audio, "hum", 1_000.0, 0.25);
    // A long tone keeps sounding across the whole ramp.
    audio
        .play(SoundRequest {
            looping: true,
            ..unpositioned("ghost", "hum")
        })
        .expect("tone plays");
    audio.set_ghost_bus_profile(OPEN, Duration::from_millis(200));
    let backend = audio.manager.backend_mut();
    let before_reversal = backend.capture_rms(4_800); // 100 ms up the ramp
    assert!(before_reversal > 0.001, "the ramp is under way");

    audio.set_ghost_bus_profile(
        GhostBusProfile {
            volume: 0.0,
            ..OPEN
        },
        Duration::from_millis(200),
    );
    let after_reversal = audio.manager.backend_mut().capture_rms(480);
    assert!(
        after_reversal > before_reversal * 0.8,
        "the reversal starts from where the ramp was, not from silence \
         (before {before_reversal}, after {after_reversal})"
    );
    let later = audio.manager.backend_mut().capture_rms(4_800);
    assert!(
        later < after_reversal,
        "and then falls ({after_reversal} → {later})"
    );
}

/// Play a positional fixture tone on `bus` into a fresh 8 kHz mixer with the
/// ghost bus open, after `setup`, returning its RMS.
fn positional_rms(bus: &str, setup: impl FnOnce(&mut AudioCore<CapturingBackend>)) -> f32 {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.set_ghost_bus_profile(OPEN, Duration::ZERO);
    setup(&mut audio);
    audio.manager.backend_mut().capture_rms(800);
    audio
        .play(SoundRequest {
            bus: bus.to_string(),
            ..at([0.0, 0.0, -3.0])
        })
        .expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.manager.backend_mut().capture_rms(1_600)
}

// AC: ghost voices follow SFX volume and world muffle.
#[test]
fn ghost_voices_follow_sfx_volume_and_world_muffle() {
    let dry = positional_rms("ghost", |_| {});
    assert!(dry > 0.001, "a positional ghost voice is audible ({dry})");

    let muted = positional_rms("ghost", |audio| audio.set_bus_volume(BusId::Sfx, -80.0));
    assert!(
        muted < dry * 0.01,
        "SFX volume reaches the ghost bus (muted {muted}, dry {dry})"
    );

    // The 438 Hz fixture under the 300 Hz maximum muffle cutoff.
    let muffled = positional_rms("ghost", |audio| audio.set_world_muffle(1.0));
    assert!(
        muffled < dry * 0.7,
        "world muffle reaches the ghost bus (muffled {muffled}, dry {dry})"
    );
    assert!(muffled > 0.0, "muffled, not silenced");
}

fn ghost_sub_tracks<B: kira::backend::Backend>(audio: &AudioCore<B>) -> usize {
    audio.buses.track(BusId::Ghost).num_sub_tracks()
}

// AC: the seventeenth live ghost voice is refused while a real SFX sound
// requested in the same frame still plays.
#[test]
fn seventeenth_ghost_voice_is_refused_while_an_sfx_sound_in_the_same_frame_plays() {
    let mut audio = mock_audio();
    let cap = BusId::Ghost.voice_cap();
    assert_eq!(cap, 16);
    for i in 0..cap {
        assert!(
            audio.play(ghost_at([i as f32, 0.0, -5.0])).is_some(),
            "ghost voice {i} within the cap is admitted",
        );
    }
    assert!(
        audio.play(ghost_at([0.0, 0.0, -5.0])).is_none(),
        "the seventeenth ghost voice is refused",
    );
    let real = audio
        .play(at([0.0, 0.0, -5.0]))
        .expect("an SFX sound in the same frame is admitted");

    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    assert!(audio.spatial.probe(real).is_some(), "the SFX sound plays");
    assert_eq!(audio.active_voices(BusId::Ghost), cap);
    assert_eq!(audio.active_voices(BusId::Sfx), 1);
    assert_eq!(ghost_sub_tracks(&audio), cap, "ghost voices hang off Ghost");
    assert_eq!(
        sfx_sub_tracks(&audio),
        1,
        "only the SFX voice hangs off SFX"
    );
}

// AC: with the Ghost track present, SFX keeps headroom for two full caps of
// voices, and the ghost pool keeps its own.
#[test]
fn sfx_and_ghost_each_fit_a_fresh_full_cap_while_the_mixer_holds_the_last() {
    let mut audio = capturing_audio();
    audio.set_ghost_bus_profile(OPEN, Duration::ZERO);
    let listener = forward_listener();
    let sfx_cap = BusId::Sfx.voice_cap();
    let ghost_cap = BusId::Ghost.voice_cap();
    audio.update(listener, 1.0 / 60.0, |_| None);
    for _ in 0..sfx_cap {
        audio.play(at([0.0, 0.0, -3.0])).expect("sfx admitted");
    }
    for _ in 0..ghost_cap {
        audio
            .play(ghost_at([0.0, 0.0, -3.0]))
            .expect("ghost admitted");
    }
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.manager.backend_mut().capture_rms(4_000);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(audio.active_voices(BusId::Sfx), 0);
    assert_eq!(audio.active_voices(BusId::Ghost), 0);
    assert_eq!(
        audio.buses.track(BusId::Sfx).num_sub_tracks(),
        sfx_cap + 1,
        "kira still holds every reclaimed SFX track, beside the Ghost track",
    );
    assert_eq!(ghost_sub_tracks(&audio), ghost_cap);

    let handles: Vec<_> = (0..sfx_cap)
        .map(|_| at([0.0, 0.0, -3.0]))
        .chain((0..ghost_cap).map(|_| ghost_at([0.0, 0.0, -3.0])))
        .map(|request| audio.play(request).expect("a fresh cap is admitted"))
        .collect();
    audio.update(listener, 1.0 / 60.0, |_| None);
    for handle in handles {
        assert!(
            audio.spatial.probe(handle).is_some(),
            "the mixer refused none of the admitted voices",
        );
    }
    assert_eq!(audio.active_voices(BusId::Sfx), sfx_cap);
    assert_eq!(audio.active_voices(BusId::Ghost), ghost_cap);
}

/// A looping unpositioned SFX sound holds one SFX slot for the whole test, so
/// a ghost release landing on SFX by mistake would show.
fn hold_one_sfx_voice(audio: &mut AudioCore<MockBackend>) {
    audio
        .play(SoundRequest {
            looping: true,
            ..sfx_request()
        })
        .expect("held sfx voice");
}

// AC: ghost voices release their slot on finish, stop and level unload (and
// on a failed start), and only their own.
#[test]
fn ghost_voices_release_their_own_slot_on_finish_stop_unload_and_failed_start() {
    let listener = forward_listener();

    let mut audio = mock_audio();
    hold_one_sfx_voice(&mut audio);
    audio.play(ghost_at([0.0, 0.0, -3.0])).expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    advance_playback(&mut audio, 8);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(audio.active_voices(BusId::Ghost), 0, "finish releases");
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    let pending = audio.play(ghost_at([0.0, 0.0, -3.0])).expect("admitted");
    audio.stop(pending);
    let live = audio.play(ghost_at([0.0, 0.0, -3.0])).expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.stop(live);
    assert_eq!(audio.active_voices(BusId::Ghost), 0, "stop releases");
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    audio
        .play(ghost_at([f32::NAN, 0.0, 0.0]))
        .expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(
        audio.active_voices(BusId::Ghost),
        0,
        "a failed start releases"
    );
    assert_eq!(audio.active_voices(BusId::Sfx), 1);

    audio.play(ghost_at([0.0, 0.0, -3.0])).expect("live");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.play(ghost_at([0.0, 0.0, -4.0])).expect("pending");
    audio.fade_out_positional();
    assert_eq!(
        audio.active_voices(BusId::Ghost),
        0,
        "level unload releases"
    );
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        1,
        "the unanchored SFX sound outlives the unload",
    );
}

// Regression: a client demoted mid-level kept its ghost voices playing. The
// demotion release stops ghost voices, live and pending, on their own bus,
// and leaves every real voice playing.
#[test]
fn fading_out_ghost_voices_releases_only_the_ghost_bus() {
    let listener = forward_listener();
    let mut audio = mock_audio();
    hold_one_sfx_voice(&mut audio);
    let real = audio
        .play(at([1.0, 0.0, -3.0]))
        .expect("real voice admitted");
    audio.play(ghost_at([0.0, 0.0, -3.0])).expect("live");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.play(ghost_at([0.0, 0.0, -4.0])).expect("pending");
    assert_eq!(audio.active_voices(BusId::Ghost), 2);

    audio.fade_out_ghost_voices();
    assert_eq!(
        audio.active_voices(BusId::Ghost),
        0,
        "ghost voices released"
    );
    assert_eq!(audio.active_voices(BusId::Sfx), 2, "real voices kept");
    assert!(
        audio.spatial.probe(real).is_some(),
        "the real voice plays on"
    );
}

// Bus half: a ghost voice sits at its mapped point and plays
// out there after its member is gone (the resolver no longer answers for any
// entity), while the member's own entity-anchored voice freezes where it last
// was. The ghost voice holds its slot until it finishes.
#[test]
fn a_ghost_point_voice_plays_out_at_its_point_after_its_member_is_gone() {
    let listener = forward_listener();
    let mut audio = mock_audio();
    let member = audio
        .play(SoundRequest {
            anchor: Some(SoundAnchor::Entity {
                key: 9,
                point: [1.0, 0.0, -3.0],
            }),
            ..sfx_request()
        })
        .expect("real voice admitted");
    let ghost = audio
        .play(ghost_at([101.0, 0.0, -3.0]))
        .expect("ghost voice admitted");
    audio.update(listener, 1.0 / 60.0, |key| {
        (key == 9).then_some([1.0, 0.0, -3.0])
    });
    let started = audio.spatial.probe(ghost).expect("ghost voice live");
    assert!(!started.tracking, "a ghost voice follows no entity");

    for _ in 0..3 {
        audio.update(listener, 1.0 / 60.0, |_| None);
    }
    let ghost_now = audio.spatial.probe(ghost).expect("still playing out");
    assert_eq!(ghost_now.position, [101.0, 0.0, -3.0]);
    assert_eq!(audio.active_voices(BusId::Ghost), 1);
    assert!(
        audio.spatial.probe(member).is_some(),
        "the real voice is untouched"
    );
}
