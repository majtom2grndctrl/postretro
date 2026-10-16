// Arrival cues use captured points, independently of delayed pawn presentation.
// See: context/lib/audio.md §4 (Teleporters)

use glam::Vec3;
use postretro_audio::{SoundAnchor, SoundRequest};

pub(crate) fn arrival_sound(
    teleporters: &postretro_physics::teleporters::Teleporters,
    index: u16,
    arrival: Vec3,
) -> Option<SoundRequest> {
    let record = teleporters.records().get(usize::from(index))?;
    if record.arrival_sound.is_empty() {
        return None;
    }
    Some(SoundRequest {
        bus: "sfx".to_owned(),
        sound: record.arrival_sound.clone(),
        looping: false,
        anchor: Some(SoundAnchor::Point(arrival.to_array())),
    })
}

/// One cue per accepted remote generation change, at the sample's fixed arrival
/// point. Netcode already coalesced each pawn's changes to its last sample and
/// kept first samples silent; an entity anchor would resolve the delayed pose.
pub(crate) fn observed_arrival_sounds(
    teleporters: &postretro_physics::teleporters::Teleporters,
    observed: &[postretro_netcode::ObservedTeleport],
    sounds: &mut Vec<SoundRequest>,
) {
    sounds.extend(
        observed
            .iter()
            .filter_map(|cue| arrival_sound(teleporters, cue.last_teleporter, cue.arrival)),
    );
}
