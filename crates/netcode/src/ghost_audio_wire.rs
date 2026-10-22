// Engine <-> wire conversion for the level-scoped ghost-audio record: the host's `setGhostAudio` fields.
// See: context/lib/networking.md §Current contract

use postretro_foundation::{GhostAudioOverride, GhostScalarRamp};
use postretro_net::wire::{
    GHOST_AUDIO_DRIVE, GHOST_AUDIO_HIGH_PASS, GHOST_AUDIO_LOW_PASS, GHOST_AUDIO_VOLUME,
    WireGhostAudioState, WireGhostScalarRamp,
};

/// The host's always-on record: only the fields a script has set ride, each
/// with its ramp as the bus holds it (the unclamped from-value at the applying
/// host tick, the host script time it starts at). The manifest profile and any
/// floor-cutoff fallback never reach the record.
#[must_use]
pub fn ghost_audio_to_wire(fields: &GhostAudioOverride) -> WireGhostAudioState {
    let mut mask = 0;
    let mut slot = |ramp: Option<GhostScalarRamp>, bit| {
        ramp.map(|ramp| {
            mask |= bit;
            WireGhostScalarRamp {
                from: ramp.from,
                to: ramp.to,
                start_time: ramp.start_time,
                duration_ms: ramp.duration_ms,
            }
        })
    };
    let volume = slot(fields.volume, GHOST_AUDIO_VOLUME);
    let high_pass_hz = slot(fields.high_pass_hz, GHOST_AUDIO_HIGH_PASS);
    let low_pass_hz = slot(fields.low_pass_hz, GHOST_AUDIO_LOW_PASS);
    let drive = slot(fields.drive, GHOST_AUDIO_DRIVE);
    WireGhostAudioState {
        fields: mask,
        volume,
        high_pass_hz,
        low_pass_hz,
        drive,
    }
}

/// The script fields a validated record carries, still on the host's script
/// clock: the receiving client rebases each start time before it presents
/// ([`WireGhostAudioState::validate`] has rejected a mask/slot mismatch, so
/// the slots alone say which fields are set).
#[must_use]
pub fn ghost_audio_from_wire(state: &WireGhostAudioState) -> GhostAudioOverride {
    let field = |ramp: Option<WireGhostScalarRamp>| {
        ramp.map(|ramp| GhostScalarRamp {
            from: ramp.from,
            to: ramp.to,
            start_time: ramp.start_time,
            duration_ms: ramp.duration_ms,
        })
    };
    GhostAudioOverride {
        volume: field(state.volume),
        high_pass_hz: field(state.high_pass_hz),
        low_pass_hz: field(state.low_pass_hz),
        drive: field(state.drive),
    }
}

#[cfg(test)]
mod tests {
    use postretro_foundation::{GhostAudioTargets, GhostAudioValues};
    use postretro_net::wire::{GHOST_AUDIO_FIELD_BITS, RawSnapshotMessage};

    use super::*;

    const MANIFEST: GhostAudioValues = GhostAudioValues {
        volume: 0.6,
        high_pass_hz: 200.0,
        low_pass_hz: 4000.0,
        drive: 0.0,
    };

    fn set(fields: &mut GhostAudioOverride, targets: GhostAudioTargets, start: f64) {
        let from = fields.resolve(MANIFEST, start);
        fields.set(from, &targets, start);
    }

    // AC4 (audio half, round trip): every field a script set encodes, rides a
    // snapshot envelope through the codec and validation, and converts back to
    // the same ramps, start times on the host clock included.
    #[test]
    fn ghost_audio_fields_round_trip_encode_decode_apply_with_every_field() {
        let mut fields = GhostAudioOverride::default();
        set(
            &mut fields,
            GhostAudioTargets {
                volume: Some(0.9),
                high_pass_hz: Some(250.0),
                low_pass_hz: Some(5000.0),
                drive: Some(0.3),
                period_ms: 1500.0,
            },
            12.75,
        );
        let wire = ghost_audio_to_wire(&fields);
        assert_eq!(wire.fields, GHOST_AUDIO_FIELD_BITS);
        let raw = RawSnapshotMessage {
            version: postretro_net::wire::SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 2,
            records: Vec::new(),
            state_schema_fingerprint: [0; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
            ghost_audio: Some(wire),
            server_script_time: 13.0,
        };
        let bytes = postretro_net::wire::encode(&raw);
        let decoded: RawSnapshotMessage = postretro_net::wire::decode(&bytes).unwrap();
        let typed = decoded.validate().expect("the envelope validates");
        assert_eq!(ghost_audio_from_wire(&typed.ghost_audio.unwrap()), fields);
    }

    // Fields no script has set stay local: a volume-only set rides as volume
    // alone, and an untouched bus carries an empty mask.
    #[test]
    fn ghost_audio_record_carries_only_the_fields_a_script_set() {
        assert_eq!(
            ghost_audio_to_wire(&GhostAudioOverride::default()),
            WireGhostAudioState::default()
        );
        let mut fields = GhostAudioOverride::default();
        set(
            &mut fields,
            GhostAudioTargets {
                volume: Some(0.9),
                high_pass_hz: None,
                low_pass_hz: None,
                drive: None,
                period_ms: 0.0,
            },
            2.0,
        );
        let wire = ghost_audio_to_wire(&fields);
        assert_eq!(wire.validate(), Ok(()));
        assert_eq!(wire.fields, GHOST_AUDIO_VOLUME);
        assert!(wire.high_pass_hz.is_none() && wire.low_pass_hz.is_none() && wire.drive.is_none());
        let volume = wire.volume.unwrap();
        assert_eq!((volume.from, volume.to, volume.start_time), (0.6, 0.9, 2.0));
        assert_eq!(ghost_audio_from_wire(&wire), fields);
    }
}
