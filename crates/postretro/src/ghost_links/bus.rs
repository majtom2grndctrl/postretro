// Ghost bus profile on this peer: floor, then manifest `audio.ghost`, then the host's `setGhostAudio` fields.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/scripting.md §10.4

use std::time::Duration;

use postretro_foundation::{GhostAudioOverride, GhostAudioTargets, GhostAudioValues};
use postretro_scripting_core::runtime::ModGhostAudio;

use crate::startup::audio_profile::{audio_ghost_profile, ghost_audio_values};

/// A `setGhostAudio` whose targets cross the cutoffs: stable wording
/// (acceptance contract, P29).
pub(crate) const CROSSING_STEP_WARNING: &str = "would leave highPassHz at or above lowPassHz";

/// A resolved profile whose cutoffs cross: stable wording (acceptance
/// contract, P36).
pub(crate) const CROSSED_PROFILE_WARNING: &str =
    "resolved Ghost bus high-pass reaches its low-pass";

/// Longest audio-thread ramp one presented change asks for. A ramp in flight
/// re-targets every frame, so a frame-length tween keeps the bus moving
/// without a step; a long hitch snaps rather than sliding across the gap.
const MAX_PRESENT_RAMP: Duration = Duration::from_millis(100);

/// The Ghost bus mix this peer presents. The manifest profile is mod-wide and
/// follows each commit; the script fields are level-scoped and the host's.
/// Resolution is per field: the floor (`ModGhostAudio::FLOOR`, which the
/// manifest drain already folded into any field it omits or rejects), then the
/// manifest, then each field a script set, at its ramp's value now.
#[derive(Debug)]
pub(crate) struct GhostBus {
    manifest: ModGhostAudio,
    /// The fields `setGhostAudio` has set, with their ramps in host script time.
    fields: GhostAudioOverride,
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
            pending: Vec::new(),
            presented: None,
            crossed: false,
        }
    }
}

impl GhostBus {
    /// A mod init or committed staged reload: the manifest profile the
    /// unscripted fields follow. Script fields and their ramps stand (P17).
    pub(crate) fn set_manifest(&mut self, profile: ModGhostAudio) {
        self.manifest = profile;
        self.presented = None;
    }

    /// The fields `setGhostAudio` has set: what the level-scoped record carries.
    // The replicated record (ghost-entities Task 13b) reads this; until it
    // lands only tests do.
    #[cfg_attr(not(test), allow(dead_code))]
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
        self.pending.clear();
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
    /// warning once until the cutoffs part again (P36).
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
