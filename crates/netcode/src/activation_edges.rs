//! Correlated start/release/cancel recovery beside movement playout.
use crate::prediction::client_tick_le;
use crate::sim::RemoteStartAim;
use postretro_foundation::{ActivationInput, ActivationRelease, ActivationToken};
use std::collections::VecDeque;

pub const MAX_RETAINED_ACTIVATION_EDGES: usize = 64;
const RETENTION_TICKS: u32 = 120;
#[derive(Debug, Clone, Copy)]
struct RetainedEdge {
    token: ActivationToken,
    release_tick: Option<u32>,
    cancel: bool,
    first_tick: u32,
    cancel_first_tick: Option<u32>,
    delivered: bool,
    cancel_delivered: bool,
}
/// A client-named start retained beside movement playout, so a catch-up trim or
/// stale-drop of its carrying command cannot erase it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RetainedStart {
    pub token: ActivationToken,
    /// Client tick of the carrying command. The start is due once the resolved
    /// cursor reaches it, so it never fires ahead of its own aim.
    pub command_tick: u32,
    /// Firing slot the carrying command named.
    pub firing_slot: u8,
    /// Aim the carrying command declared, already through intake sanitization.
    /// The start's shot fires along it, whichever later command delivers it.
    pub aim: RemoteStartAim,
    /// Host tick at which playout first found it due. Expiry runs from here, so
    /// a start playout has not reached yet is never refused for waiting.
    due_since: Option<u32>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DueStart {
    Start(RetainedStart),
    /// Two seconds passed without admission; the caller publishes the refusal.
    Expired(ActivationToken),
}
#[derive(Debug, Default)]
pub(crate) struct ActivationEdges {
    edges: VecDeque<RetainedEdge>,
    starts: VecDeque<RetainedStart>,
    /// The start most recently taken from the lane, for its shot's aim.
    delivered: Option<RetainedStart>,
    admitted: VecDeque<ActivationToken>,
    overflow_cancel: Option<ActivationToken>,
    settled_start: [Option<u32>; 2],
}
impl ActivationEdges {
    /// Retain a start in intake order. Duplicates, settled starts, and starts past
    /// the per-client bound are not retained, so playout never delivers them.
    pub fn observe_start(
        &mut self,
        token: ActivationToken,
        command_tick: u32,
        firing_slot: u8,
        aim: RemoteStartAim,
    ) {
        if self.is_settled(token)
            || self.admitted.contains(&token)
            || self.starts.iter().any(|start| start.token == token)
            || self.starts.len() >= MAX_RETAINED_ACTIVATION_EDGES
        {
            return;
        }
        self.starts.push_back(RetainedStart {
            token,
            command_tick,
            firing_slot,
            aim,
            due_since: None,
        });
    }
    /// The oldest retained start once its command tick is resolved and no
    /// execution is live. Starts leave strictly in intake order, so the ledger's
    /// monotonic settled-start watermark never passes a start still retained.
    /// A due start not admitted within two seconds expires.
    pub fn due_start(&mut self, resolved_tick: u32, tick: u32, live: bool) -> Option<DueStart> {
        let front = self.starts.front_mut()?;
        if !client_tick_le(front.command_tick, resolved_tick) {
            return None;
        }
        let due_since = *front.due_since.get_or_insert(tick);
        if tick.wrapping_sub(due_since) >= RETENTION_TICKS {
            let token = front.token;
            self.starts.pop_front();
            return Some(DueStart::Expired(token));
        }
        (!live).then_some(DueStart::Start(*front))
    }
    /// Remove the delivered front start and open its edge correlation. False
    /// when retention settled it meanwhile; the caller refuses it.
    pub fn take_start(&mut self, token: ActivationToken) -> bool {
        if self
            .starts
            .front()
            .is_some_and(|start| start.token == token)
        {
            self.delivered = self.starts.pop_front();
        }
        self.admit(token)
    }
    /// Aim captured with `token`'s start, once the lane has delivered it.
    pub fn delivered_aim(&self, token: ActivationToken) -> Option<RemoteStartAim> {
        self.delivered
            .filter(|start| start.token == token)
            .map(|start| start.aim)
    }
    pub fn observe(&mut self, input: ActivationInput, tick: u32) {
        self.prune(tick);
        if let Some(release) = input.release {
            self.retain(release.token, Some(release.release_tick), false, tick);
        }
        if let Some(token) = input.cancel {
            self.retain(token, None, true, tick);
        }
    }
    fn retain(
        &mut self,
        token: ActivationToken,
        release_tick: Option<u32>,
        cancel: bool,
        tick: u32,
    ) {
        if self.is_settled(token) && !self.admitted.contains(&token) {
            return;
        }
        if let Some(edge) = self.edges.iter_mut().find(|e| e.token == token) {
            // Cancellation may strengthen an undelivered release; duplicates never
            // refresh age or redeliver a terminal edge.
            if cancel && edge.cancel_first_tick.is_none() && !edge.cancel_delivered {
                edge.cancel_first_tick = Some(tick);
            }
            edge.cancel |= cancel;
            if !edge.delivered && edge.release_tick.is_none() {
                edge.release_tick = release_tick;
            }
            return;
        }
        if self.edges.len() >= MAX_RETAINED_ACTIVATION_EDGES {
            if self.admitted.contains(&token) {
                self.overflow_cancel = Some(token);
            }
            return;
        }
        self.edges.push_back(RetainedEdge {
            token,
            release_tick,
            cancel,
            first_tick: tick,
            cancel_first_tick: cancel.then_some(tick),
            delivered: false,
            cancel_delivered: false,
        });
    }
    pub fn admit(&mut self, token: ActivationToken) -> bool {
        if self.admitted.contains(&token) {
            return true;
        }
        if self.is_settled(token) {
            return false;
        }
        if self.admitted.len() == MAX_RETAINED_ACTIVATION_EDGES {
            self.admitted.pop_front();
        }
        self.admitted.push_back(token);
        true
    }
    pub fn deliver(
        &mut self,
        command: &mut ActivationInput,
        tick: u32,
        live: Option<ActivationToken>,
    ) {
        self.prune(tick);
        command.release = None;
        command.cancel = None;
        // Queue admission does not authorize a competing start. Its edges must not
        // displace the execution actually owned by the host ledger.
        let cancel_edge = self
            .edges
            .iter_mut()
            .filter(|edge| {
                edge.cancel && !edge.cancel_delivered && self.admitted.contains(&edge.token)
            })
            .min_by_key(|edge| live != Some(edge.token));
        let prefer_retained = cancel_edge
            .as_ref()
            .is_some_and(|edge| live == Some(edge.token))
            && self.overflow_cancel != live;
        if self.overflow_cancel.is_some() && !prefer_retained {
            command.cancel = self.overflow_cancel.take();
        } else if let Some(edge) = cancel_edge {
            command.cancel = Some(edge.token);
            edge.cancel_delivered = true;
            edge.delivered = true;
        }
        if let Some(edge) = self
            .edges
            .iter_mut()
            .filter(|edge| {
                !edge.delivered
                    && !edge.cancel
                    && command.cancel != Some(edge.token)
                    && edge.release_tick.is_some()
                    && self.admitted.contains(&edge.token)
            })
            .min_by_key(|edge| live != Some(edge.token))
        {
            command.release = edge.release_tick.map(|release_tick| ActivationRelease {
                token: edge.token,
                release_tick,
            });
            edge.delivered = true;
        }
    }
    pub fn terminal(&mut self, token: ActivationToken) {
        self.admitted.retain(|admitted| *admitted != token);
        self.edges.retain(|edge| edge.token != token);
        self.starts.retain(|start| start.token != token);
        if self.overflow_cancel == Some(token) {
            self.overflow_cancel = None;
        }
        settle(&mut self.settled_start, token);
    }
    fn is_settled(&self, token: ActivationToken) -> bool {
        self.settled_start[token.lane as usize]
            .is_some_and(|watermark| (token.start_tick.wrapping_sub(watermark) as i32) <= 0)
    }
    fn prune(&mut self, tick: u32) {
        for edge in &mut self.edges {
            if tick.wrapping_sub(edge.first_tick) >= RETENTION_TICKS {
                edge.delivered = true;
            }
            if edge
                .cancel_first_tick
                .is_some_and(|first| tick.wrapping_sub(first) >= RETENTION_TICKS)
            {
                edge.cancel_delivered = true;
            }
        }
        self.edges.retain(|edge| {
            // A live activation keeps its delivered-edge history until terminal,
            // so duplicate releases cannot revive while later cancellation remains valid.
            let retain = self.admitted.contains(&edge.token)
                || tick.wrapping_sub(edge.first_tick) < RETENTION_TICKS
                || edge
                    .cancel_first_tick
                    .is_some_and(|first| tick.wrapping_sub(first) < RETENTION_TICKS);
            if !retain {
                settle(&mut self.settled_start, edge.token);
            }
            retain
        });
    }
}
fn settle(watermarks: &mut [Option<u32>; 2], token: ActivationToken) {
    let watermark = &mut watermarks[token.lane as usize];
    if watermark.is_none_or(|old| (token.start_tick.wrapping_sub(old) as i32) > 0) {
        *watermark = Some(token.start_tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::ActivationLane;
    fn token(tick: u32) -> ActivationToken {
        ActivationToken {
            start_tick: tick,
            lane: ActivationLane::Secondary,
        }
    }
    fn aim(yaw: f32) -> RemoteStartAim {
        RemoteStartAim { pitch: 0.1, yaw }
    }
    #[test]
    fn activation_start_lane_keeps_first_aim_and_ignores_duplicates_settled_starts_and_overflow() {
        let mut edges = ActivationEdges::default();
        edges.observe_start(token(4), 4, 0, aim(0.0));
        edges.observe_start(token(4), 9, 0, aim(0.7));
        assert_eq!(edges.starts.len(), 1, "a duplicate start is retained once");
        assert_eq!(
            edges.due_start(3, 0, false),
            None,
            "not due before its tick"
        );
        let Some(DueStart::Start(start)) = edges.due_start(4, 0, false) else {
            panic!("a resolved start is due");
        };
        assert_eq!(edges.delivered_aim(start.token), None, "not yet delivered");
        assert!(edges.take_start(start.token));
        assert_eq!(
            edges.delivered_aim(start.token),
            Some(aim(0.0)),
            "the delivered start keeps its first carrying command's aim"
        );
        assert_eq!(edges.delivered_aim(token(5)), None);
        edges.terminal(start.token);
        edges.observe_start(token(4), 4, 0, aim(0.0));
        edges.observe_start(token(3), 3, 0, aim(0.0));
        assert!(
            edges.starts.is_empty(),
            "settled starts are never retained again"
        );
        for tick in 5..5 + MAX_RETAINED_ACTIVATION_EDGES as u32 + 1 {
            edges.observe_start(token(tick), tick, 0, aim(0.0));
        }
        assert_eq!(edges.starts.len(), MAX_RETAINED_ACTIVATION_EDGES);
        assert!(
            edges.starts.iter().all(|start| start.token != token(69)),
            "overflow drops the newest start"
        );
    }
    #[test]
    fn activation_edge_release_then_cancel_delivers_both_once() {
        let token = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        edges.observe(release, 10);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 10, None);
        assert_eq!(delivered.release, release.release);
        let cancel = ActivationInput {
            cancel: Some(token),
            ..ActivationInput::default()
        };
        edges.observe(cancel, 11);
        edges.deliver(&mut delivered, 11, None);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(cancel, 12);
        edges.deliver(&mut delivered, 12, None);
        assert_eq!(delivered, ActivationInput::default());
    }
    #[test]
    fn activation_edge_same_token_cancel_suppresses_release_without_redelivery() {
        let token = token(4);
        let input = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            cancel: Some(token),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        edges.observe(input, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None);
        assert_eq!(delivered.cancel, Some(token));
        assert!(delivered.release.is_none());
        edges.observe(input, 2);
        edges.deliver(&mut delivered, 2, None);
        assert_eq!(delivered, ActivationInput::default());
    }
    #[test]
    fn activation_edge_duplicate_does_not_refresh_unknown_expiry() {
        let token = token(4);
        let input = ActivationInput {
            cancel: Some(token),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.observe(input, 0);
        edges.observe(input, 119);
        edges.admit(token);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 120, None);
        assert!(delivered.cancel.is_none());
    }
    #[test]
    fn activation_edge_overflow_cancels_affected_live_execution() {
        let mut edges = ActivationEdges::default();
        for start in 0..64 {
            edges.observe(
                ActivationInput {
                    cancel: Some(token(start)),
                    ..ActivationInput::default()
                },
                0,
            );
        }
        let active = token(65);
        edges.admit(token(0));
        edges.admit(active);
        edges.observe(
            ActivationInput {
                release: Some(ActivationRelease {
                    token: active,
                    release_tick: 80,
                }),
                ..ActivationInput::default()
            },
            1,
        );
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None);
        assert_eq!(delivered.cancel, Some(active));
        assert_eq!(edges.edges.len(), 64);
        edges.deliver(&mut delivered, 2, None);
        assert_eq!(delivered.cancel, Some(token(0)));
    }
    #[test]
    fn activation_edge_late_cancel_uses_its_own_first_receipt_expiry() {
        let token = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(token);
        edges.observe(
            ActivationInput {
                release: Some(ActivationRelease {
                    token,
                    release_tick: 9,
                }),
                ..ActivationInput::default()
            },
            0,
        );
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None);
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            119,
        );
        edges.deliver(&mut delivered, 120, None);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            121,
        );
        edges.deliver(&mut delivered, 121, None);
        assert!(delivered.cancel.is_none());
    }
    #[test]
    fn activation_edge_expired_unknown_cannot_revive_after_duplicate_and_admission() {
        let token = token(4);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        let mut edges = ActivationEdges::default();
        edges.observe(release, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 120, None);
        edges.observe(release, 121);
        edges.admit(token);
        edges.deliver(&mut delivered, 122, None);
        assert_eq!(delivered, ActivationInput::default());
        assert!(edges.edges.is_empty());
        assert!(edges.admitted.is_empty());
    }
    #[test]
    fn activation_edge_unknown_watermark_preserves_live_release_history_and_later_cancel() {
        let live = token(4);
        let mut edges = ActivationEdges::default();
        edges.admit(live);
        let release = ActivationInput {
            release: Some(ActivationRelease {
                token: live,
                release_tick: 9,
            }),
            ..ActivationInput::default()
        };
        edges.observe(release, 0);
        let mut delivered = ActivationInput::default();
        edges.deliver(&mut delivered, 1, None);
        assert_eq!(delivered.release, release.release);
        edges.observe(
            ActivationInput {
                cancel: Some(token(5)),
                ..ActivationInput::default()
            },
            0,
        );
        edges.deliver(&mut delivered, 120, None);
        edges.observe(release, 121);
        edges.deliver(&mut delivered, 121, None);
        assert_eq!(delivered, ActivationInput::default());
        edges.observe(
            ActivationInput {
                cancel: Some(live),
                ..ActivationInput::default()
            },
            122,
        );
        edges.deliver(&mut delivered, 122, None);
        assert_eq!(delivered.cancel, Some(live));
    }
}
