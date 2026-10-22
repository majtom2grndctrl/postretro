// Received link state: replicated records into link history, look ramps rebased onto this client's clock.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use postretro_foundation::{GhostColorFilter, GhostScalarRamp, GhostTintRamp};
use postretro_level_format::ghost_links::GhostMembers;

use super::GhostLinks;
use super::history::LinkState;
use crate::netcode::{GhostLinkUpdate, NetEndpoint};

/// One `setColorFilter` field, in the wire mask's bit order.
#[derive(Debug, Clone, Copy)]
enum LookField {
    Tint,
    Opacity,
    Saturation,
}

const LOOK_FIELDS: [LookField; 3] = [LookField::Tint, LookField::Opacity, LookField::Saturation];

/// A received look value clamped into its `0..=1` range. An honest host only
/// sends values its parser accepted; a bad one clamps, warning once for the
/// record that carried it.
fn unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn in_unit(value: f32) -> bool {
    (0.0..=1.0).contains(&value)
}

fn rebase_scalar(ramp: Option<GhostScalarRamp>, offset: f64) -> Option<GhostScalarRamp> {
    ramp.map(|ramp| GhostScalarRamp {
        from: unit(ramp.from),
        to: unit(ramp.to),
        start_time: ramp.start_time + offset,
        duration_ms: ramp.duration_ms,
    })
}

impl LookField {
    fn same(self, a: &GhostColorFilter, b: &GhostColorFilter) -> bool {
        match self {
            Self::Tint => a.tint == b.tint,
            Self::Opacity => a.opacity == b.opacity,
            Self::Saturation => a.saturation == b.saturation,
        }
    }

    /// Whether this field's ramp, if set, lies in range end to end.
    fn in_range(self, filter: &GhostColorFilter) -> bool {
        match self {
            Self::Tint => filter
                .tint
                .is_none_or(|ramp| ramp.from.iter().chain(&ramp.to).copied().all(in_unit)),
            Self::Opacity => filter
                .opacity
                .is_none_or(|ramp| in_unit(ramp.from) && in_unit(ramp.to)),
            Self::Saturation => filter
                .saturation
                .is_none_or(|ramp| in_unit(ramp.from) && in_unit(ramp.to)),
        }
    }

    fn is_set(self, filter: &GhostColorFilter) -> bool {
        match self {
            Self::Tint => filter.tint.is_some(),
            Self::Opacity => filter.opacity.is_some(),
            Self::Saturation => filter.saturation.is_some(),
        }
    }

    /// Write this field of `host` into `local`, its values clamped into range
    /// and its start moved `offset` seconds onto this client's script clock.
    fn rebase(self, local: &mut GhostColorFilter, host: &GhostColorFilter, offset: f64) {
        match self {
            Self::Tint => {
                local.tint = host.tint.map(|ramp| GhostTintRamp {
                    from: ramp.from.map(unit),
                    to: ramp.to.map(unit),
                    start_time: ramp.start_time + offset,
                    duration_ms: ramp.duration_ms,
                });
            }
            Self::Opacity => local.opacity = rebase_scalar(host.opacity, offset),
            Self::Saturation => local.saturation = rebase_scalar(host.saturation, offset),
        }
    }

    fn copy(self, to: &mut GhostColorFilter, from: &GhostColorFilter) {
        match self {
            Self::Tint => to.tint = from.tint,
            Self::Opacity => to.opacity = from.opacity,
            Self::Saturation => to.saturation = from.saturation,
        }
    }
}

/// One link's received colour filter.
#[derive(Debug, Clone, Copy, Default)]
struct ReceivedLook {
    /// The newest received filter, on the host's script clock as the wire
    /// carried it.
    host: GhostColorFilter,
    /// The same ramps on this client's script clock. Each field is rebased
    /// once, when its host ramp changes, so a record repeating it every
    /// snapshot never moves the presented value as the RTT estimate drifts.
    local: GhostColorFilter,
    /// Per field in [`LOOK_FIELDS`] order: the stamp of the record whose ramp
    /// arrived before the presentation clock anchored and waits to rebase.
    deferred: [Option<u32>; 3],
}

/// A connected client's received look filters, one per installed link.
#[derive(Debug, Default)]
pub(crate) struct ReceivedLooks {
    links: Vec<ReceivedLook>,
}

impl ReceivedLooks {
    pub(crate) fn new(links: usize) -> Self {
        Self {
            links: vec![ReceivedLook::default(); links],
        }
    }
}

impl GhostLinks {
    /// Push one received link state into its link's history at the stamp of
    /// the snapshot that carried it. Netcode bound the record to this link by
    /// map-order index and source entity; a repeated state changes nothing.
    ///
    /// `offset` is this client's script clock minus the host's (`None` before
    /// the presentation clock anchors). A look field whose host ramp changed
    /// rebases by it now; before the anchor, armed state and categories apply
    /// and the ramp waits ([`Self::release_deferred_looks`]).
    pub(crate) fn apply_received(&mut self, update: GhostLinkUpdate, offset: Option<f64>) {
        let index = usize::from(update.state.index);
        let (Some(link), Some(received)) = (
            self.links.get_mut(index),
            self.received.links.get_mut(index),
        ) else {
            return;
        };
        let host = crate::netcode::ghost_color_filter_from_wire(&update.state);
        let mut out_of_range = false;
        for (slot, field) in LOOK_FIELDS.into_iter().enumerate() {
            if field.same(&host, &received.host) {
                continue;
            }
            out_of_range |= !field.in_range(&host);
            match offset {
                Some(offset) => {
                    field.rebase(&mut received.local, &host, offset);
                    received.deferred[slot] = None;
                }
                None if !field.is_set(&host) => {
                    field.copy(&mut received.local, &host);
                    received.deferred[slot] = None;
                }
                None => received.deferred[slot] = Some(update.server_tick),
            }
        }
        if out_of_range {
            log::warn!(
                "[Net] received ghost link {index} look carries a value outside 0..=1; clamped"
            );
        }
        received.host = host;
        link.history.push(
            update.server_tick,
            LinkState {
                filter: received.local,
                // The net crate rejected reserved bits before apply.
                ..LinkState::new(
                    update.state.armed,
                    GhostMembers::from_bits_truncate(update.state.members),
                )
            },
        );
    }

    /// Once the presentation clock anchors, rebase every deferred look ramp
    /// and put it in force from the stamp of the record that carried it. No
    /// work while nothing waits.
    pub(crate) fn release_deferred_looks(&mut self, offset: Option<f64>) {
        let Some(offset) = offset else {
            return;
        };
        for (link, received) in self.links.iter_mut().zip(&mut self.received.links) {
            for (slot, field) in LOOK_FIELDS.into_iter().enumerate() {
                let Some(stamp) = received.deferred[slot].take() else {
                    continue;
                };
                field.rebase(&mut received.local, &received.host, offset);
                let local = received.local;
                link.history
                    .amend_from(stamp, |state| field.copy(&mut state.filter, &local));
            }
        }
    }

    /// One frame's received link states, in apply order, then any look ramp
    /// the clock anchoring released. Every state is kept, so one that lasted
    /// only between two frames still presents at its own host time.
    pub(crate) fn intake_received(
        &mut self,
        updates: impl Iterator<Item = GhostLinkUpdate>,
        offset: Option<f64>,
    ) {
        for update in updates {
            self.apply_received(update, offset);
        }
        self.release_deferred_looks(offset);
    }

    /// Drop every received state: each link returns to its local spawn state
    /// and style, presented through the membership ramp. Demotion calls this;
    /// a level unload drops the whole table.
    pub(crate) fn reset_received_state(&mut self) {
        for link in &mut self.links {
            link.history.reset_to_spawn();
        }
        self.received.links.fill(ReceivedLook::default());
    }
}

/// This client's script clock minus the host's, from the latest accepted
/// snapshot's host script time plus half the measured RTT: the rule bound
/// presentation state rebases by. `None` until the clock anchors.
pub(crate) fn host_clock_offset(
    replication: &crate::netcode::ClientReplication,
    local_script_time: f64,
    rtt_micros: Option<f64>,
) -> Option<f64> {
    replication
        .presentation_host_time(local_script_time, rtt_micros)
        .map(|host_now| local_script_time - host_now)
}

impl crate::App {
    /// On a connected client, move this frame's received link states into link
    /// history, rebasing look ramps onto this client's script clock. Runs once
    /// per frame at the top of the audio stage, before ghost-voice admission
    /// and the membership update both read that history.
    pub(crate) fn drain_received_ghost_link_state(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let local_script_time = session.scripting.script_ctx.presentation_script_time.get();
        let Some(NetEndpoint::Client {
            replication,
            time_sync,
            ..
        }) = session.net_endpoint.as_mut()
        else {
            return;
        };
        let offset = host_clock_offset(replication, local_script_time, time_sync.rtt_micros());
        self.ghost_links
            .intake_received(replication.drain_ghost_link_updates(), offset);
    }
}

#[cfg(test)]
#[path = "replication_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "replication_test_fixtures.rs"]
mod test_fixtures;

#[cfg(test)]
#[path = "replication_loopback_tests.rs"]
mod loopback_tests;

#[cfg(test)]
#[path = "replication_mover_tests.rs"]
mod mover_tests;

#[cfg(test)]
#[path = "replication_look_tests.rs"]
mod look_tests;

#[cfg(test)]
#[path = "replication_audio_tests.rs"]
mod audio_tests;
