// Per-frame presentation clocks: the host time each kind of member presents at.
// See: context/lib/entity_model.md §7d · context/lib/networking.md §Current contract

use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry};

use super::eligibility::{CandidateOrigin, GhostCategory};

/// What this machine knows about host time this frame, by role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ClockSource {
    /// No endpoint: the binary's own count of completed fixed ticks.
    SinglePlayer { completed_ticks: u32 },
    /// Listen host: the endpoint's post-increment authoritative tick.
    Host { tick: u32 },
    /// Connected client: the tick remotes were sampled at this frame, the
    /// estimated host tick movers re-simulate to, and the newest snapshot as the
    /// fallback before time sync names either.
    Client {
        render_server_tick: Option<f64>,
        estimated_server_tick: Option<f64>,
        latest_snapshot_tick: Option<u32>,
    },
}

/// Which presentation path draws a member on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberLane {
    /// Presented from this machine's own simulation at the sub-tick alpha.
    Simulated,
    /// Presented through a delay buffer: client pawns on the host seat, every
    /// replicated remote on a client.
    Remote,
    /// Kinematic movers.
    Mover,
}

/// Fractional host ticks, in snapshot-stamp units, that each lane presents at
/// this frame. Link-state history is read at these values, so a link change and
/// the motion it reveals present on the same frame on every machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentationClocks {
    simulated: f64,
    remote: f64,
    mover: f64,
}

impl PresentationClocks {
    pub(crate) fn compute(source: ClockSource, alpha: f32) -> Self {
        match source {
            ClockSource::SinglePlayer { completed_ticks } => {
                let now = sub_tick(completed_ticks, alpha);
                Self {
                    simulated: now,
                    remote: now,
                    mover: now,
                }
            }
            ClockSource::Host { tick } => {
                // The registry blends end-of-tick (tick - 1) toward end-of-tick
                // (tick) states by alpha; client pawns trail by the host-seat delay.
                let now = sub_tick(tick, alpha);
                Self {
                    simulated: now,
                    remote: crate::netcode::host_presented_client_pawn_tick(tick, alpha),
                    mover: now,
                }
            }
            ClockSource::Client {
                render_server_tick,
                estimated_server_tick,
                latest_snapshot_tick,
            } => {
                // Before time sync, remotes hold their newest applied snapshot.
                let held = latest_snapshot_tick.map_or(0.0, f64::from);
                let mover = estimated_server_tick.unwrap_or(held);
                Self {
                    // A client simulates nothing that ghosts: its own pawn and
                    // shots never ghost for it. Host-now is the defined fallback.
                    simulated: mover,
                    remote: render_server_tick.unwrap_or(held),
                    mover,
                }
            }
        }
    }

    pub(crate) fn lane(&self, lane: MemberLane) -> f64 {
        match lane {
            MemberLane::Simulated => self.simulated,
            MemberLane::Remote => self.remote,
            MemberLane::Mover => self.mover,
        }
    }

    /// The lane clock this machine's script clock presents at this frame: the
    /// host time a look ramp evaluated at this frame's script time shows. On
    /// the host and in single player that is the simulated tick its frame
    /// script time stamps commands with; on a client, the estimated host tick,
    /// which its rebased script clock tracks.
    pub(crate) fn script_now(&self) -> f64 {
        self.mover
    }

    /// How far behind [`Self::script_now`] a member presents: its link-state
    /// read at `state_clock` (its pose stamp, or its lane clock) and its look
    /// ramps at `lane_clock`, its lane's continuous clock.
    pub(crate) fn lag(&self, state_clock: f64, lane_clock: f64) -> MemberLag {
        let now = self.script_now();
        MemberLag {
            state_ticks: now - state_clock,
            look_ticks: (now - lane_clock).max(0.0),
        }
    }

    /// The earliest lane clock this frame.
    pub(crate) fn oldest(&self) -> f64 {
        self.simulated.min(self.remote).min(self.mover)
    }

    /// The earliest host time any member reads this frame, pose stamps
    /// included; history older than the change covering it is unreachable.
    pub(crate) fn oldest_readable(&self) -> f64 {
        self.oldest() - POSE_STAMP_LAG_TICKS
    }
}

/// How far behind this machine's script clock a member presents, in host ticks.
///
/// A member reads its link's state at `script_now - state_ticks` and evaluates
/// the look ramps of that state at the script time `look_ticks` fixed ticks
/// before this frame's: lanes trail host-now by whole presentation delays, and
/// a fixed tick is the one exact conversion between host ticks and script
/// seconds on every machine, so no host script-time history is kept. A look
/// ramp never presents before its own start once the state carrying it is in
/// force ([`super::look::LinkLooks::resolve`]), so a `setColorFilter` lands
/// with the motion of its tick even where the two clocks differ by a frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct MemberLag {
    pub state_ticks: f64,
    pub look_ticks: f64,
}

impl MemberLag {
    /// Script seconds the member's look trails this frame's script time by.
    pub(crate) fn look_seconds(self) -> f64 {
        self.look_ticks * crate::frame_timing::TICK_DURATION.as_secs_f64()
    }
}

/// How far below its lane clock a member's pose stamp can read: a pose held on
/// the near side of a teleport cut reads just before the cut's tick, which trails
/// the lane clock by less than one tick. History pruning keeps that much more.
pub(crate) const POSE_STAMP_LAG_TICKS: f64 = 1.0;

/// Where this frame's per-member pose stamps come from: the host time whose
/// state each delay-buffered member's presented pose actually shows. A blend
/// shows its leading sample, so a link change and the first motion toward it
/// present together; a pose held on the near side of a teleport cut shows no
/// state of the cut's tick, so an arm sent with a swap presents on the cut
/// frame rather than one tick early (P18). Members with no stamp read their
/// lane clock.
#[derive(Clone, Copy)]
pub(crate) enum PoseStamps<'a> {
    /// No delay-buffered member: single player.
    None,
    /// Listen host: connected-client pawns presented through the host-seat buffer.
    HostSeat {
        buffer: &'a crate::netcode::RemoteInterpolationBuffer,
        allocator: &'a crate::netcode::NetworkIdAllocator,
        tick: u32,
        alpha: f32,
    },
    /// Connected client: every interpolated remote.
    Client(&'a crate::netcode::ClientReplication),
}

impl PoseStamps<'_> {
    /// The pose stamp of `entity` this frame, in snapshot-stamp units, for a
    /// member presenting through `lane`. `None` falls back to the lane clock.
    pub(crate) fn state_tick(&self, entity: EntityId, lane: MemberLane) -> Option<f64> {
        if lane != MemberLane::Remote {
            return None;
        }
        match *self {
            Self::None => None,
            Self::HostSeat {
                buffer,
                allocator,
                tick,
                alpha,
            } => crate::netcode::host_presented_client_pawn_state_tick(
                buffer, allocator, entity, tick, alpha,
            ),
            Self::Client(replication) => replication.presented_state_tick(entity),
        }
    }
}

fn sub_tick(post_increment_tick: u32, alpha: f32) -> f64 {
    f64::from(post_increment_tick) - 1.0 + f64::from(alpha)
}

/// The lane a candidate presents through. On the host a slot pawn the host
/// simulates for a client presents delayed; every replicated copy on a client
/// presents at the remote render tick.
pub(crate) fn member_lane(
    registry: &EntityRegistry,
    entity: EntityId,
    origin: CandidateOrigin<'_>,
    category: GhostCategory,
) -> MemberLane {
    if category == GhostCategory::Movers {
        return MemberLane::Mover;
    }
    match origin {
        CandidateOrigin::Replicated(_) => MemberLane::Remote,
        CandidateOrigin::Local => {
            let slot_pawn = registry
                .get_component::<DescriptorProvenance>(entity)
                .is_ok_and(|provenance| provenance.spawn_path == DescriptorSpawnPath::NetworkSlot);
            if slot_pawn {
                MemberLane::Remote
            } else {
                MemberLane::Simulated
            }
        }
    }
}

#[cfg(test)]
#[path = "clocks_tests.rs"]
mod tests;
