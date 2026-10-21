// Client weapon-switch declaration chain: the send, the pending chain and outcome resolution.
// See: context/lib/networking.md §Combat authority

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSwitchDeclaration {
    pub declaration_id: u32,
    pub target_slot: u8,
    /// Slot the client actually presented as active when this local switch chain
    /// began. Older host outcomes may move the authoritative rollback point, but
    /// must not turn an unpresented intermediate target into last-weapon history.
    pub held_origin_slot: usize,
    /// Host-authoritative active slot restored if this declaration is refused.
    /// Ordered predecessor outcomes rebase this without changing local history.
    pub rollback_slot: usize,
    pub rollback_last_weapon_slot: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchOutcome {
    Accepted(ServerSwitchAccepted),
    Refused(ServerSwitchRefused),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentSwitchResolution {
    None,
    Accepted {
        last_weapon_slot: Option<usize>,
    },
    Refused {
        target_slot: usize,
        rollback_slot: usize,
        last_weapon_slot: Option<usize>,
    },
}

impl NetEndpoint {
    /// Send a client-local switch declaration over reliable Control. The client
    /// transport refuses to queue it before participation, so an old level cannot
    /// leak a selection into a newly promoted pawn. `client_tick` is the tick of
    /// the input command the switch was made on; the host orders the switch
    /// against that command's neighbours by it.
    pub fn send_client_switch_declaration(
        &mut self,
        slot: u8,
        client_tick: u32,
        rollback_slot: usize,
        rollback_last_weapon_slot: Option<usize>,
    ) {
        if let Self::Client {
            client,
            next_switch_declaration_id,
            pending_switch_declarations,
            ..
        } = self
        {
            let declaration_id = *next_switch_declaration_id;
            *next_switch_declaration_id = (*next_switch_declaration_id).wrapping_add(1);
            pending_switch_declarations.push_back(PendingSwitchDeclaration {
                declaration_id,
                target_slot: slot,
                held_origin_slot: rollback_slot,
                rollback_slot,
                rollback_last_weapon_slot,
            });
            client.send_switch_declaration(ClientSwitchDeclaration {
                declaration_id,
                slot,
                client_tick,
            });
        }
    }

    /// Whether this client has a switch declaration the host has not yet
    /// answered. `false` on a host.
    pub fn has_pending_switch_declarations(&self) -> bool {
        matches!(
            self,
            Self::Client {
                pending_switch_declarations,
                ..
            } if !pending_switch_declarations.is_empty()
        )
    }

    pub fn take_switch_outcome(&mut self, outcome: SwitchOutcome) -> CurrentSwitchResolution {
        let Self::Client {
            pending_switch_declarations,
            ..
        } = self
        else {
            return CurrentSwitchResolution::None;
        };
        resolve_switch_outcome(pending_switch_declarations, outcome)
    }
}

pub(crate) fn resolve_switch_outcome(
    pending: &mut VecDeque<PendingSwitchDeclaration>,
    outcome: SwitchOutcome,
) -> CurrentSwitchResolution {
    let (declaration_id, target_slot, accepted) = match outcome {
        SwitchOutcome::Accepted(accepted) => (accepted.declaration_id, accepted.slot, true),
        SwitchOutcome::Refused(refused) => (refused.declaration_id, refused.slot, false),
    };
    let Some(front) = pending.front().copied() else {
        return CurrentSwitchResolution::None;
    };
    // Both directions use reliable-ordered Control. Refuse an out-of-order or
    // stale outcome rather than letting it skip an unresolved predecessor.
    if front.declaration_id != declaration_id || front.target_slot != target_slot {
        return CurrentSwitchResolution::None;
    }
    let settled = pending
        .pop_front()
        .expect("the non-empty declaration chain was checked");
    let (settled_active, settled_last_weapon) = if accepted {
        // A declaration naming the slot the client held when it made it is the
        // active-slot re-sync: the local switch machine never emits one, and a
        // round trip back (0 -> 1 -> 0) holds the intermediate slot as its
        // origin. Accepting it makes no last-weapon history.
        let last_weapon = if usize::from(settled.target_slot) == settled.held_origin_slot {
            settled.rollback_last_weapon_slot
        } else {
            Some(settled.held_origin_slot)
        };
        (usize::from(settled.target_slot), last_weapon)
    } else {
        (settled.rollback_slot, settled.rollback_last_weapon_slot)
    };

    if let Some(next) = pending.front_mut() {
        // An older outcome changes where a refusal of the next declaration must
        // return host authority. It does not change which slot the client had
        // actually held before the superseding local switch, nor should it touch
        // the newest in-flight presentation or last-weapon memory.
        next.rollback_slot = settled_active;
        next.rollback_last_weapon_slot = settled_last_weapon;
        return CurrentSwitchResolution::None;
    }

    if accepted {
        CurrentSwitchResolution::Accepted {
            last_weapon_slot: settled_last_weapon,
        }
    } else {
        CurrentSwitchResolution::Refused {
            target_slot: usize::from(settled.target_slot),
            rollback_slot: settled_active,
            last_weapon_slot: settled_last_weapon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_double_refusal_rebases_newer_declaration_to_authoritative_origin() {
        // Regression: A->B D1 completed locally, then B->C D2 completed. When the
        // host refused both, retaining only D2 rolled the client back to B, not A.
        let mut pending = VecDeque::from([
            PendingSwitchDeclaration {
                declaration_id: 11,
                target_slot: 1,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(9),
            },
            PendingSwitchDeclaration {
                declaration_id: 12,
                target_slot: 2,
                held_origin_slot: 1,
                rollback_slot: 1,
                rollback_last_weapon_slot: Some(0),
            },
        ]);

        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Refused(ServerSwitchRefused {
                    declaration_id: 11,
                    slot: 1,
                }),
            ),
            CurrentSwitchResolution::None
        );
        assert_eq!(pending.front().unwrap().rollback_slot, 0);
        assert_eq!(pending.front().unwrap().rollback_last_weapon_slot, Some(9));
        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Refused(ServerSwitchRefused {
                    declaration_id: 12,
                    slot: 2,
                }),
            ),
            CurrentSwitchResolution::Refused {
                target_slot: 2,
                rollback_slot: 0,
                last_weapon_slot: Some(9),
            }
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn o29_ordered_accept_accept_retains_the_slot_the_client_actually_held() {
        // Regression: A->B and A->C were both emitted before the client repointed;
        // accepting B first must not make the unpresented B become last-weapon.
        let mut pending = VecDeque::from([
            PendingSwitchDeclaration {
                declaration_id: 21,
                target_slot: 1,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(9),
            },
            PendingSwitchDeclaration {
                declaration_id: 22,
                target_slot: 2,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(0),
            },
        ]);

        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Accepted(ServerSwitchAccepted {
                    declaration_id: 21,
                    slot: 1,
                }),
            ),
            CurrentSwitchResolution::None,
            "the superseded predecessor never mutates the newest local presentation"
        );
        assert_eq!(pending.front().unwrap().rollback_slot, 1);
        assert_eq!(pending.front().unwrap().held_origin_slot, 0);
        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Accepted(ServerSwitchAccepted {
                    declaration_id: 22,
                    slot: 2,
                }),
            ),
            CurrentSwitchResolution::Accepted {
                last_weapon_slot: Some(0),
            }
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn o29_ordered_accept_refuse_rolls_back_to_host_slot_with_local_history() {
        let mut pending = VecDeque::from([
            PendingSwitchDeclaration {
                declaration_id: 31,
                target_slot: 1,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(9),
            },
            PendingSwitchDeclaration {
                declaration_id: 32,
                target_slot: 2,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(0),
            },
        ]);

        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Accepted(ServerSwitchAccepted {
                    declaration_id: 31,
                    slot: 1,
                }),
            ),
            CurrentSwitchResolution::None
        );
        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Refused(ServerSwitchRefused {
                    declaration_id: 32,
                    slot: 2,
                }),
            ),
            CurrentSwitchResolution::Refused {
                target_slot: 2,
                rollback_slot: 1,
                last_weapon_slot: Some(0),
            }
        );
        assert!(pending.is_empty());
    }

    // The active-slot re-sync declares the slot the client already holds. Its
    // accept leaves last-weapon memory as it was; its refusal rolls back to
    // that same slot, so neither outcome changes what the player holds.
    #[test]
    fn same_slot_resync_declaration_settles_without_moving_selection_or_history() {
        let resync = |declaration_id| PendingSwitchDeclaration {
            declaration_id,
            target_slot: 0,
            held_origin_slot: 0,
            rollback_slot: 0,
            rollback_last_weapon_slot: Some(2),
        };
        let mut pending = VecDeque::from([resync(41)]);
        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Accepted(ServerSwitchAccepted {
                    declaration_id: 41,
                    slot: 0,
                }),
            ),
            CurrentSwitchResolution::Accepted {
                last_weapon_slot: Some(2),
            }
        );
        assert!(pending.is_empty());

        let mut pending = VecDeque::from([resync(42)]);
        assert_eq!(
            resolve_switch_outcome(
                &mut pending,
                SwitchOutcome::Refused(ServerSwitchRefused {
                    declaration_id: 42,
                    slot: 0,
                }),
            ),
            CurrentSwitchResolution::Refused {
                target_slot: 0,
                rollback_slot: 0,
                last_weapon_slot: Some(2),
            }
        );
        assert!(pending.is_empty());
    }

    // A round trip back (0 -> 1 -> 0) also ends on the slot the chain started
    // from, but each declaration holds the slot it switched away from as its
    // origin, so the return is an ordinary switch: its accept remembers the
    // intermediate slot, never the re-sync's unchanged history.
    #[test]
    fn round_trip_switch_chain_is_not_mistaken_for_a_resync() {
        let mut pending = VecDeque::from([
            PendingSwitchDeclaration {
                declaration_id: 1,
                target_slot: 1,
                held_origin_slot: 0,
                rollback_slot: 0,
                rollback_last_weapon_slot: Some(2),
            },
            PendingSwitchDeclaration {
                declaration_id: 2,
                target_slot: 0,
                held_origin_slot: 1,
                rollback_slot: 1,
                rollback_last_weapon_slot: Some(0),
            },
        ]);
        for (declaration_id, slot, expected) in [
            (1, 1, CurrentSwitchResolution::None),
            (
                2,
                0,
                CurrentSwitchResolution::Accepted {
                    last_weapon_slot: Some(1),
                },
            ),
        ] {
            assert_eq!(
                resolve_switch_outcome(
                    &mut pending,
                    SwitchOutcome::Accepted(ServerSwitchAccepted {
                        declaration_id,
                        slot,
                    }),
                ),
                expected
            );
        }
        assert!(pending.is_empty());
    }
}
