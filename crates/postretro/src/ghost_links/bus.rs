// Ghost bus profile on this peer: floor, then manifest `audio.ghost`, then the host's `setGhostAudio` fields, received on a client.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/scripting.md §10.4

use std::time::Duration;

use postretro_foundation::{
    GhostAudioOverride, GhostAudioTargets, GhostAudioValues, GhostScalarRamp,
};
use postretro_scripting_core::runtime::ModGhostAudio;

use crate::startup::audio_profile::{audio_ghost_profile, ghost_audio_values};

/// Warning text for a `setGhostAudio` whose targets cross the cutoffs; keep it
/// stable.
pub(crate) const CROSSING_STEP_WARNING: &str = "would leave highPassHz at or above lowPassHz";

/// Warning text for a resolved profile whose cutoffs cross; keep it stable.
pub(crate) const CROSSED_PROFILE_WARNING: &str =
    "resolved Ghost bus high-pass reaches its low-pass";

/// Longest audio-thread ramp one presented change asks for. A ramp in flight
/// re-targets every frame, so a frame-length tween keeps the bus moving
/// without a step; a long hitch snaps rather than sliding across the gap.
const MAX_PRESENT_RAMP: Duration = Duration::from_millis(100);

/// Each script field's slot, in the replicated mask's bit order.
fn slots(fields: &GhostAudioOverride) -> [Option<GhostScalarRamp>; 4] {
    [
        fields.volume,
        fields.high_pass_hz,
        fields.low_pass_hz,
        fields.drive,
    ]
}

fn slots_mut(fields: &mut GhostAudioOverride) -> [&mut Option<GhostScalarRamp>; 4] {
    [
        &mut fields.volume,
        &mut fields.high_pass_hz,
        &mut fields.low_pass_hz,
        &mut fields.drive,
    ]
}

/// Each field's accepted range, in slot order: the ranges the manifest drain
/// and the `setGhostAudio` parser accept.
const RANGES: [(f32, f32); 4] = [
    (0.0, 1.0),
    (ModGhostAudio::MIN_CUTOFF_HZ, ModGhostAudio::MAX_CUTOFF_HZ),
    (ModGhostAudio::MIN_CUTOFF_HZ, ModGhostAudio::MAX_CUTOFF_HZ),
    (0.0, 1.0),
];

/// Whether both ends of `ramp` lie in `range`. Every value between them then
/// does too.
fn in_range(ramp: GhostScalarRamp, (min, max): (f32, f32)) -> bool {
    (min..=max).contains(&ramp.from) && (min..=max).contains(&ramp.to)
}

/// A received `ramp` with both ends clamped into `range` and its start moved
/// `offset` seconds onto this client's script clock. An honest host only sends
/// values its parser accepted; clamping keeps a bad one from reaching the bus,
/// which would refuse the whole profile and keep its old mix.
fn rebase(
    ramp: Option<GhostScalarRamp>,
    (min, max): (f32, f32),
    offset: f64,
) -> Option<GhostScalarRamp> {
    ramp.map(|ramp| GhostScalarRamp {
        from: ramp.from.clamp(min, max),
        to: ramp.to.clamp(min, max),
        start_time: ramp.start_time + offset,
        duration_ms: ramp.duration_ms,
    })
}

/// A connected client's newest received host fields, on the host's script
/// clock as the wire carried them, and per field whether its ramp arrived
/// before the presentation clock anchored and waits to rebase.
#[derive(Debug, Default, Clone, Copy)]
struct ReceivedFields {
    host: GhostAudioOverride,
    deferred: [bool; 4],
}

/// The Ghost bus mix this peer presents. The manifest profile is mod-wide and
/// follows each commit; the script fields are level-scoped and the host's.
/// Resolution is per field: the floor (`ModGhostAudio::FLOOR`, which the
/// manifest drain already folded into any field it omits or rejects), then the
/// manifest, then each field a script set, at its ramp's value now.
#[derive(Debug)]
pub(crate) struct GhostBus {
    manifest: ModGhostAudio,
    /// The fields `setGhostAudio` has set, with their ramps on this peer's
    /// script clock: the host's own, or on a connected client the host's
    /// received ramps rebased.
    fields: GhostAudioOverride,
    /// Connected client only: the host record the script layer came from.
    received: ReceivedFields,
    /// Calls drained since the last authoritative tick, in call order.
    pending: Vec<GhostAudioTargets>,
    /// The values last handed to the audio bus; `None` until the first
    /// presentation, or after a commit forces the next one.
    presented: Option<GhostAudioValues>,
    /// The resolved cutoffs crossed at the last resolution; the warning fires
    /// once per crossing, not per frame.
    crossed: bool,
}

impl Default for GhostBus {
    fn default() -> Self {
        Self {
            manifest: ModGhostAudio::FLOOR,
            fields: GhostAudioOverride::default(),
            received: ReceivedFields::default(),
            pending: Vec::new(),
            presented: None,
            crossed: false,
        }
    }
}

impl GhostBus {
    /// A mod init or committed staged reload: the manifest profile the
    /// unscripted fields follow. Script fields and their ramps stand.
    pub(crate) fn set_manifest(&mut self, profile: ModGhostAudio) {
        self.manifest = profile;
        self.presented = None;
    }

    /// The fields `setGhostAudio` has set: what the host's level-scoped record
    /// carries every snapshot.
    pub(crate) fn script_fields(&self) -> &GhostAudioOverride {
        &self.fields
    }

    /// Queue one drained `setGhostAudio` for the next authoritative tick.
    pub(crate) fn queue(&mut self, targets: GhostAudioTargets) {
        self.pending.push(targets);
    }

    /// Level unload or demotion: drop every script field and queued call, so
    /// the bus returns to the manifest profile.
    pub(crate) fn drop_script_fields(&mut self) {
        self.fields = GhostAudioOverride::default();
        self.received = ReceivedFields::default();
        self.pending.clear();
    }

    /// Connected client: take one received host record as the script layer
    /// over this peer's own floor and manifest. A field whose host ramp
    /// changed rebases by `offset` (this client's script clock minus the
    /// host's) now; a repeat changes nothing, so the presented value never
    /// moves as the RTT estimate drifts. Before the presentation clock
    /// anchors (`offset` `None`) a changed ramp waits
    /// ([`Self::release_deferred`]) and the field keeps what it presented,
    /// while a field the host no longer sets drops at once. A changed value
    /// outside its field's range clamps into it, warning once for the record.
    pub(crate) fn apply_received(&mut self, host: GhostAudioOverride, offset: Option<f64>) {
        let previous = slots(&self.received.host);
        let deferred = &mut self.received.deferred;
        let mut out_of_range = false;
        for (slot, (local, ramp)) in slots_mut(&mut self.fields)
            .into_iter()
            .zip(slots(&host))
            .enumerate()
        {
            if ramp == previous[slot] {
                continue;
            }
            out_of_range |= ramp.is_some_and(|ramp| !in_range(ramp, RANGES[slot]));
            match offset {
                Some(offset) => {
                    *local = rebase(ramp, RANGES[slot], offset);
                    deferred[slot] = false;
                }
                None if ramp.is_none() => {
                    *local = None;
                    deferred[slot] = false;
                }
                None => deferred[slot] = true,
            }
        }
        if out_of_range {
            log::warn!(
                "[Audio] received Ghost bus record {host:?} carries a value outside its field's range; clamped"
            );
        }
        self.received.host = host;
    }

    /// Once the presentation clock anchors, rebase every deferred field and
    /// put it in force. No work while nothing waits.
    pub(crate) fn release_deferred(&mut self, offset: Option<f64>) {
        let Some(offset) = offset else {
            return;
        };
        let host = slots(&self.received.host);
        let deferred = &mut self.received.deferred;
        for (slot, local) in slots_mut(&mut self.fields).into_iter().enumerate() {
            if std::mem::take(&mut deferred[slot]) {
                *local = rebase(host[slot], RANGES[slot], offset);
            }
        }
    }

    /// One frame's intake on a connected client: the newest received record,
    /// if one landed, then any field the clock anchoring released.
    pub(crate) fn intake_received(
        &mut self,
        record: Option<GhostAudioOverride>,
        offset: Option<f64>,
    ) {
        if let Some(host) = record {
            self.apply_received(host, offset);
        }
        self.release_deferred(offset);
    }

    /// Apply the queued calls at the start of an authoritative tick, in call
    /// order, at that tick's script time `time`. Each named field ramps from
    /// the unclamped value as it stood before this tick, so the later of two
    /// calls in one tick wins per field and still starts from the pre-tick
    /// value. A call whose targets would cross the cutoffs, judged against the
    /// targets after it (a ramp in flight by its target), warns and drops.
    pub(crate) fn apply_pending(&mut self, time: f64) {
        if self.pending.is_empty() {
            return;
        }
        let base = ghost_audio_values(self.manifest);
        let before = self.fields.resolve(base, time);
        for step in self.pending.drain(..) {
            let after = self.fields.targets_after(base, &step);
            if after.cutoffs_cross() {
                log::warn!(
                    "[Audio] setGhostAudio {CROSSING_STEP_WARNING} (highPassHz {} Hz, lowPassHz {} Hz); the reaction was dropped",
                    after.high_pass_hz,
                    after.low_pass_hz,
                );
                continue;
            }
            self.fields.set(before, &step, time);
        }
    }

    /// The profile the bus presents at script time `time`. A resolution whose
    /// high-pass reaches its low-pass presents the floor cutoffs instead,
    /// warning once until the cutoffs part again.
    pub(crate) fn resolve(&mut self, time: f64) -> GhostAudioValues {
        let mut resolved = self.fields.resolve(ghost_audio_values(self.manifest), time);
        let crossed = resolved.cutoffs_cross();
        if crossed {
            if !self.crossed {
                log::warn!(
                    "[Audio] {CROSSED_PROFILE_WARNING} (highPassHz {} Hz, lowPassHz {} Hz); presenting the floor cutoffs",
                    resolved.high_pass_hz,
                    resolved.low_pass_hz,
                );
            }
            resolved.high_pass_hz = ModGhostAudio::FLOOR.high_pass_hz;
            resolved.low_pass_hz = ModGhostAudio::FLOOR.low_pass_hz;
        }
        self.crossed = crossed;
        resolved
    }

    /// Resolve at `time` and return the profile to hand the bus, only when it
    /// differs from the last one handed over. Steady state sends nothing; a
    /// ramp in flight sends each frame's value, which the bus tweens to over
    /// `frame` (at least its own de-click floor), so it never steps.
    pub(crate) fn present(
        &mut self,
        time: f64,
        frame: Duration,
    ) -> Option<(postretro_audio::GhostBusProfile, Duration)> {
        let resolved = self.resolve(time);
        if self.presented == Some(resolved) {
            return None;
        }
        self.presented = Some(resolved);
        Some((audio_ghost_profile(resolved), frame.min(MAX_PRESENT_RAMP)))
    }
}

impl crate::App {
    /// The frame-end drain of `setGhostAudio`. Host and single player queue it
    /// for the next fixed tick; a connected client drops it, since the bus
    /// fields are the host's and reach clients by replication.
    pub(crate) fn queue_ghost_audio(&mut self, targets: GhostAudioTargets) {
        if !self.is_connected_client() {
            self.ghost_bus.queue(targets);
        }
    }

    /// Host and single player, at each fixed tick's start, beside the ghost-link
    /// commands.
    pub(crate) fn apply_pending_ghost_audio_commands(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let time = session.scripting.script_ctx.presentation_script_time.get();
        self.ghost_bus.apply_pending(time);
    }

    /// Hand the bus its resolved profile when it changed: once per frame in the
    /// audio stage, and at once after a commit, unload or demotion. `frame` is
    /// the ramp a ramp-in-flight change asks for.
    pub(crate) fn present_ghost_bus(&mut self, frame: Duration) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let time = session.scripting.script_ctx.presentation_script_time.get();
        if let Some((profile, ramp)) = self.ghost_bus.present(time, frame)
            && let Some(audio) = session.audio.as_mut()
        {
            audio.set_ghost_bus_profile(profile, ramp);
        }
    }

    /// On a connected client, before the frame's bus presentation: take the
    /// newest received ghost-audio record as the bus's script layer, rebased
    /// onto this client's script clock.
    pub(crate) fn drain_received_ghost_audio(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let local_script_time = session.scripting.script_ctx.presentation_script_time.get();
        let Some(crate::netcode::NetEndpoint::Client {
            replication,
            time_sync,
            ..
        }) = session.net_endpoint.as_mut()
        else {
            return;
        };
        let offset = super::replication::host_clock_offset(
            replication,
            local_script_time,
            time_sync.rtt_micros(),
        );
        let record = replication
            .take_ghost_audio()
            .map(|state| crate::netcode::ghost_audio_from_wire(&state));
        self.ghost_bus.intake_received(record, offset);
    }

    /// Level unload or demotion: drop the script fields and return the bus to
    /// the resolved manifest profile now, not at the next level's first frame.
    pub(crate) fn drop_script_ghost_audio(&mut self) {
        self.ghost_bus.drop_script_fields();
        self.present_ghost_bus(Duration::ZERO);
    }
}

#[cfg(test)]
#[path = "bus_tests.rs"]
mod tests;
