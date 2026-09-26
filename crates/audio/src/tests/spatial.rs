// Positional playback through the spatial chokepoint: admission against kira's
// live occupancy, anchors, panning and attenuation, own-pawn treatment, and
// level-unload fades.
// See: context/lib/audio.md §5

use super::*;

/// The committed fixture tone lasts 0.25 s: 2000 frames at the capturing
/// backend's 8 kHz. Windows are sized against it.
const TONE_FRAMES: usize = 2_000;

fn positioned(anchor: SoundAnchor) -> SoundRequest {
    SoundRequest {
        anchor: Some(anchor),
        ..sfx_request()
    }
}

fn at(point: [f32; 3]) -> SoundRequest {
    positioned(SoundAnchor::Point(point))
}

fn listener_at(position: [f32; 3], forward: [f32; 3]) -> ListenerState {
    ListenerState {
        position,
        forward,
        up: [0.0, 1.0, 0.0],
        attached: None,
    }
}

fn sfx_sub_tracks<B: kira::backend::Backend>(audio: &Audio<B>) -> usize {
    audio.buses.track(BusId::Sfx).num_sub_tracks()
}

/// Play one positioned tone into a fresh capturing mixer and return the RMS of
/// its first 1024 frames per channel. `setup` runs before the request.
fn channel_rms_of(
    listener: ListenerState,
    request: SoundRequest,
    setup: impl FnOnce(&mut Audio<CapturingBackend>),
) -> (f32, f32) {
    let mut audio = capturing_audio();
    audio.update(listener, 1.0 / 60.0, |_| None);
    setup(&mut audio);
    audio.play(request).expect("positioned tone is admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.manager.backend_mut().capture_channel_rms(1024)
}

fn total(rms: (f32, f32)) -> f32 {
    rms.0 + rms.1
}

#[test]
fn positioned_request_plays_on_a_spatial_track_under_sfx_and_mutes_with_it() {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.play(at([0.0, 0.0, -3.0])).expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(
        sfx_sub_tracks(&audio),
        1,
        "the voice owns a child track of SFX"
    );

    let loud = audio.manager.backend_mut().capture_rms(400);
    assert!(loud > 0.001, "the positioned tone is audible (rms {loud})");

    // Muting SFX mid-sound silences it: the spatial track sits under SFX.
    audio.set_bus_volume(BusId::Sfx, -80.0);
    audio.manager.backend_mut().capture_rms(200); // let the 10 ms fade settle
    let muted = audio.manager.backend_mut().capture_rms(400);
    assert!(
        muted < loud * 0.01,
        "muting SFX silences a playing positional sound (muted {muted}, loud {loud})",
    );
}

#[test]
fn positional_cap_admits_n_voices_and_refuses_the_next() {
    let mut audio = mock_audio();
    let cap = BusId::Sfx.voice_cap();
    for i in 0..cap {
        assert!(
            audio.play(at([i as f32, 0.0, -5.0])).is_some(),
            "positional voice {i} within the cap is admitted",
        );
    }
    assert!(
        audio.play(at([0.0, 0.0, -5.0])).is_none(),
        "the voice past the cap is refused, not queued",
    );
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    assert_eq!(audio.active_voices(BusId::Sfx), cap);
    assert_eq!(sfx_sub_tracks(&audio), cap, "every admitted voice started");
}

#[test]
fn finished_positional_voice_frees_its_slot_and_its_track_outlives_the_sound() {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    audio.update(listener, 1.0 / 60.0, |_| None);
    let handle = audio.play(at([0.0, 0.0, -3.0])).expect("admitted");
    audio.update(listener, 1.0 / 60.0, |_| None);

    // Mid-sound: an audio step reclaims nothing and the track stays put.
    audio.manager.backend_mut().capture_rms(TONE_FRAMES / 4);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        1,
        "a playing voice holds its slot"
    );
    assert!(
        audio.spatial.probe(handle).is_some(),
        "a playing voice stays live"
    );
    assert_eq!(
        sfx_sub_tracks(&audio),
        1,
        "the track is not dropped mid-sound"
    );

    // Past the end: the next audio step frees the slot.
    audio.manager.backend_mut().capture_rms(TONE_FRAMES * 2);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "the finished voice frees its slot"
    );

    // kira removes the dropped track on its own thread within two chunks.
    audio.manager.backend_mut().capture_rms(64);
    audio.manager.backend_mut().capture_rms(64);
    assert_eq!(sfx_sub_tracks(&audio), 0, "kira eventually frees the track");
}

// Pin P1: the engine reclaims before kira frees; a full new cap must still fit.
#[test]
fn full_cap_after_reclaim_plays_before_the_mixer_frees_old_tracks() {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    let cap = BusId::Sfx.voice_cap();
    audio.update(listener, 1.0 / 60.0, |_| None);
    for _ in 0..cap {
        audio.play(at([0.0, 0.0, -3.0])).expect("admitted");
    }
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.manager.backend_mut().capture_rms(TONE_FRAMES * 2);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "the engine reclaimed every voice"
    );
    assert_eq!(
        sfx_sub_tracks(&audio),
        cap,
        "kira still holds every reclaimed track until its thread runs again",
    );

    let handles: Vec<_> = (0..cap)
        .map(|i| {
            audio
                .play(at([0.0, 0.0, -3.0]))
                .unwrap_or_else(|| panic!("request {i} of a fresh cap is admitted"))
        })
        .collect();
    audio.update(listener, 1.0 / 60.0, |_| None);
    for handle in handles {
        assert!(
            audio.spatial.probe(handle).is_some(),
            "the mixer refused none of the admitted voices",
        );
    }
    assert_eq!(audio.active_voices(BusId::Sfx), cap);
}

// Pin P8: a voice that finishes mid-frame frees its slot at the audio step.
#[test]
fn voice_finishing_mid_frame_frees_its_slot_only_at_the_audio_step() {
    let mut audio = mock_audio();
    let cap = BusId::Sfx.voice_cap();
    for _ in 0..cap {
        audio.play(at([0.0, 0.0, -3.0])).expect("admitted");
    }
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    advance_playback(&mut audio, 8); // every voice finishes

    assert!(
        audio.play(at([0.0, 0.0, -3.0])).is_none(),
        "before the audio step the finished voices still hold the cap",
    );
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    assert!(
        audio.play(at([0.0, 0.0, -3.0])).is_some(),
        "after the audio step a slot is free",
    );
}

#[test]
fn source_on_the_right_is_louder_right_and_turning_around_swaps_it() {
    let right = [5.0, 0.0, 0.0];
    let (l, r) = channel_rms_of(listener_at([0.0; 3], [0.0, 0.0, -1.0]), at(right), |_| {});
    assert!(
        r > l * 1.5,
        "a source to the right is louder right (l {l}, r {r})"
    );

    let (l, r) = channel_rms_of(listener_at([0.0; 3], [0.0, 0.0, 1.0]), at(right), |_| {});
    assert!(
        l > r * 1.5,
        "facing the other way swaps the louder ear (l {l}, r {r})"
    );
}

#[test]
fn a_source_twice_as_far_beyond_the_minimum_distance_is_quieter() {
    let listener = forward_listener();
    let near = total(channel_rms_of(listener, at([0.0, 0.0, -8.0]), |_| {}));
    let far = total(channel_rms_of(listener, at([0.0, 0.0, -16.0]), |_| {}));
    assert!(
        far < near * 0.9,
        "twice as far is quieter (near {near}, far {far})"
    );
}

#[test]
fn entity_anchor_tracks_its_pose_then_holds_its_last_position_and_completes() {
    let mut audio = capturing_audio();
    let listener = forward_listener();
    let request = positioned(SoundAnchor::Entity {
        key: 7,
        point: [0.0, 0.0, -1.0],
    });
    let handle = audio.play(request).expect("admitted");

    audio.update(listener, 1.0 / 60.0, |key| {
        (key == 7).then_some([1.0, 0.0, -2.0])
    });
    let probe = audio.spatial.probe(handle).expect("started");
    assert_eq!(
        probe.position,
        [1.0, 0.0, -2.0],
        "starts at the entity's pose"
    );
    assert!(probe.tracking);

    audio.update(listener, 1.0 / 60.0, |key| {
        (key == 7).then_some([2.0, 0.0, -3.0])
    });
    assert_eq!(
        audio.spatial.probe(handle).unwrap().position,
        [2.0, 0.0, -3.0]
    );

    // Despawned: hold the last position, and never pick the key back up.
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.update(listener, 1.0 / 60.0, |_| Some([9.0, 9.0, 9.0]));
    let probe = audio.spatial.probe(handle).expect("still playing");
    assert_eq!(probe.position, [2.0, 0.0, -3.0], "holds its last position");
    assert!(!probe.tracking);

    audio.manager.backend_mut().capture_rms(TONE_FRAMES * 2);
    audio.update(listener, 1.0 / 60.0, |_| None);
    assert_eq!(audio.active_voices(BusId::Sfx), 0, "the sound completes");
}

// Pin P6: the entity is gone before audio first moves the sound.
#[test]
fn entity_removed_before_the_first_audio_step_plays_at_its_fire_time_point() {
    let mut audio = mock_audio();
    let handle = audio
        .play(positioned(SoundAnchor::Entity {
            key: 3,
            point: [4.0, 0.0, -4.0],
        }))
        .expect("admitted");
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    let probe = audio
        .spatial
        .probe(handle)
        .expect("started despite the missing entity");
    assert_eq!(probe.position, [4.0, 0.0, -4.0]);
    assert!(!probe.tracking);
}

#[test]
fn own_pawn_sound_plays_unpositioned_and_another_pawn_plays_spatial() {
    let mut audio = mock_audio();
    let attached = ListenerState {
        attached: Some(1),
        ..forward_listener()
    };
    audio.update(attached, 1.0 / 60.0, |_| None);

    let own = audio
        .play(positioned(SoundAnchor::Entity {
            key: 1,
            point: [0.0; 3],
        }))
        .expect("own pawn plays");
    let other = audio
        .play(positioned(SoundAnchor::Entity {
            key: 2,
            point: [0.0, 0.0, -5.0],
        }))
        .expect("other pawn plays");
    assert!(!audio.spatial.is_pending(own), "own pawn is not positional");
    assert!(
        audio.spatial.is_pending(other),
        "another pawn is positional"
    );

    audio.update(attached, 1.0 / 60.0, |_| Some([0.0; 3]));
    assert_eq!(
        sfx_sub_tracks(&audio),
        1,
        "only the other pawn's sound has a track"
    );
    assert_eq!(audio.active_voices(BusId::Sfx), 2);
}

// Pin P12: the listener changes pawn while both sounds play.
#[test]
fn sounds_keep_their_treatment_when_the_listener_changes_pawn() {
    let mut audio = mock_audio();
    let on = |key| ListenerState {
        attached: Some(key),
        ..forward_listener()
    };
    audio.update(on(1), 1.0 / 60.0, |_| None);
    let own = audio
        .play(positioned(SoundAnchor::Entity {
            key: 1,
            point: [0.0; 3],
        }))
        .expect("plays");
    let other = audio
        .play(positioned(SoundAnchor::Entity {
            key: 2,
            point: [0.0, 0.0, -5.0],
        }))
        .expect("plays");
    audio.update(on(1), 1.0 / 60.0, |_| Some([0.0; 3]));

    audio.update(on(2), 1.0 / 60.0, |_| Some([0.0; 3]));
    assert!(
        audio.spatial.probe(own).is_none(),
        "own-pawn sound stays unpositioned"
    );
    assert!(
        audio.spatial.probe(other).is_some(),
        "spatial sound stays spatial"
    );
    assert_eq!(sfx_sub_tracks(&audio), 1);
}

// Pin P4: a contact set resolves against the listener of the frame it starts.
#[test]
fn contact_set_starts_at_the_contact_nearest_that_frames_listener() {
    let mut audio = mock_audio();
    let near_origin = [1.0, 0.0, 0.0];
    let near_far_side = [20.0, 0.0, 0.0];
    let handle = audio
        .play(positioned(SoundAnchor::Contacts(vec![
            near_origin,
            near_far_side,
        ])))
        .expect("admitted");
    // The listener moved to the far side between the request and the step.
    audio.update(
        listener_at([19.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        1.0 / 60.0,
        |_| None,
    );
    assert_eq!(audio.spatial.probe(handle).unwrap().position, near_far_side);
}

#[test]
fn empty_contact_set_plays_nothing_and_releases_its_slot() {
    let mut audio = mock_audio();
    audio
        .play(positioned(SoundAnchor::Contacts(Vec::new())))
        .expect("admitted");
    audio.update(forward_listener(), 1.0 / 60.0, |_| None);
    assert_eq!(audio.active_voices(BusId::Sfx), 0);
    assert_eq!(sfx_sub_tracks(&audio), 0);
}

#[test]
fn attenuation_changes_gain_at_a_fixed_distance() {
    let listener = forward_listener();
    let request = || at([0.0, 0.0, -30.0]);
    let seeded = total(channel_rms_of(listener, request(), |_| {}));
    let tighter = total(channel_rms_of(listener, request(), |audio| {
        audio.set_attenuation(Attenuation {
            min_distance: 2.0,
            max_distance: 31.0,
            curve: AttenuationCurve::Linear,
        });
    }));
    assert!(
        tighter < seeded * 0.5,
        "a shorter max distance is quieter at 30 m (seeded {seeded}, tighter {tighter})",
    );
}

// Pin P9: attenuation applies when a sound starts; a live sound keeps its own.
#[test]
fn attenuation_change_applies_to_later_sounds_and_live_sounds_keep_theirs() {
    let listener = forward_listener();
    let tight = Attenuation {
        min_distance: 2.0,
        max_distance: 31.0,
        curve: AttenuationCurve::Linear,
    };
    let seeded = total(channel_rms_of(listener, at([0.0, 0.0, -30.0]), |_| {}));

    let mut audio = capturing_audio();
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.play(at([0.0, 0.0, -30.0])).expect("admitted");
    audio.set_attenuation(tight);
    audio.update(listener, 1.0 / 60.0, |_| None);
    let kept = total(audio.manager.backend_mut().capture_channel_rms(1024));
    assert!(
        (kept - seeded).abs() < seeded * 0.05,
        "a sound admitted before the change keeps its attenuation (seeded {seeded}, kept {kept})",
    );
}

#[test]
fn invalid_attenuation_falls_back_to_the_default() {
    let mut audio = mock_audio();
    audio.set_attenuation(Attenuation {
        min_distance: 10.0,
        max_distance: 5.0,
        curve: AttenuationCurve::Linear,
    });
    assert_eq!(audio.attenuation, Attenuation::DEFAULT);
}

// Pin P2: nothing anchored survives its world.
#[test]
fn fade_out_positional_stops_every_anchored_voice_and_spares_the_rest() {
    let mut audio = capturing_audio();
    let listener = ListenerState {
        attached: Some(1),
        ..forward_listener()
    };
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio.play(at([0.0, 0.0, -3.0])).expect("live positional");
    audio.update(listener, 1.0 / 60.0, |_| None);
    audio
        .play(at([0.0, 0.0, -4.0]))
        .expect("pending positional");
    audio
        .play(positioned(SoundAnchor::Entity {
            key: 1,
            point: [0.0; 3],
        }))
        .expect("own pawn");
    audio
        .play(SoundRequest {
            bus: "ui".to_string(),
            ..sfx_request()
        })
        .expect("unanchored ui sound");

    audio.fade_out_positional();
    assert_eq!(
        audio.active_voices(BusId::Sfx),
        0,
        "every anchored voice is released"
    );
    assert_eq!(
        audio.active_voices(BusId::UI),
        1,
        "unanchored sounds are untouched"
    );

    // Nothing is left to follow an entity into the next level.
    let mut resolved = 0;
    audio.update(listener, 1.0 / 60.0, |_| {
        resolved += 1;
        Some([0.0; 3])
    });
    assert_eq!(
        resolved, 0,
        "no voice asks where an entity is after the fade"
    );
}

#[test]
fn positioned_request_on_a_non_sfx_bus_or_looping_is_dropped() {
    let mut audio = mock_audio();
    let ui = SoundRequest {
        bus: "ui".to_string(),
        ..at([0.0; 3])
    };
    assert!(audio.play(ui).is_none());
    let looping = SoundRequest {
        looping: true,
        ..at([0.0; 3])
    };
    assert!(audio.play(looping).is_none());
    for bus in BusId::ALL {
        assert_eq!(audio.active_voices(bus), 0);
    }
}
