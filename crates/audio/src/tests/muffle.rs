// The world muffle in the mixer: it filters the SFX bus, positional voices
// included, and leaves Music and UI untouched.
// See: context/lib/audio.md §1 (Mixer bus tree)

use super::*;

/// The fixture tone's length at the capturing backend's 8 kHz.
const TONE_FRAMES: usize = 2_000;

/// Longer than the muffle's ramp at 8 kHz, so the amount settles before the
/// tone starts.
const SETTLE_FRAMES: usize = 800;

fn on_bus(bus: &str) -> SoundRequest {
    SoundRequest {
        bus: bus.to_string(),
        ..sfx_request()
    }
}

fn positioned() -> SoundRequest {
    SoundRequest {
        anchor: Some(SoundAnchor::Point([0.0, 0.0, -3.0])),
        ..sfx_request()
    }
}

/// Play `request` into a fresh capturing mixer with the world muffle settled
/// at `amount`, returning the core and the tone's rendered samples.
fn render(request: SoundRequest, amount: f32) -> (AudioCore<CapturingBackend>, Vec<f32>) {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.set_world_muffle(amount);
    audio.manager.backend_mut().capture_samples(SETTLE_FRAMES);
    audio.play(request).expect("tone plays");
    audio.update(listener, 1.0 / 60.0, |_| None);
    let samples = audio.manager.backend_mut().capture_samples(TONE_FRAMES);
    (audio, samples)
}

fn rms(samples: &[f32]) -> f32 {
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[test]
fn world_muffle_filters_the_sfx_bus_and_leaves_music_and_ui_unfiltered() {
    let (_, dry) = render(on_bus("sfx"), 0.0);
    let (_, muffled) = render(on_bus("sfx"), 1.0);
    assert!(rms(&dry) > 0.01, "the tone is audible: {}", rms(&dry));
    // The 438 Hz tone under the 300 Hz maximum cutoff.
    assert!(
        rms(&muffled) < rms(&dry) * 0.7,
        "sfx muffled {} vs dry {}",
        rms(&muffled),
        rms(&dry)
    );
    assert!(rms(&muffled) > 0.0, "muffled, not silenced");

    for bus in ["music", "ui"] {
        let (_, dry) = render(on_bus(bus), 0.0);
        let (_, engaged) = render(on_bus(bus), 1.0);
        assert!(rms(&dry) > 0.01, "{bus}: the tone is audible");
        assert_eq!(engaged, dry, "{bus} is never filtered");
    }
}

#[test]
fn world_muffle_reaches_positional_voices_through_the_bus_with_no_per_voice_filter() {
    let (dry_audio, dry) = render(positioned(), 0.0);
    let (muffled_audio, muffled) = render(positioned(), 1.0);
    assert!(rms(&dry) > 0.01, "the positioned tone is audible");
    assert!(
        rms(&muffled) < rms(&dry) * 0.7,
        "positional muffled {} vs dry {}",
        rms(&muffled),
        rms(&dry)
    );
    // The filter is the bus's one effect: engaging it builds no track or
    // filter per voice, so the mixer's track tree is the same shape: the three
    // top-level buses, and under SFX only the Ghost track and the voice's own.
    for audio in [&dry_audio, &muffled_audio] {
        assert_eq!(audio.manager.num_sub_tracks(), BusId::TOP_LEVEL.len());
        assert_eq!(audio.buses.track(BusId::Sfx).num_sub_tracks(), 2);
    }
}
