// Positional-voice chokepoint: the only code that creates kira spatial tracks or
// updates their parameters. Owns each positional voice's anchor and position;
// the track carries the attenuation it started with. Later directional cues
// extend this module.
// See: context/lib/audio.md §5

use std::collections::HashMap;
use std::time::Duration;

use kira::listener::ListenerId;
use kira::sound::PlaybackState;
use kira::track::{SpatialTrackBuilder, SpatialTrackHandle, TrackHandle};
use kira::{Easing, Tween};

use crate::boundary::{Attenuation, AttenuationCurve, SoundAnchor};
use crate::playable::Playable;
use crate::voices::{KiraVoice, SoundHandle};

/// Two-ear panning weight. kira's default: the far ear keeps a quarter of the
/// signal, so a hard-left source is still faintly present on the right.
const SPATIALIZATION_STRENGTH: f32 = 0.75;

/// Upper bound on a reposition tween. A frame hitch longer than this snaps the
/// source rather than sliding it across the gap.
const MAX_REPOSITION_TWEEN_SECONDS: f32 = 0.1;

/// A positional request admitted against the voice budget but not yet started.
/// Starting waits for the audio step so a contact set resolves against that
/// frame's listener, and an entity anchor takes that frame's pose.
struct PendingVoice {
    handle: SoundHandle,
    playable: Playable,
    anchor: SoundAnchor,
    attenuation: Attenuation,
}

/// A started positional voice. The track handle is held until the sound reports
/// `Stopped`: dropping it earlier would let kira remove the track mid-sound.
struct LiveVoice {
    sound: KiraVoice,
    track: SpatialTrackHandle,
    /// Entity key still being followed; `None` once frozen or for a fixed point.
    tracking: Option<u64>,
    position: [f32; 3],
}

/// Every positional voice, pending and live, keyed by the shared `SoundHandle`
/// id space.
#[derive(Default)]
pub(crate) struct SpatialVoices {
    pending: Vec<PendingVoice>,
    live: HashMap<SoundHandle, LiveVoice>,
}

impl SpatialVoices {
    /// Whether kira has a sub-track slot under `sfx` for one more positional
    /// voice. Admitted-but-unstarted voices count, and kira keeps a removed
    /// track's slot until its own thread frees it, so this can refuse a request
    /// the engine's voice counter would accept.
    pub(crate) fn has_room(&self, sfx: &TrackHandle) -> bool {
        sfx.num_sub_tracks() + self.pending.len() < sfx.sub_track_capacity()
    }

    /// Queue an admitted request to start at the next audio step.
    pub(crate) fn admit(
        &mut self,
        handle: SoundHandle,
        playable: Playable,
        anchor: SoundAnchor,
        attenuation: Attenuation,
    ) {
        self.pending.push(PendingVoice {
            handle,
            playable,
            anchor,
            attenuation,
        });
    }

    /// Per-frame step: move live voices to their anchors' current positions,
    /// then start pending voices against this frame's listener. Returns how many
    /// admitted voices failed to start, so the caller releases their slots.
    pub(crate) fn update(
        &mut self,
        sfx: &mut TrackHandle,
        listener: ListenerId,
        listener_position: [f32; 3],
        dt: f32,
        resolve: &mut dyn FnMut(u64) -> Option<[f32; 3]>,
    ) -> usize {
        let tween = Tween {
            duration: Duration::from_secs_f32(dt.clamp(0.0, MAX_REPOSITION_TWEEN_SECONDS)),
            ..Tween::default()
        };
        for voice in self.live.values_mut() {
            let Some(key) = voice.tracking else {
                continue;
            };
            match resolve(key) {
                Some(position) => {
                    voice.position = position;
                    voice.track.set_position(position, tween);
                }
                // The entity is gone: hold the last position and play out.
                None => voice.tracking = None,
            }
        }

        let mut failed = 0;
        for pending in std::mem::take(&mut self.pending) {
            let Some((position, tracking)) =
                start_position(&pending.anchor, listener_position, resolve)
            else {
                failed += 1;
                continue;
            };
            match start_voice(
                sfx,
                listener,
                position,
                pending.attenuation,
                pending.playable,
            ) {
                Ok((sound, track)) => {
                    self.live.insert(
                        pending.handle,
                        LiveVoice {
                            sound,
                            track,
                            tracking,
                            position,
                        },
                    );
                }
                Err(err) => {
                    log::warn!("[Audio] kira rejected positional sound: {err}");
                    failed += 1;
                }
            }
        }
        failed
    }

    /// Drop every voice whose sound has finished, returning how many were
    /// reclaimed. Dropping the track handle then lets kira remove the track;
    /// its sound is already done, so no tail is cut.
    pub(crate) fn reclaim_finished(&mut self) -> usize {
        let before = self.live.len();
        self.live
            .retain(|_, voice| voice.sound.state() != PlaybackState::Stopped);
        before - self.live.len()
    }

    /// Stop one voice, live or pending. Returns whether `handle` was ours.
    pub(crate) fn stop(&mut self, handle: SoundHandle, tween: Tween) -> bool {
        if let Some(mut voice) = self.live.remove(&handle) {
            voice.sound.stop(tween);
            return true;
        }
        let before = self.pending.len();
        self.pending.retain(|pending| pending.handle != handle);
        before != self.pending.len()
    }

    /// Stop every voice, fading live ones over `tween`, and return how many
    /// voice slots the caller must release. A faded track persists in kira until
    /// its fade ends, so it never outlives the fade but never cuts it either.
    pub(crate) fn stop_all(&mut self, tween: Tween) -> usize {
        let count = self.live.len() + self.pending.len();
        for (_, mut voice) in self.live.drain() {
            voice.sound.stop(tween);
        }
        self.pending.clear();
        count
    }

    #[cfg(test)]
    pub(crate) fn probe(&self, handle: SoundHandle) -> Option<SpatialProbe> {
        self.live.get(&handle).map(|voice| SpatialProbe {
            position: voice.position,
            tracking: voice.tracking.is_some(),
        })
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self, handle: SoundHandle) -> bool {
        self.pending.iter().any(|pending| pending.handle == handle)
    }
}

/// Test view of a live positional voice.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpatialProbe {
    pub(crate) position: [f32; 3],
    pub(crate) tracking: bool,
}

/// Where a pending voice starts, and the entity key it keeps following. An
/// entity that no longer resolves starts frozen at its fire-time point. An
/// empty contact set has nowhere to play.
fn start_position(
    anchor: &SoundAnchor,
    listener: [f32; 3],
    resolve: &mut dyn FnMut(u64) -> Option<[f32; 3]>,
) -> Option<([f32; 3], Option<u64>)> {
    match anchor {
        SoundAnchor::Entity { key, point } => Some(match resolve(*key) {
            Some(position) => (position, Some(*key)),
            None => (*point, None),
        }),
        SoundAnchor::Point(point) => Some((*point, None)),
        SoundAnchor::Contacts(points) => points
            .iter()
            .copied()
            .min_by(|a, b| {
                distance_squared(*a, listener).total_cmp(&distance_squared(*b, listener))
            })
            .map(|point| (point, None)),
    }
}

fn start_voice(
    sfx: &mut TrackHandle,
    listener: ListenerId,
    position: [f32; 3],
    attenuation: Attenuation,
    playable: Playable,
) -> Result<(KiraVoice, SpatialTrackHandle), String> {
    // One sound per track: position belongs to the track, not the sound. The
    // track persists until its sound finishes, so a dropped handle never cuts a tail.
    let builder = SpatialTrackBuilder::new()
        .distances((attenuation.min_distance, attenuation.max_distance))
        .attenuation_function(Some(easing(attenuation.curve)))
        .spatialization_strength(SPATIALIZATION_STRENGTH)
        .persist_until_sounds_finish(true)
        .sound_capacity(1)
        .sub_track_capacity(0);
    let mut track = sfx
        .add_spatial_sub_track(listener, position, builder)
        .map_err(|err| err.to_string())?;
    let sound = match playable {
        Playable::Static(data) => track
            .play(data)
            .map(KiraVoice::Static)
            .map_err(|err| err.to_string())?,
        Playable::Streaming(data) => track
            .play(data)
            .map(KiraVoice::Streaming)
            .map_err(|err| err.to_string())?,
    };
    Ok((sound, track))
}

/// kira applies the curve to `1 - relative distance` and interpolates the
/// result in decibels, so `Linear` is a straight dB ramp and `Quadratic` falls
/// away faster at range.
fn easing(curve: AttenuationCurve) -> Easing {
    match curve {
        AttenuationCurve::Linear => Easing::Linear,
        AttenuationCurve::Quadratic => Easing::InPowi(2),
    }
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}
