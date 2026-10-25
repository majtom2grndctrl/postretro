// Positional-voice chokepoint: the only code that creates kira spatial tracks or
// updates their parameters. Owns each positional voice's bus, anchor and position;
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
use crate::buses::{BusId, BusTree};
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
    /// The bus the voice holds its slot on and parents its track to.
    bus: BusId,
    playable: Playable,
    anchor: SoundAnchor,
    attenuation: Attenuation,
}

/// A started positional voice. The track handle is held until the sound reports
/// `Stopped`: dropping it earlier would let kira remove the track mid-sound.
struct LiveVoice {
    sound: KiraVoice,
    track: SpatialTrackHandle,
    /// The bus whose slot the voice holds, released when it ends.
    bus: BusId,
    /// Entity key still being followed; `None` once frozen or for a fixed point.
    tracking: Option<u64>,
    position: [f32; 3],
}

/// Every positional voice, pending and live, keyed by the shared `SoundHandle`
/// id space. Each voice carries its bus and releases that bus's slot when it
/// finishes, stops, fails to start or is faded out with its world.
#[derive(Default)]
pub(crate) struct SpatialVoices {
    pending: Vec<PendingVoice>,
    live: HashMap<SoundHandle, LiveVoice>,
}

impl SpatialVoices {
    /// Whether kira has a sub-track slot under `bus`'s `track` for one more
    /// positional voice. Admitted-but-unstarted voices on that bus count, and
    /// kira keeps a removed track's slot until its own thread frees it, so this
    /// can refuse a request the engine's voice counter would accept. SFX's own
    /// count includes the Ghost track, which its capacity reserves a slot for.
    pub(crate) fn has_room(&self, bus: BusId, track: &TrackHandle) -> bool {
        let pending = self
            .pending
            .iter()
            .filter(|pending| pending.bus == bus)
            .count();
        track.num_sub_tracks() + pending < track.sub_track_capacity()
    }

    /// Queue an admitted request to start at the next audio step.
    pub(crate) fn admit(
        &mut self,
        handle: SoundHandle,
        bus: BusId,
        playable: Playable,
        anchor: SoundAnchor,
        attenuation: Attenuation,
    ) {
        self.pending.push(PendingVoice {
            handle,
            bus,
            playable,
            anchor,
            attenuation,
        });
    }

    /// Per-frame step: move live voices to their anchors' current positions,
    /// then start pending voices against this frame's listener, each on a child
    /// track of its own bus. An admitted voice that fails to start releases its
    /// slot on that bus.
    pub(crate) fn update(
        &mut self,
        buses: &mut BusTree,
        listener: ListenerId,
        listener_position: [f32; 3],
        dt: f32,
        resolve: &mut dyn FnMut(u64) -> Option<[f32; 3]>,
    ) {
        let tween = Tween {
            duration: Duration::from_secs_f32(if dt.is_finite() {
                dt.clamp(0.0, MAX_REPOSITION_TWEEN_SECONDS)
            } else {
                0.0
            }),
            ..Tween::default()
        };
        for voice in self.live.values_mut() {
            let Some(key) = voice.tracking else {
                continue;
            };
            match resolve(key) {
                Some(position) if is_finite_point(position) => {
                    voice.position = position;
                    voice.track.set_position(position, tween);
                }
                // The entity is gone, or its pose went non-finite: hold the
                // last finite position and play out.
                _ => voice.tracking = None,
            }
        }

        for pending in self.pending.drain(..) {
            let Some((position, tracking)) =
                start_position(&pending.anchor, listener_position, resolve)
            else {
                // An empty contact set is an ordinary miss; any other anchor
                // with no finite point is an upstream bug worth a warning.
                if !matches!(&pending.anchor, SoundAnchor::Contacts(points) if points.is_empty()) {
                    log::warn!("[Audio] positional sound has no finite position — dropped");
                }
                buses.release_voice(pending.bus);
                continue;
            };
            match start_voice(
                buses.track_mut(pending.bus),
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
                            bus: pending.bus,
                            tracking,
                            position,
                        },
                    );
                }
                Err(err) => {
                    log::warn!("[Audio] kira rejected positional sound: {err}");
                    buses.release_voice(pending.bus);
                }
            }
        }
    }

    /// Drop every voice whose sound has finished, releasing its slot on its
    /// bus. Dropping the track handle then lets kira remove the track; its
    /// sound is already done, so no tail is cut.
    pub(crate) fn reclaim_finished(&mut self, buses: &mut BusTree) {
        self.live.retain(|_, voice| {
            let finished = voice.sound.state() == PlaybackState::Stopped;
            if finished {
                buses.release_voice(voice.bus);
            }
            !finished
        });
    }

    /// Stop one voice, live or pending, returning the bus whose slot the caller
    /// must release, or `None` when `handle` is not ours.
    pub(crate) fn stop(&mut self, handle: SoundHandle, tween: Tween) -> Option<BusId> {
        if let Some(mut voice) = self.live.remove(&handle) {
            voice.sound.stop(tween);
            return Some(voice.bus);
        }
        let index = self
            .pending
            .iter()
            .position(|pending| pending.handle == handle)?;
        Some(self.pending.remove(index).bus)
    }

    /// Stop every voice, fading live ones over `tween`, and release each slot
    /// on its bus. A faded track persists in kira until its fade ends, so it
    /// never outlives the fade but never cuts it either.
    pub(crate) fn stop_all(&mut self, tween: Tween, buses: &mut BusTree) {
        for (_, mut voice) in self.live.drain() {
            voice.sound.stop(tween);
            buses.release_voice(voice.bus);
        }
        for pending in self.pending.drain(..) {
            buses.release_voice(pending.bus);
        }
    }

    /// [`stop_all`](Self::stop_all) for the voices on `bus` alone; every
    /// other voice plays on.
    pub(crate) fn stop_bus(&mut self, bus: BusId, tween: Tween, buses: &mut BusTree) {
        self.live.retain(|_, voice| {
            if voice.bus != bus {
                return true;
            }
            voice.sound.stop(tween);
            buses.release_voice(bus);
            false
        });
        self.pending.retain(|pending| {
            if pending.bus != bus {
                return true;
            }
            buses.release_voice(bus);
            false
        });
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
/// entity that no longer resolves to a finite pose starts frozen at its
/// fire-time point. Non-finite points never reach kira: a contact set picks its
/// nearest finite contact, and an anchor with no finite point has nowhere to
/// play.
fn start_position(
    anchor: &SoundAnchor,
    listener: [f32; 3],
    resolve: &mut dyn FnMut(u64) -> Option<[f32; 3]>,
) -> Option<([f32; 3], Option<u64>)> {
    match anchor {
        SoundAnchor::Entity { key, point } => match resolve(*key) {
            Some(position) if is_finite_point(position) => Some((position, Some(*key))),
            _ => is_finite_point(*point).then_some((*point, None)),
        },
        SoundAnchor::Point(point) => is_finite_point(*point).then_some((*point, None)),
        SoundAnchor::Contacts(points) => points
            .iter()
            .copied()
            .filter(|point| is_finite_point(*point))
            .min_by(|a, b| {
                distance_squared(*a, listener).total_cmp(&distance_squared(*b, listener))
            })
            .map(|point| (point, None)),
    }
}

/// Whether every coordinate of `point` is finite. The chokepoint's guard: a
/// NaN or infinite position fed to kira poisons its gain and panning math.
pub(crate) fn is_finite_point(point: [f32; 3]) -> bool {
    point.iter().all(|coordinate| coordinate.is_finite())
}

fn start_voice(
    parent: &mut TrackHandle,
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
    let mut track = parent
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

/// kira's relative volume is `curve(1 - relative distance)`, interpolated in
/// decibels, so `Linear` is a straight dB ramp. `Quadratic` is `OutPowi(2)`,
/// which evaluates to `1 - relative distance²`: it holds level near the
/// minimum distance, then falls off faster toward the maximum. (`InPowi(2)`
/// would give `(1 - relative distance)²`, the opposite shape.)
fn easing(curve: AttenuationCurve) -> Easing {
    match curve {
        AttenuationCurve::Linear => Easing::Linear,
        AttenuationCurve::Quadratic => Easing::OutPowi(2),
    }
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}
