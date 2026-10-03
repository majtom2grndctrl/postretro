//! Correlated release/cancel recovery beside movement playout.
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
#[derive(Debug, Default)]
pub(crate) struct ActivationEdges {
    edges: VecDeque<RetainedEdge>,
    admitted: VecDeque<ActivationToken>,
    overflow_cancel: Option<ActivationToken>,
    settled_start: [Option<u32>; 2],
}
impl ActivationEdges {
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
    pub fn deliver(&mut self, command: &mut ActivationInput, tick: u32) {
        self.prune(tick);
        command.release = None;
        command.cancel = self.overflow_cancel.take();
        if command.cancel.is_some() {
            return;
        }
        if let Some(edge) = self.edges.iter_mut().find(|e| {
            (!e.delivered || (e.cancel && !e.cancel_delivered)) && self.admitted.contains(&e.token)
        }) {
            if edge.cancel && !edge.cancel_delivered {
                command.cancel = Some(edge.token);
                edge.cancel_delivered = true;
            } else if let Some(release_tick) = edge.release_tick {
                command.release = Some(ActivationRelease {
                    token: edge.token,
                    release_tick,
                });
            }
            edge.delivered = true;
        }
    }
    pub fn terminal(&mut self, token: ActivationToken) {
        self.admitted.retain(|admitted| *admitted != token);
        self.edges.retain(|edge| edge.token != token);
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
        edges.deliver(&mut delivered, 10);
        assert_eq!(delivered.release, release.release);
        let cancel = ActivationInput {
            cancel: Some(token),
            ..ActivationInput::default()
        };
        edges.observe(cancel, 11);
        edges.deliver(&mut delivered, 11);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(cancel, 12);
        edges.deliver(&mut delivered, 12);
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
        edges.deliver(&mut delivered, 120);
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
        edges.deliver(&mut delivered, 1);
        assert_eq!(delivered.cancel, Some(active));
        assert_eq!(edges.edges.len(), 64);
        edges.deliver(&mut delivered, 2);
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
        edges.deliver(&mut delivered, 1);
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            119,
        );
        edges.deliver(&mut delivered, 120);
        assert_eq!(delivered.cancel, Some(token));
        edges.observe(
            ActivationInput {
                cancel: Some(token),
                ..ActivationInput::default()
            },
            121,
        );
        edges.deliver(&mut delivered, 121);
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
        edges.deliver(&mut delivered, 120);
        edges.observe(release, 121);
        edges.admit(token);
        edges.deliver(&mut delivered, 122);
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
        edges.deliver(&mut delivered, 1);
        assert_eq!(delivered.release, release.release);
        edges.observe(
            ActivationInput {
                cancel: Some(token(5)),
                ..ActivationInput::default()
            },
            0,
        );
        edges.deliver(&mut delivered, 120);
        edges.observe(release, 121);
        edges.deliver(&mut delivered, 121);
        assert_eq!(delivered, ActivationInput::default());
        edges.observe(
            ActivationInput {
                cancel: Some(live),
                ..ActivationInput::default()
            },
            122,
        );
        edges.deliver(&mut delivered, 122);
        assert_eq!(delivered.cancel, Some(live));
    }
}
