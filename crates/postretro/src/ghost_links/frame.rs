// Frame-loop glue: record link history after each fixed tick, evaluate membership per frame.
// See: context/lib/entity_model.md §7d · context/lib/networking.md §Current contract

use std::time::Instant;

use super::GhostFrame;
use super::clocks::{ClockSource, PresentationClocks};
use super::eligibility::CandidateView;
use crate::netcode::NetEndpoint;

impl crate::App {
    /// After each completed fixed tick on the host or in single player: stamp
    /// every link's end-of-tick state. A connected client simulates no link
    /// state; it takes it from replication.
    pub(crate) fn record_ghost_link_history(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let host_tick = match session.net_endpoint.as_ref() {
            Some(NetEndpoint::Host { tick, .. }) => Some(*tick),
            Some(NetEndpoint::Client { .. }) => return,
            None => None,
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        self.ghost_links
            .record_after_fixed_tick(&registry, host_tick);
    }

    /// Once per frame, after every presented pose is written (remote sampling,
    /// host-seat client pawns) and before render collection reads entities.
    pub(crate) fn update_ghost_links(&mut self, alpha: f32, now: Instant) {
        let elapsed_seconds = self.ghost_links.frame_elapsed(now);
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let (source, view) = match session.net_endpoint.as_ref() {
            Some(NetEndpoint::Host { tick, .. }) => (
                ClockSource::Host { tick: *tick },
                CandidateView::Authoritative,
            ),
            Some(NetEndpoint::Client {
                replication,
                time_sync,
                ..
            }) => (
                ClockSource::Client {
                    render_server_tick: self.remote_player_presentation.render_server_tick,
                    estimated_server_tick: time_sync.estimated_server_tick(),
                    latest_snapshot_tick: replication.latest_server_tick(),
                },
                CandidateView::Client(replication),
            ),
            None => (
                ClockSource::SinglePlayer {
                    completed_ticks: self.ghost_links.completed_ticks(),
                },
                CandidateView::Authoritative,
            ),
        };
        let registry = session.scripting.script_ctx.registry.borrow();
        self.ghost_links.update(
            &registry,
            GhostFrame {
                clocks: PresentationClocks::compute(source, alpha),
                alpha,
                elapsed_seconds,
                view,
            },
        );
    }
}
